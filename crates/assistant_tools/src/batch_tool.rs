use crate::schema::json_schema_for;
use anyhow::{Result, anyhow};
use assistant_tool::{ActionLog, Tool, ToolResult, ToolWorkingSet};
use futures::future::join_all;
use gpui::{AnyWindowHandle, App, Entity, Task};
use language_model::{
    LanguageModel, LanguageModelRegistry, LanguageModelRequest, LanguageModelToolSchemaFormat,
};
use project::Project;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use ui::IconName;

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ToolInvocation {
    pub name: String,

    pub input: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BatchToolInput {
    pub invocations: Vec<ToolInvocation>,

    #[serde(default)]
    pub run_tools_concurrently: bool,
}

pub struct BatchTool;

impl Tool for BatchTool {
    fn name(&self) -> String {
        "batch-tool".into()
    }

    fn needs_confirmation(&self, _input: &serde_json::Value, _cx: &App) -> bool {
        true
    }

    fn description(&self) -> String {
        include_str!("./batch_tool/description.md").into()
    }

    fn icon(&self) -> IconName {
        IconName::Cog
    }

    fn input_schema(&self, format: LanguageModelToolSchemaFormat) -> Result<serde_json::Value> {
        json_schema_for::<BatchToolInput>(format)
    }

    fn ui_text(&self, input: &serde_json::Value) -> String {
        match serde_json::from_value::<BatchToolInput>(input.clone()) {
            Ok(input) => {
                let count = input.invocations.len();
                let mode = if input.run_tools_concurrently {
                    "concurrently"
                } else {
                    "sequentially"
                };

                let first_tool_name = input
                    .invocations
                    .first()
                    .map(|inv| inv.name.clone())
                    .unwrap_or_default();

                let all_same = input
                    .invocations
                    .iter()
                    .all(|invocation| invocation.name == first_tool_name);

                if all_same {
                    format!(
                        "Run `{}` {} times {}",
                        first_tool_name,
                        input.invocations.len(),
                        mode
                    )
                } else {
                    format!("Run {} tools {}", count, mode)
                }
            }
            Err(_) => "Batch tools".to_string(),
        }
    }

    fn run(
        self: Arc<Self>,
        input: serde_json::Value,
        request: Arc<LanguageModelRequest>,
        project: Entity<Project>,
        action_log: Entity<ActionLog>,
        _model: Arc<dyn LanguageModel>,
        _window: Option<AnyWindowHandle>,
        cx: &mut App,
    ) -> ToolResult {
        let input = match serde_json::from_value::<BatchToolInput>(input) {
            Ok(input) => input,
            Err(err) => return Task::ready(Err(anyhow!(err))).into(),
        };

        if input.invocations.is_empty() {
            return Task::ready(Err(anyhow!("No tool invocations provided"))).into();
        }

        let run_tools_concurrently = input.run_tools_concurrently;

        let foreground_task = {
            let working_set = ToolWorkingSet::default();
            let invocations = input.invocations;
            let request = request.clone();

            cx.spawn(async move |cx| {
                let mut tasks = Vec::new();
                let mut tool_names = Vec::new();

                for invocation in invocations {
                    let tool_name = invocation.name.clone();
                    tool_names.push(tool_name.clone());

                    let tool = cx
                        .update(|cx| working_set.tool(&tool_name, cx))
                        .map_err(|err| {
                            anyhow!("Failed to look up tool '{}': {}", tool_name, err)
                        })?;

                    let Some(tool) = tool else {
                        return Err(anyhow!("Tool '{}' not found", tool_name));
                    };

                    let project = project.clone();
                    let action_log = action_log.clone();
                    let request = request.clone();
                    let tool_result = cx
                        .update(|cx| {
                            // Get a real model from the registry
                            let model = LanguageModelRegistry::global(cx)
                                .read(cx)
                                .default_model()
                                .map(|configured_model| configured_model.model.clone())
                                .unwrap_or_else(|| {
                                    // Create a minimal fallback if no model is available
                                    Arc::new(FallbackLanguageModel) as Arc<dyn LanguageModel>
                                });
                            let window: Option<AnyWindowHandle> = None;
                            tool.run(
                                invocation.input,
                                request,
                                project,
                                action_log,
                                model,
                                window,
                                cx,
                            )
                        })
                        .map_err(|err| anyhow!("Failed to start tool '{}': {}", tool_name, err))?;

                    tasks.push(tool_result.output);
                }

                Ok((tasks, tool_names))
            })
        };

        cx.spawn(
            async move |_cx| -> Result<assistant_tool::ToolResultOutput> {
                let (tasks, tool_names) = foreground_task.await?;
                let mut results = Vec::with_capacity(tasks.len());

                if run_tools_concurrently {
                    results.extend(join_all(tasks).await)
                } else {
                    for task in tasks {
                        results.push(task.await);
                    }
                };

                let mut formatted_results = String::new();
                let mut error_occurred = false;

                for (i, result) in results.into_iter().enumerate() {
                    let tool_name = &tool_names[i];

                    match result {
                        Ok(output) => {
                            let output_text = match output.content {
                                assistant_tool::ToolResultContent::Text(text) => text,
                                assistant_tool::ToolResultContent::Image(_) => {
                                    "[Image output]".to_string()
                                }
                            };
                            formatted_results.push_str(&format!(
                                "Tool '{}' result:\n{}\n\n",
                                tool_name, output_text
                            ));
                        }
                        Err(err) => {
                            error_occurred = true;
                            formatted_results
                                .push_str(&format!("Tool '{}' error: {}\n\n", tool_name, err));
                        }
                    }
                }

                if error_occurred {
                    formatted_results.push_str(
                        "Note: Some tool invocations failed. See individual results above.",
                    );
                }

                Ok(formatted_results.trim().to_string().into())
            },
        )
        .into()
    }
}

struct FallbackLanguageModel;

impl LanguageModel for FallbackLanguageModel {
    fn id(&self) -> language_model::LanguageModelId {
        language_model::LanguageModelId::from("fallback".to_string())
    }

    fn name(&self) -> language_model::LanguageModelName {
        language_model::LanguageModelName::from("fallback".to_string())
    }

    fn provider_id(&self) -> language_model::LanguageModelProviderId {
        language_model::LanguageModelProviderId::from("fallback".to_string())
    }

    fn provider_name(&self) -> language_model::LanguageModelProviderName {
        language_model::LanguageModelProviderName::from("fallback".to_string())
    }

    fn telemetry_id(&self) -> String {
        "fallback".to_string()
    }

    fn max_token_count(&self) -> usize {
        4096
    }

    fn supports_images(&self) -> bool {
        false
    }

    fn supports_tools(&self) -> bool {
        false
    }

    fn supports_tool_choice(&self, _choice: language_model::LanguageModelToolChoice) -> bool {
        false
    }

    fn count_tokens(
        &self,
        _request: language_model::LanguageModelRequest,
        _cx: &gpui::App,
    ) -> futures::future::BoxFuture<'static, anyhow::Result<usize>> {
        Box::pin(async { Ok(0) })
    }

    fn stream_completion(
        &self,
        _request: language_model::LanguageModelRequest,
        _cx: &gpui::AsyncApp,
    ) -> futures::future::BoxFuture<
        'static,
        anyhow::Result<
            futures::stream::BoxStream<
                'static,
                anyhow::Result<
                    language_model::LanguageModelCompletionEvent,
                    language_model::LanguageModelCompletionError,
                >,
            >,
        >,
    > {
        Box::pin(async {
            Err(anyhow::anyhow!(
                "FallbackLanguageModel does not support streaming"
            ))
        })
    }
}

use anyhow::{Result, anyhow};
use futures::{FutureExt, StreamExt, future::BoxFuture};
use gpui::{AnyView, App, AsyncApp, Context, Subscription, Task, Window};
use http_client::HttpClient;
use language_model::{
    AuthenticateError, LanguageModel, LanguageModelCompletionError, LanguageModelCompletionEvent,
    LanguageModelId, LanguageModelName, LanguageModelProvider, LanguageModelProviderId,
    LanguageModelProviderName, LanguageModelProviderState, LanguageModelRequest,
    LanguageModelToolChoice, LanguageModelToolResultContent, LanguageModelToolUse, MessageContent,
    RateLimiter, Role, StopReason,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use settings::{Settings, SettingsStore};
use std::collections::HashMap;
use std::sync::Arc;
use tensor_zero::{ResponseStreamEvent, stream_completion};
use ui::{IconName, prelude::*};

use crate::AllLanguageModelSettings;

const PROVIDER_ID: &str = "tensorzero";
const PROVIDER_NAME: &str = "TensorZero";

#[derive(Default, Clone, Debug, PartialEq)]
pub struct TensorZeroSettings {
    pub api_url: String,
    pub available_models: Vec<AvailableModel>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AvailableModel {
    pub name: String,
    pub display_name: Option<String>,
    pub max_tokens: usize,
    pub max_output_tokens: Option<u32>,
    pub max_completion_tokens: Option<u32>,
}

pub struct TensorZeroLanguageModelProvider {
    http_client: Arc<dyn HttpClient>,
    state: gpui::Entity<State>,
}

pub struct State {
    available_models: Vec<tensor_zero::Model>,
    _subscription: Subscription,
}

impl State {
    fn is_authenticated(&self) -> bool {
        // TensorZero runs locally, no authentication needed
        true
    }

    fn available_models(&self) -> Vec<tensor_zero::Model> {
        self.available_models.clone()
    }

    fn update_models(&mut self, cx: &mut Context<Self>) {
        let settings = &AllLanguageModelSettings::get_global(cx).tensor_zero;

        let mut models = Vec::new();
        for model in &settings.available_models {
            models.push(tensor_zero::Model::new(
                &model.name,
                model.display_name.as_deref(),
                Some(model.max_tokens),
                Some(true),  // Tools supported by default
                Some(false), // Parallel tool calls false by default
            ));
        }

        self.available_models = models;
        cx.notify();
    }
}

impl TensorZeroLanguageModelProvider {
    pub fn new(http_client: Arc<dyn HttpClient>, cx: &mut App) -> Self {
        let state = cx.new(|cx| State {
            available_models: Vec::new(),
            _subscription: cx.observe_global::<SettingsStore>(|this: &mut State, cx| {
                this.update_models(cx);
                cx.notify();
            }),
        });

        state.update(cx, |state, cx| {
            state.update_models(cx);
        });

        Self { http_client, state }
    }

    fn create_language_model(&self, model: tensor_zero::Model) -> Arc<dyn LanguageModel> {
        Arc::new(TensorZeroLanguageModel {
            id: LanguageModelId::from(model.id().to_string()),
            model,
            state: self.state.clone(),
            http_client: self.http_client.clone(),
            request_limiter: RateLimiter::new(4),
        })
    }
}

impl LanguageModelProviderState for TensorZeroLanguageModelProvider {
    type ObservableEntity = State;

    fn observable_entity(&self) -> Option<gpui::Entity<Self::ObservableEntity>> {
        Some(self.state.clone())
    }
}

impl LanguageModelProvider for TensorZeroLanguageModelProvider {
    fn id(&self) -> LanguageModelProviderId {
        LanguageModelProviderId(PROVIDER_ID.into())
    }

    fn name(&self) -> LanguageModelProviderName {
        LanguageModelProviderName(PROVIDER_NAME.into())
    }

    fn icon(&self) -> IconName {
        IconName::AiTensorZero
    }

    fn default_model(&self, _cx: &App) -> Option<Arc<dyn LanguageModel>> {
        Some(self.create_language_model(tensor_zero::Model::default()))
    }

    fn default_fast_model(&self, _cx: &App) -> Option<Arc<dyn LanguageModel>> {
        Some(self.create_language_model(tensor_zero::Model::default_fast()))
    }

    fn provided_models(&self, cx: &App) -> Vec<Arc<dyn LanguageModel>> {
        let mut models = Vec::new();

        // Add settings models
        let settings_models = &AllLanguageModelSettings::get_global(cx)
            .tensor_zero
            .available_models;

        for model in settings_models {
            models.push(self.create_language_model(tensor_zero::Model {
                name: model.name.clone(),
                display_name: model.display_name.clone(),
                max_tokens: model.max_tokens,
                supports_tools: Some(true),
                supports_parallel_tool_calls: Some(false),
            }));
        }

        // Add state models if available
        let state_models = self.state.read(cx).available_models();
        for model in state_models {
            if !models.iter().any(|m| m.id().0 == model.id()) {
                models.push(self.create_language_model(model));
            }
        }

        models
    }

    fn is_authenticated(&self, cx: &App) -> bool {
        self.state.read(cx).is_authenticated()
    }

    fn authenticate(&self, _cx: &mut App) -> Task<Result<(), AuthenticateError>> {
        // TensorZero runs locally, no authentication needed
        Task::ready(Ok(()))
    }

    fn configuration_view(&self, _window: &mut Window, cx: &mut App) -> AnyView {
        let state = self.state.clone();
        cx.new(move |cx| ConfigurationView::new(state, cx)).into()
    }

    fn reset_credentials(&self, _cx: &mut App) -> Task<Result<()>> {
        // TensorZero runs locally, no credentials to reset
        Task::ready(Ok(()))
    }
}

pub struct TensorZeroLanguageModel {
    id: LanguageModelId,
    model: tensor_zero::Model,
    state: gpui::Entity<State>,
    http_client: Arc<dyn HttpClient>,
    request_limiter: RateLimiter,
}

impl TensorZeroLanguageModel {
    fn stream_completion(
        &self,
        request: tensor_zero::Request,
        cx: &AsyncApp,
    ) -> BoxFuture<'static, Result<futures::stream::BoxStream<'static, Result<ResponseStreamEvent>>>>
    {
        let http_client = self.http_client.clone();

        let Ok(api_url) = cx.read_entity(&self.state, |_state, cx| {
            let settings = &AllLanguageModelSettings::get_global(cx).tensor_zero;
            settings.api_url.clone()
        }) else {
            return async move { Err(anyhow!("Could not read TensorZero settings")) }.boxed();
        };

        let future = self.request_limiter.stream(async move {
            let response = stream_completion(http_client.as_ref(), &api_url, request).await?;
            Ok(response)
        });

        async move { Ok(future.await?.boxed()) }.boxed()
    }
}

impl LanguageModel for TensorZeroLanguageModel {
    fn id(&self) -> LanguageModelId {
        self.id.clone()
    }

    fn name(&self) -> LanguageModelName {
        LanguageModelName::from(self.model.display_name().to_string())
    }

    fn provider_id(&self) -> LanguageModelProviderId {
        LanguageModelProviderId(PROVIDER_ID.into())
    }

    fn provider_name(&self) -> LanguageModelProviderName {
        LanguageModelProviderName(PROVIDER_NAME.into())
    }

    fn supports_tools(&self) -> bool {
        self.model.supports_tool_calls()
    }

    fn telemetry_id(&self) -> String {
        format!("tensorzero/{}", self.model.id())
    }

    fn max_token_count(&self) -> usize {
        self.model.max_token_count()
    }

    fn max_output_tokens(&self) -> Option<u32> {
        self.model.max_output_tokens()
    }

    fn supports_tool_choice(&self, _tool_choice: LanguageModelToolChoice) -> bool {
        // Generally true for TensorZero models
        true
    }

    fn supports_images(&self) -> bool {
        // TensorZero supports models that can handle images, but this depends on the specific model
        false
    }

    fn count_tokens(
        &self,
        request: LanguageModelRequest,
        _cx: &App,
    ) -> BoxFuture<'static, Result<usize>> {
        async move {
            let mut token_count = 0;
            for message in &request.messages {
                for content in &message.content {
                    match content {
                        MessageContent::Text(text) => {
                            token_count += tensor_zero::count_tensor_zero_tokens(text);
                        }
                        _ => {}
                    }
                }
            }
            Ok(token_count)
        }
        .boxed()
    }

    fn stream_completion(
        &self,
        request: LanguageModelRequest,
        cx: &AsyncApp,
    ) -> BoxFuture<
        'static,
        Result<
            futures::stream::BoxStream<
                'static,
                Result<LanguageModelCompletionEvent, LanguageModelCompletionError>,
            >,
        >,
    > {
        let request = into_tensor_zero_request(request, &self.model);
        let response = self.stream_completion(request, cx);

        async move {
            let events = response.await?;
            let mapper = TensorZeroEventMapper::new();
            Ok(mapper.map_stream(events).boxed())
        }
        .boxed()
    }
}

pub fn into_tensor_zero_request(request: LanguageModelRequest, model: &tensor_zero::Model) -> tensor_zero::Request {
    let mut messages = Vec::new();

    for req_message in request.messages {
        for content in req_message.content {
            match content {
                MessageContent::Text(text) => messages.push(match req_message.role {
                    Role::User => tensor_zero::RequestMessage::User { content: text },
                    Role::Assistant => tensor_zero::RequestMessage::Assistant {
                        content: Some(text),
                        tool_calls: Vec::new(),
                    },
                    Role::System => tensor_zero::RequestMessage::System { content: text },
                }),
                MessageContent::ToolUse(tool_use) => {
                    let tool_call = tensor_zero::ToolCall {
                        id: tool_use.id.to_string(),
                        content: tensor_zero::ToolCallContent::Function {
                            function: tensor_zero::FunctionContent {
                                name: tool_use.name.to_string(),
                                arguments: serde_json::to_string(&tool_use.input)
                                    .unwrap_or_default(),
                            },
                        },
                    };

                    if let Some(tensor_zero::RequestMessage::Assistant { tool_calls, .. }) =
                        messages.last_mut()
                    {
                        tool_calls.push(tool_call);
                    } else {
                        messages.push(tensor_zero::RequestMessage::Assistant {
                            content: None,
                            tool_calls: vec![tool_call],
                        });
                    }
                }
                MessageContent::ToolResult(tool_result) => {
                    let content = match &tool_result.content {
                        LanguageModelToolResultContent::Text(text) => text.to_string(),
                        LanguageModelToolResultContent::Image(_) => {
                            "[Tool responded with an image, but TensorZero doesn't support these yet]".to_string()
                        }
                    };

                    messages.push(tensor_zero::RequestMessage::Tool {
                        content,
                        tool_call_id: tool_result.tool_use_id.to_string(),
                    });
                }
                MessageContent::Image(_) => {}
                MessageContent::Thinking { .. } => {}
                MessageContent::RedactedThinking(_) => {}
            }
        }
    }

    tensor_zero::Request {
        model: model.id().to_string(),
        messages,
        stream: true,
        max_tokens: None, // Let TensorZero handle this
        stop: request.stop,
        temperature: request.temperature.unwrap_or(0.7),
        tool_choice: match request.tool_choice {
            Some(LanguageModelToolChoice::Auto) => Some(tensor_zero::ToolChoice::Auto),
            Some(LanguageModelToolChoice::Any) => Some(tensor_zero::ToolChoice::Required),
            Some(LanguageModelToolChoice::None) => Some(tensor_zero::ToolChoice::None),
            None => None,
        },
        parallel_tool_calls: Some(false), // Default to false as specified
        tools: request
            .tools
            .into_iter()
            .map(|tool| tensor_zero::ToolDefinition::Function {
                function: tensor_zero::FunctionDefinition {
                    name: tool.name,
                    description: Some(tool.description),
                    parameters: Some(tool.input_schema),
                },
            })
            .collect(),
    }
}

pub struct TensorZeroEventMapper {
    tool_calls_by_index: HashMap<usize, RawToolCall>,
}

impl TensorZeroEventMapper {
    pub fn new() -> Self {
        Self {
            tool_calls_by_index: HashMap::default(),
        }
    }

    pub fn map_stream(
        mut self,
        events: futures::stream::BoxStream<'static, Result<ResponseStreamEvent>>,
    ) -> impl futures::stream::Stream<Item = Result<LanguageModelCompletionEvent, LanguageModelCompletionError>>
    {
        events.flat_map(move |event| {
            futures::stream::iter(match event {
                Ok(event) => self.map_event(event),
                Err(error) => vec![Err(LanguageModelCompletionError::Other(error))],
            })
        })
    }

    pub fn map_event(
        &mut self,
        event: ResponseStreamEvent,
    ) -> Vec<Result<LanguageModelCompletionEvent, LanguageModelCompletionError>> {
        let Some(choice) = event.choices.first() else {
            return vec![Err(LanguageModelCompletionError::Other(anyhow!(
                "Response contained no choices"
            )))];
        };

        let mut events = Vec::new();
        
        if let Some(content) = choice.delta.content.clone() {
            events.push(Ok(LanguageModelCompletionEvent::Text(content)));
        }

        if let Some(choice_tool_calls) = choice.delta.tool_calls.as_ref() {
            for tool_call_chunk in choice_tool_calls {
                let raw_tool_call = self
                    .tool_calls_by_index
                    .entry(tool_call_chunk.index)
                    .or_insert_with(|| RawToolCall {
                        id: String::new(),
                        name: String::new(),
                        arguments: String::new(),
                    });

                if let Some(id) = tool_call_chunk.id.clone() {
                    raw_tool_call.id = id;
                }

                if let Some(function) = tool_call_chunk.function.as_ref() {
                    if let Some(name) = function.name.clone() {
                        raw_tool_call.name.push_str(&name);
                    }
                    if let Some(arguments) = function.arguments.clone() {
                        raw_tool_call.arguments.push_str(&arguments);
                    }
                }
            }
        }

        match choice.finish_reason.as_deref() {
            Some("stop") => {
                events.push(Ok(LanguageModelCompletionEvent::Stop(
                    StopReason::EndTurn
                )));
            }
            Some("tool_calls") => {
                events.extend(self.tool_calls_by_index.drain().map(|(_, tool_call)| {
                    match serde_json::from_str(&tool_call.arguments) {
                        Ok(input) => Ok(LanguageModelCompletionEvent::ToolUse(
                            LanguageModelToolUse {
                                id: tool_call.id.clone().into(),
                                name: tool_call.name.as_str().into(),
                                is_input_complete: true,
                                input,
                                raw_input: tool_call.arguments.clone(),
                            },
                        )),
                        Err(error) => Err(LanguageModelCompletionError::BadInputJson {
                            id: tool_call.id.into(),
                            tool_name: tool_call.name.as_str().into(),
                            raw_input: tool_call.arguments.into(),
                            json_parse_error: error.to_string(),
                        }),
                    }
                }));

                events.push(Ok(LanguageModelCompletionEvent::Stop(StopReason::ToolUse)));
            }
            Some("length") => {
                events.push(Ok(LanguageModelCompletionEvent::Stop(
                    StopReason::MaxTokens
                )));
            }
            _ => {}
        }

        events
    }
}

#[derive(Clone)]
struct RawToolCall {
    id: String,
    name: String,
    arguments: String,
}

pub fn count_tensor_zero_tokens(content: &str) -> usize {
    tensor_zero::count_tensor_zero_tokens(content)
}

struct ConfigurationView;

impl ConfigurationView {
    fn new(_state: gpui::Entity<State>, _cx: &mut Context<Self>) -> Self {
        Self
    }
}

impl Render for ConfigurationView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().px_2().child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().colors().text_muted)
                        .child("TensorZero runs locally and requires no configuration."),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().colors().text_muted)
                        .child("Ensure TensorZero is running on http://localhost:3000"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().colors().text_muted)
                        .child("Configure available models in your settings."),
                ),
        )
    }
}

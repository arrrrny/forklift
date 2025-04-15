use anyhow::{Context as _, Result, anyhow};
use collections::{BTreeMap, HashMap};
use credentials_provider::CredentialsProvider;
use editor::{Editor, EditorElement, EditorStyle};
use futures::Stream;

// The existing futures import should look like:
use futures::{
    future::BoxFuture,
    stream::BoxStream,
    FutureExt,
    StreamExt
};
use gpui::{
    AnyView, AppContext as _, AsyncApp, Entity, FontStyle, Subscription, Task, TextStyle,
    WhiteSpace,
};
use http_client::HttpClient;
use language_model::{
    AuthenticateError, LanguageModel, LanguageModelCompletionEvent, LanguageModelId,
    LanguageModelName, LanguageModelProvider, LanguageModelProviderId, LanguageModelProviderName,
    LanguageModelProviderState, LanguageModelRequest, LanguageModelToolUse, RateLimiter, Role,
    StopReason,
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use settings::{Settings, SettingsStore};
use std::sync::Arc;
use theme::ThemeSettings;
use ui::{Icon, IconName, List, prelude::*};
use util::ResultExt;

use crate::{AllLanguageModelSettings, ui::InstructionListItem};

const PROVIDER_ID: &str = "deepseek";
const PROVIDER_NAME: &str = "DeepSeek";
const DEEPSEEK_API_KEY_VAR: &str = "DEEPSEEK_API_KEY";

#[derive(Default, Clone, Debug, PartialEq)]
pub struct DeepSeekSettings {
    pub api_url: String,
    pub available_models: Vec<AvailableModel>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AvailableModel {
    pub name: String,
    pub display_name: Option<String>,
    pub max_tokens: usize,
    pub max_output_tokens: Option<u32>,
}

pub struct DeepSeekLanguageModelProvider {
    http_client: Arc<dyn HttpClient>,
    state: Entity<State>,
}

pub struct State {
    api_key: Option<String>,
    api_key_from_env: bool,
    _subscription: Subscription,
}

impl State {
    fn is_authenticated(&self) -> bool {
        self.api_key.is_some()
    }

    fn reset_api_key(&self, cx: &mut Context<Self>) -> Task<Result<()>> {
        let credentials_provider = <dyn CredentialsProvider>::global(cx);
        let api_url = AllLanguageModelSettings::get_global(cx)
            .deepseek
            .api_url
            .clone();
        cx.spawn(async move |this, cx| {
            credentials_provider
                .delete_credentials(&api_url, &cx)
                .await
                .log_err();
            this.update(cx, |this, cx| {
                this.api_key = None;
                this.api_key_from_env = false;
                cx.notify();
            })
        })
    }

    fn set_api_key(&mut self, api_key: String, cx: &mut Context<Self>) -> Task<Result<()>> {
        let credentials_provider = <dyn CredentialsProvider>::global(cx);
        let api_url = AllLanguageModelSettings::get_global(cx)
            .deepseek
            .api_url
            .clone();
        cx.spawn(async move |this, cx| {
            credentials_provider
                .write_credentials(&api_url, "Bearer", api_key.as_bytes(), &cx)
                .await?;
            this.update(cx, |this, cx| {
                this.api_key = Some(api_key);
                cx.notify();
            })
        })
    }

    fn authenticate(&self, cx: &mut Context<Self>) -> Task<Result<(), AuthenticateError>> {
        if self.is_authenticated() {
            return Task::ready(Ok(()));
        }

        let credentials_provider = <dyn CredentialsProvider>::global(cx);
        let api_url = AllLanguageModelSettings::get_global(cx)
            .deepseek
            .api_url
            .clone();
        cx.spawn(async move |this, cx| {
            let (api_key, from_env) = if let Ok(api_key) = std::env::var(DEEPSEEK_API_KEY_VAR) {
                (api_key, true)
            } else {
                let (_, api_key) = credentials_provider
                    .read_credentials(&api_url, &cx)
                    .await?
                    .ok_or(AuthenticateError::CredentialsNotFound)?;
                (
                    String::from_utf8(api_key).context("invalid {PROVIDER_NAME} API key")?,
                    false,
                )
            };

            this.update(cx, |this, cx| {
                this.api_key = Some(api_key);
                this.api_key_from_env = from_env;
                cx.notify();
            })?;

            Ok(())
        })
    }
}

impl DeepSeekLanguageModelProvider {
    pub fn new(http_client: Arc<dyn HttpClient>, cx: &mut App) -> Self {
        let state = cx.new(|cx| State {
            api_key: None,
            api_key_from_env: false,
            _subscription: cx.observe_global::<SettingsStore>(|_this: &mut State, cx| {
                cx.notify();
            }),
        });

        Self { http_client, state }
    }
}

impl LanguageModelProviderState for DeepSeekLanguageModelProvider {
    type ObservableEntity = State;

    fn observable_entity(&self) -> Option<Entity<Self::ObservableEntity>> {
        Some(self.state.clone())
    }
}

impl LanguageModelProvider for DeepSeekLanguageModelProvider {
    fn id(&self) -> LanguageModelProviderId {
        LanguageModelProviderId(PROVIDER_ID.into())
    }

    fn name(&self) -> LanguageModelProviderName {
        LanguageModelProviderName(PROVIDER_NAME.into())
    }

    fn icon(&self) -> IconName {
        IconName::AiDeepSeek
    }

    fn default_model(&self, _cx: &App) -> Option<Arc<dyn LanguageModel>> {
        let model = deepseek::Model::Chat;
        Some(Arc::new(DeepSeekLanguageModel {
            id: LanguageModelId::from(model.id().to_string()),
            model,
            state: self.state.clone(),
            http_client: self.http_client.clone(),
            request_limiter: RateLimiter::new(4),
        }))
    }

    fn provided_models(&self, cx: &App) -> Vec<Arc<dyn LanguageModel>> {
        let mut models = BTreeMap::default();

        models.insert("deepseek-chat", deepseek::Model::Chat);
        models.insert("deepseek-reasoner", deepseek::Model::Reasoner);

        for available_model in AllLanguageModelSettings::get_global(cx)
            .deepseek
            .available_models
            .iter()
        {
            models.insert(
                &available_model.name,
                deepseek::Model::Custom {
                    name: available_model.name.clone(),
                    display_name: available_model.display_name.clone(),
                    max_tokens: available_model.max_tokens,
                    max_output_tokens: available_model.max_output_tokens,
                },
            );
        }

        models
            .into_values()
            .map(|model| {
                Arc::new(DeepSeekLanguageModel {
                    id: LanguageModelId::from(model.id().to_string()),
                    model,
                    state: self.state.clone(),
                    http_client: self.http_client.clone(),
                    request_limiter: RateLimiter::new(4),
                }) as Arc<dyn LanguageModel>
            })
            .collect()
    }

    fn is_authenticated(&self, cx: &App) -> bool {
        self.state.read(cx).is_authenticated()
    }

    fn authenticate(&self, cx: &mut App) -> Task<Result<(), AuthenticateError>> {
        self.state.update(cx, |state, cx| state.authenticate(cx))
    }

    fn configuration_view(&self, window: &mut Window, cx: &mut App) -> AnyView {
        cx.new(|cx| ConfigurationView::new(self.state.clone(), window, cx))
            .into()
    }

    fn reset_credentials(&self, cx: &mut App) -> Task<Result<()>> {
        self.state.update(cx, |state, cx| state.reset_api_key(cx))
    }
}

pub struct DeepSeekLanguageModel {
    id: LanguageModelId,
    model: deepseek::Model,
    state: Entity<State>,
    http_client: Arc<dyn HttpClient>,
    request_limiter: RateLimiter,
}

impl DeepSeekLanguageModel {
    fn stream_completion_internal(
        &self,
        request: deepseek::Request,
        cx: &AsyncApp,
    ) -> BoxFuture<'static, Result<BoxStream<'static, Result<deepseek::StreamResponse>>>> {
        let http_client = self.http_client.clone();
        let Ok((api_key, api_url)) = cx.read_entity(&self.state, |state, cx| {
            let settings = &AllLanguageModelSettings::get_global(cx).deepseek;
            (state.api_key.clone(), settings.api_url.clone())
        }) else {
            return futures::future::ready(Err(anyhow!("App state dropped"))).boxed();
        };

        let future = self.request_limiter.stream(async move {
            let api_key = api_key.ok_or_else(|| anyhow!("Missing DeepSeek API Key"))?;
            let request =
                deepseek::stream_completion(http_client.as_ref(), &api_url, &api_key, request);
            let response = request.await?;
            Ok(response)
        });

        async move { Ok(future.await?.boxed()) }.boxed()
    }
}

fn map_deepseek_to_events(
    stream: BoxStream<'static, Result<deepseek::StreamResponse>>,
) -> impl Stream<Item = Result<LanguageModelCompletionEvent>> {
    #[derive(Default)]
    struct RawToolCall {
        id: String,
        name: String,
        arguments: String,
    }

    struct State {
        stream: BoxStream<'static, Result<deepseek::StreamResponse>>,
        tool_calls_by_index: HashMap<usize, RawToolCall>,
        // Track incomplete tool calls to handle streaming better
        incomplete_tool_processing: bool,
    }

    futures::stream::unfold(
        State {
            stream,
            tool_calls_by_index: HashMap::default(),
            incomplete_tool_processing: false,
        },
        |mut state| async move {
            while let Some(response) = state.stream.next().await {
                match response {
                    Ok(response) => {
                        let mut events = Vec::new();
                        for choice in response.choices {
                            let delta = choice.delta;
                            let finish_reason = choice.finish_reason;

                            // Process tool calls with higher priority than content
                            // This helps ensure tools are processed completely
                            if let Some(tool_calls) = delta.tool_calls {
                                state.incomplete_tool_processing = true;
                                
                                for tool_call in tool_calls {
                                    let index = tool_call.index;
                                    let entry = state
                                        .tool_calls_by_index
                                        .entry(index)
                                        .or_default();

                                    if let Some(id) = tool_call.id {
                                        entry.id = id;
                                    }

                                    if let Some(function) = tool_call.function {
                                        if let Some(name) = function.name {
                                            // Clone the name before using it in the log message
                                            let name_for_log = name.clone();
                                            entry.name = name;
                                            
                                            // Log that we're processing a tool call to help with debugging
                                            log::info!("DeepSeek processing tool call: {}", name_for_log);
                                        }
                                        if let Some(arguments) = function.arguments {
                                            entry.arguments.push_str(&arguments);
                                        }
                                    }
                                }
                                
                                // If we get a finish reason and are in tool processing mode,
                                // make sure to emit the tool events even without an explicit tool_calls finish reason
                                if let Some(reason) = finish_reason.clone() {
                                    if reason == "stop" && state.incomplete_tool_processing && !state.tool_calls_by_index.is_empty() {
                                        // Override the finish reason to handle cases where the model doesn't correctly 
                                        // signal tool_calls completion
                                        log::info!("DeepSeek overriding finish reason from 'stop' to 'tool_calls' due to incomplete tool processing");
                                        
                                        // Emit tool use events and then proceed with normal handling
                                        let tool_events = state
                                            .tool_calls_by_index
                                            .drain()
                                            .map(|(_, tool_call)| {
                                                match serde_json::from_str(&tool_call.arguments) {
                                                    Ok(input) => {
                                                        let name_clone = tool_call.name.clone();
                                                        log::info!("DeepSeek emitting tool use event for: {}", name_clone);
                                                        Ok(LanguageModelCompletionEvent::ToolUse(
                                                            LanguageModelToolUse {
                                                                id: tool_call.id.into(),
                                                                name: tool_call.name.into(),
                                                                input,
                                                            }
                                                        ))
                                                    },
                                                    Err(e) => {
                                                        log::error!("Failed to parse tool arguments: {} - Arguments: {}", e, tool_call.arguments);
                                                        Err(anyhow::anyhow!("Failed to parse tool call arguments: {}", e))
                                                    },
                                                }
                                            })
                                            .collect::<Vec<_>>();
                                            
                                        events.extend(tool_events);
                                        events.push(Ok(LanguageModelCompletionEvent::Stop(
                                            StopReason::ToolUse,
                                        )));
                                        
                                        state.incomplete_tool_processing = false;
                                        return Some((events, state));
                                    }
                                }
                            }

                            // Process text content after handling tools
                            if let Some(content) = delta.content {
                                events.push(Ok(LanguageModelCompletionEvent::Text(content)));
                            }

                            if let Some(reason) = finish_reason {
                                match reason.as_str() {
                                    "stop" => {
                                        events.push(Ok(LanguageModelCompletionEvent::Stop(
                                            StopReason::EndTurn,
                                        )));
                                    }
                                    "tool_calls" => {
                                        let tool_events = state
                                            .tool_calls_by_index
                                            .drain()
                                            .map(|(_, tool_call)| {
                                                match serde_json::from_str(&tool_call.arguments) {
                                                    Ok(input) => {
                                                        let name_clone = tool_call.name.clone();
                                                        log::info!("DeepSeek emitting tool use event for: {}", name_clone);
                                                        Ok(LanguageModelCompletionEvent::ToolUse(
                                                            LanguageModelToolUse {
                                                                id: tool_call.id.into(),
                                                                name: tool_call.name.into(),
                                                                input,
                                                            }
                                                        ))
                                                    },
                                                    Err(e) => {
                                                        log::error!("Failed to parse tool arguments: {} - Arguments: {}", e, tool_call.arguments);
                                                        Err(anyhow::anyhow!("Failed to parse tool call arguments: {}", e))
                                                    },
                                                }
                                            })
                                            .collect::<Vec<_>>();
                                        events.extend(tool_events);
                                        events.push(Ok(LanguageModelCompletionEvent::Stop(
                                            StopReason::ToolUse,
                                        )));
                                        state.incomplete_tool_processing = false;
                                    }
                                    _ => {
                                        log::error!("Unexpected finish reason: {}", reason);
                                        events.push(Ok(LanguageModelCompletionEvent::Stop(
                                            StopReason::EndTurn,
                                        )));
                                    }
                                }
                            }
                        }
                        return Some((events, state));
                    }
                    Err(err) => return Some((vec![Err(err)], state)),
                }
            }
            None
        },
    )
    .flat_map(futures::stream::iter)
}

impl LanguageModel for DeepSeekLanguageModel {
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
        matches!(
            self.model,
            deepseek::Model::Chat | deepseek::Model::Reasoner
        )
    }

    fn telemetry_id(&self) -> String {
        format!("deepseek/{}", self.model.id())
    }

    fn max_token_count(&self) -> usize {
        self.model.max_token_count()
    }

    fn max_output_tokens(&self) -> Option<u32> {
        self.model.max_output_tokens()
    }

    fn count_tokens(
        &self,
        request: LanguageModelRequest,
        cx: &App,
    ) -> BoxFuture<'static, Result<usize>> {
        cx.background_spawn(async move {
            let messages = request
                .messages
                .into_iter()
                .map(|message| tiktoken_rs::ChatCompletionRequestMessage {
                    role: match message.role {
                        Role::User => "user".into(),
                        Role::Assistant => "assistant".into(),
                        Role::System => "system".into(),
                    },
                    content: Some(message.string_contents()),
                    name: None,
                    function_call: None,
                })
                .collect::<Vec<_>>();

            tiktoken_rs::num_tokens_from_messages("gpt-4", &messages)
        })
        .boxed()
    }

    fn stream_completion(
        &self,
        request: LanguageModelRequest,
        cx: &AsyncApp,
    ) -> BoxFuture<'static, Result<BoxStream<'static, Result<LanguageModelCompletionEvent>>>> {
        let deepseek_request = into_deepseek(
            request,
            self.model.id().to_string(),
            self.max_output_tokens(),
        );
        let stream = self.stream_completion_internal(deepseek_request, cx);

        async move {
            let stream = stream.await?;
            Ok(map_deepseek_to_events(stream).boxed())
        }
        .boxed()
    }
}

pub fn into_deepseek(
    request: LanguageModelRequest,
    model: String,
    max_output_tokens: Option<u32>,
) -> deepseek::Request {
    let is_reasoner = model == "deepseek-reasoner";
    let is_chat = model == "deepseek-chat";
    let custom_model = !is_reasoner && !is_chat;
    let len = request.messages.len();
    
    // Determine if there are any tools in the request
    let has_tools = !request.tools.is_empty();
    
    // Enhanced message conversion with tool history handling
    let merged_messages = request.messages.into_iter().fold(Vec::with_capacity(len), |mut acc, msg| {
        let role = msg.role;
        let content = msg.string_contents();
        
        // Extract any tool-related messages for proper conversion
        let mut tool_calls = Vec::new();
        
        for content_item in &msg.content {
            if let language_model::MessageContent::ToolUse(tool_use) = content_item {
                // Build proper tool call representation
                let arguments = match serde_json::to_string(&tool_use.input) {
                    Ok(args) => args,
                    Err(e) => {
                        log::error!("Failed to serialize tool arguments: {}", e);
                        "{}".to_string()
                    }
                };
                
                tool_calls.push(deepseek::ToolCall {
                    id: tool_use.id.to_string(),
                    content: deepseek::ToolCallContent::Function {
                        function: deepseek::FunctionContent {
                            name: tool_use.name.to_string(),
                            arguments,
                        },
                    },
                });
            }
        }
        
        // Special handling for reasoner model
        if is_reasoner {
            if let Some(last_msg) = acc.last_mut() {
                match (last_msg, role) {
                    (deepseek::RequestMessage::User { content: last }, Role::User) => {
                        last.push(' ');
                        last.push_str(&content);
                        return acc;
                    }
                    (deepseek::RequestMessage::Assistant { content: last_content, .. }, Role::Assistant) => {
                        *last_content = last_content.take().map(|c| {
                            let mut s = String::with_capacity(c.len() + content.len() + 1);
                            s.push_str(&c);
                            s.push(' ');
                            s.push_str(&content);
                            s
                        }).or(Some(content));
                        return acc;
                    }
                    _ => {}
                }
            }
        }
        
        // Convert messages with enhanced tool call handling
        match role {
            Role::User => {
                acc.push(deepseek::RequestMessage::User { content });
            },
            Role::Assistant => {
                // Only include non-empty tool_calls
                acc.push(deepseek::RequestMessage::Assistant {
                    content: if content.is_empty() { None } else { Some(content) },
                    tool_calls,
                });
            },
            Role::System => {
                // For models like Chat or custom models, ensure clear instructions about tools
                if (is_chat || custom_model) && has_tools {
                    // Add tool usage instructions to system messages
                    let enhanced_content = if content.contains("tool") || content.contains("function") {
                        // If the system message already mentions tools, keep it
                        content
                    } else {
                        // Otherwise, add specific instructions about tools
                        format!("{} You have access to tools/functions that you should use when appropriate. \
                        When a user's request requires using a tool, call the appropriate tool instead of \
                        generating fake results or refusing to use the tool. Analyze the request carefully \
                        and determine whether a tool would help answer it properly.", content)
                    };
                    acc.push(deepseek::RequestMessage::System { content: enhanced_content });
                } else {
                    acc.push(deepseek::RequestMessage::System { content });
                }
            },
        };
        
        acc
    });
    
    // Clone tools for use in the request to avoid ownership issues
    let tools_clone = request.tools.clone();
    
    // Create proper tool definitions with enhanced descriptions
    let tools = if is_chat || custom_model {
        tools_clone.into_iter().map(|tool| {
            // Create more descriptive tool definitions
            let enhanced_description = if tool.description.contains("parameters") || tool.description.contains("argument") {
                tool.description
            } else {
                format!("{} Use this tool by providing the required parameters in the correct format.", 
                        tool.description)
            };
            
            deepseek::ToolDefinition::Function {
                function: deepseek::FunctionDefinition {
                    name: tool.name,
                    description: Some(enhanced_description),
                    parameters: Some(tool.input_schema),
                },
            }
        }).collect()
    } else {
        Vec::new()
    };
    
    // Create the DeepSeek request with appropriate settings based on model type
    deepseek::Request {
        model,
        messages: merged_messages,
        stream: true,
        max_tokens: max_output_tokens,
        temperature: if is_chat {
            // Lower temperature for tool usage to make it more precise
            if has_tools {
                Some(0.2) 
            } else {
                Some(0.0)
            }
        } else if is_reasoner {
            None
        } else {
            // For custom models with tools, adjust temperature
            if custom_model && has_tools {
                Some(request.temperature.unwrap_or(0.2))
            } else {
                request.temperature
            }
        },
        response_format: None,
        tools,
    }
}

struct ConfigurationView {
    api_key_editor: Entity<Editor>,
    state: Entity<State>,
    load_credentials_task: Option<Task<()>>,
}

impl ConfigurationView {
    fn new(state: Entity<State>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let api_key_editor = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text("sk-00000000000000000000000000000000", cx);
            editor
        });

        cx.observe(&state, |_, _, cx| {
            cx.notify();
        })
        .detach();

        let load_credentials_task = Some(cx.spawn({
            let state = state.clone();
            async move |this, cx| {
                if let Some(task) = state
                    .update(cx, |state, cx| state.authenticate(cx))
                    .log_err()
                {
                    let _ = task.await;
                }

                this.update(cx, |this, cx| {
                    this.load_credentials_task = None;
                    cx.notify();
                })
                .log_err();
            }
        }));

        Self {
            api_key_editor,
            state,
            load_credentials_task,
        }
    }

    fn save_api_key(&mut self, _: &menu::Confirm, _window: &mut Window, cx: &mut Context<Self>) {
        let api_key = self.api_key_editor.read(cx).text(cx);
        if api_key.is_empty() {
            return;
        }

        let state = self.state.clone();
        cx.spawn(async move |_, cx| {
            state
                .update(cx, |state, cx| state.set_api_key(api_key, cx))?
                .await
        })
        .detach_and_log_err(cx);

        cx.notify();
    }

    fn reset_api_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.api_key_editor
            .update(cx, |editor, cx| editor.set_text("", window, cx));

        let state = self.state.clone();
        cx.spawn(async move |_, cx| state.update(cx, |state, cx| state.reset_api_key(cx))?.await)
            .detach_and_log_err(cx);

        cx.notify();
    }

    fn render_api_key_editor(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = ThemeSettings::get_global(cx);
        let text_style = TextStyle {
            color: cx.theme().colors().text,
            font_family: settings.ui_font.family.clone(),
            font_features: settings.ui_font.features.clone(),
            font_fallbacks: settings.ui_font.fallbacks.clone(),
            font_size: rems(0.875).into(),
            font_weight: settings.ui_font.weight,
            font_style: FontStyle::Normal,
            line_height: relative(1.3),
            background_color: None,
            underline: None,
            strikethrough: None,
            white_space: WhiteSpace::Normal,
            ..Default::default()
        };
        EditorElement::new(
            &self.api_key_editor,
            EditorStyle {
                background: cx.theme().colors().editor_background,
                local_player: cx.theme().players().local(),
                text: text_style,
                ..Default::default()
            },
        )
    }

    fn should_render_editor(&self, cx: &mut Context<Self>) -> bool {
        !self.state.read(cx).is_authenticated()
    }
}

impl Render for ConfigurationView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let env_var_set = self.state.read(cx).api_key_from_env;

        if self.load_credentials_task.is_some() {
            div().child(Label::new("Loading credentials...")).into_any()
        } else if self.should_render_editor(cx) {
            v_flex()
                .size_full()
                .on_action(cx.listener(Self::save_api_key))
                .child(Label::new("To use DeepSeek in Zed, you need an API key:"))
                .child(
                    List::new()
                        .child(InstructionListItem::new(
                            "Get your API key from the",
                            Some("DeepSeek console"),
                            Some("https://platform.deepseek.com/api_keys"),
                        ))
                        .child(InstructionListItem::text_only(
                            "Paste your API key below and hit enter to start using the assistant",
                        )),
                )
                .child(
                    h_flex()
                        .w_full()
                        .my_2()
                        .px_2()
                        .py_1()
                        .bg(cx.theme().colors().editor_background)
                        .border_1()
                        .border_color(cx.theme().colors().border_variant)
                        .rounded_sm()
                        .child(self.render_api_key_editor(cx)),
                )
                .child(
                    Label::new(format!(
                        "Or set the {} environment variable.",
                        DEEPSEEK_API_KEY_VAR
                    ))
                    .size(LabelSize::Small)
                    .color(Color::Muted),
                )
                .into_any()
        } else {
            h_flex()
                .size_full()
                .justify_between()
                .child(
                    h_flex()
                        .gap_1()
                        .child(Icon::new(IconName::Check).color(Color::Success))
                        .child(Label::new(if env_var_set {
                            format!("API key set in {}", DEEPSEEK_API_KEY_VAR)
                        } else {
                            "API key configured".to_string()
                        })),
                )
                .child(
                    Button::new("reset-key", "Reset")
                        .icon(IconName::Trash)
                        .disabled(env_var_set)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.reset_api_key(window, cx)),
                        ),
                )
                .into_any()
        }
    }
}

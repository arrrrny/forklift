use anyhow::{Context as _, Result, anyhow};
use collections::HashMap;
use credentials_provider::CredentialsProvider;
use editor::{Editor, EditorElement, EditorStyle};
use futures::{FutureExt, Stream, StreamExt, future::BoxFuture};
use gpui::{
    AnyView, App, AsyncApp, Context, Entity, FontStyle, Subscription, Task, TextStyle, WhiteSpace,
};
use http_client::HttpClient;
use language_model::{
    AuthenticateError, LanguageModel, LanguageModelCompletionError, LanguageModelCompletionEvent,
    LanguageModelId, LanguageModelName, LanguageModelProvider, LanguageModelProviderId,
    LanguageModelProviderName, LanguageModelProviderState, LanguageModelRequest,
    LanguageModelToolChoice, LanguageModelToolResultContent, LanguageModelToolUse, MessageContent,
    RateLimiter, Role, StopReason,
};
use litellm::{Model, ResponseStreamEvent, list_models, stream_completion};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use settings::{Settings, SettingsStore};
use std::pin::Pin;
use std::str::FromStr as _;
use std::sync::Arc;
use theme::ThemeSettings;
use ui::{Icon, IconName, List, Tooltip, prelude::*};
use util::ResultExt;

use crate::{AllLanguageModelSettings, ui::InstructionListItem};

const PROVIDER_ID: &str = "litellm";
const PROVIDER_NAME: &str = "LiteLLM";

#[derive(Clone, Debug, PartialEq)]
pub struct LiteLLMSettings {
    pub api_url: String,
    pub available_models: Vec<AvailableModel>,
}

impl Default for LiteLLMSettings {
    fn default() -> Self {
        Self {
            api_url: litellm::LITELLM_API_URL.to_string(),
            available_models: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AvailableModel {
    pub name: String,
    pub display_name: Option<String>,
    pub max_tokens: Option<usize>,
    pub max_output_tokens: Option<u32>,
    pub max_completion_tokens: Option<u32>,
    #[serde(default)]
    pub supports_tools: Option<bool>,
    #[serde(default)]
    pub recommended: Option<bool>,
}

pub struct LiteLLMLanguageModelProvider {
    http_client: Arc<dyn HttpClient>,
    state: gpui::Entity<State>,
}

pub struct State {
    api_key: Option<String>,
    api_key_from_env: bool,
    http_client: Arc<dyn HttpClient>,
    available_models: Vec<litellm::Model>,
    fetch_models_task: Option<Task<Result<()>>>,
    _subscription: Subscription,
}

const LITELLM_API_KEY_VAR: &str = "LITELLM_API_KEY";

impl State {
    fn is_authenticated(&self) -> bool {
        self.api_key.is_some()
    }

    fn reset_api_key(&self, cx: &mut Context<Self>) -> Task<Result<()>> {
        let credentials_provider = <dyn CredentialsProvider>::global(cx);
        let api_url = AllLanguageModelSettings::get_global(cx)
            .litellm
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
            .litellm
            .api_url
            .clone();
        cx.spawn(async move |this, cx| {
            credentials_provider
                .write_credentials(&api_url, "Bearer", api_key.as_bytes(), &cx)
                .await
                .log_err();
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
            .litellm
            .api_url
            .clone();

        cx.spawn(async move |this, cx| {
            let (api_key, from_env) = if let Ok(api_key) = std::env::var(LITELLM_API_KEY_VAR) {
                (api_key, true)
            } else {
                let (_, api_key) = credentials_provider
                    .read_credentials(&api_url, &cx)
                    .await?
                    .ok_or(AuthenticateError::CredentialsNotFound)?;
                (
                    String::from_utf8(api_key)
                        .context(format!("invalid {} API key", PROVIDER_NAME))?,
                    false,
                )
            };
            this.update(cx, |this, cx| {
                this.api_key = Some(api_key);
                this.api_key_from_env = from_env;
                this.restart_fetch_models_task(cx);
                cx.notify();
            })?;
            Ok(())
        })
    }

    fn fetch_models(&mut self, cx: &mut Context<Self>) -> Task<Result<()>> {
        let settings = &AllLanguageModelSettings::get_global(cx).litellm;
        let http_client = self.http_client.clone();
        let api_url = settings.api_url.clone();

        cx.spawn(async move |this, cx| {
            let models = list_models(http_client.as_ref(), &api_url).await?;

            this.update(cx, |this, cx| {
                this.available_models = models;
                cx.notify();
            })
        })
    }

    fn restart_fetch_models_task(&mut self, cx: &mut Context<Self>) {
        let task = self.fetch_models(cx);
        self.fetch_models_task.replace(task);
    }
}

impl LiteLLMLanguageModelProvider {
    pub fn new(http_client: Arc<dyn HttpClient>, cx: &mut App) -> Self {
        let state = cx.new(|cx| State {
            api_key: None,
            api_key_from_env: false,
            http_client: http_client.clone(),
            available_models: Vec::new(),
            fetch_models_task: None,
            _subscription: cx.observe_global::<SettingsStore>(|this: &mut State, cx| {
                this.restart_fetch_models_task(cx);
                cx.notify();
            }),
        });

        Self { http_client, state }
    }

    fn create_language_model(&self, model: litellm::Model) -> Arc<dyn LanguageModel> {
        Arc::new(LiteLLMLanguageModel {
            id: LanguageModelId::from(model.id().to_string()),
            model,
            state: self.state.clone(),
            http_client: self.http_client.clone(),
            request_limiter: RateLimiter::new(4),
        })
    }
}

impl LanguageModelProviderState for LiteLLMLanguageModelProvider {
    type ObservableEntity = State;

    fn observable_entity(&self) -> Option<gpui::Entity<Self::ObservableEntity>> {
        Some(self.state.clone())
    }
}

impl LanguageModelProvider for LiteLLMLanguageModelProvider {
    fn id(&self) -> LanguageModelProviderId {
        LanguageModelProviderId(PROVIDER_ID.into())
    }

    fn name(&self) -> LanguageModelProviderName {
        LanguageModelProviderName(PROVIDER_NAME.into())
    }

    fn icon(&self) -> IconName {
        IconName::AiLiteLLM
    }

    fn default_model(&self, _cx: &App) -> Option<Arc<dyn LanguageModel>> {
        Some(self.create_language_model(litellm::Model::default()))
    }

    fn default_fast_model(&self, _cx: &App) -> Option<Arc<dyn LanguageModel>> {
        Some(self.create_language_model(litellm::Model::default_fast()))
    }

    fn provided_models(&self, cx: &App) -> Vec<Arc<dyn LanguageModel>> {
        let mut settings_models = Vec::new();

        for model in &AllLanguageModelSettings::get_global(cx)
            .litellm
            .available_models
        {
            settings_models.push(litellm::Model {
                name: model.name.clone(),
                display_name: model.display_name.clone(),
                max_tokens: model.max_tokens.unwrap_or(2000000),
                supports_tools: model.supports_tools,
            });
        }

        let fetched_models = self.state.read(cx).available_models.clone();
        let mut models = Vec::new();

        for model in settings_models.into_iter().chain(fetched_models) {
            let duplicate = models
                .iter()
                .any(|m: &Arc<dyn LanguageModel>| m.id().0 == model.id());

            if !duplicate {
                models.push(self.create_language_model(model));
            }
        }

        models
    }

    fn recommended_models(&self, cx: &App) -> Vec<Arc<dyn LanguageModel>> {
        let settings = &AllLanguageModelSettings::get_global(cx).litellm;
        let fetched_models = self.state.read(cx).available_models.clone();

        // Get recommended models from settings
        let mut recommended = vec![];

        for settings_model in &settings.available_models {
            if settings_model.recommended.unwrap_or(false) {
                if let Some(fetched_model) = fetched_models
                    .iter()
                    .find(|m| m.id() == settings_model.name)
                {
                    recommended.push(self.create_language_model(fetched_model.clone()));
                } else {
                    // Model not found in fetched data, create from settings
                    recommended.push(self.create_language_model(litellm::Model {
                        name: settings_model.name.clone(),
                        display_name: settings_model.display_name.clone(),
                        max_tokens: settings_model.max_tokens.unwrap_or(2000000),
                        supports_tools: settings_model.supports_tools,
                    }));
                }
            }
        }

        recommended
    }

    fn is_authenticated(&self, cx: &App) -> bool {
        self.state.read(cx).is_authenticated()
    }

    fn authenticate(&self, cx: &mut App) -> Task<Result<(), AuthenticateError>> {
        self.state.update(cx, |state, cx| state.authenticate(cx))
    }

    fn configuration_view(&self, window: &mut gpui::Window, cx: &mut App) -> AnyView {
        cx.new(|cx| ConfigurationView::new(self.state.clone(), window, cx))
            .into()
    }

    fn reset_credentials(&self, cx: &mut App) -> Task<Result<()>> {
        self.state.update(cx, |state, cx| state.reset_api_key(cx))
    }
}

pub struct LiteLLMLanguageModel {
    id: LanguageModelId,
    model: litellm::Model,
    state: gpui::Entity<State>,
    http_client: Arc<dyn HttpClient>,
    request_limiter: RateLimiter,
}

impl LiteLLMLanguageModel {
    fn stream_completion(
        &self,
        request: litellm::Request,
        cx: &AsyncApp,
    ) -> BoxFuture<'static, Result<futures::stream::BoxStream<'static, Result<ResponseStreamEvent>>>>
    {
        let http_client = self.http_client.clone();

        let Ok((api_key, api_url)) = cx.read_entity(&self.state, |state, cx| {
            let settings = &AllLanguageModelSettings::get_global(cx).litellm;
            (state.api_key.clone(), settings.api_url.clone())
        }) else {
            return async move { Err(anyhow!("Failed to read state")) }.boxed();
        };

        let future = self.request_limiter.stream(async move {
            let api_key = api_key.ok_or_else(|| anyhow!("Missing LiteLLM API Key"))?;
            let request = stream_completion(http_client.as_ref(), &api_url, &api_key, request);
            let response = request.await?;
            Ok(response)
        });

        future.boxed()
    }
}

impl LanguageModel for LiteLLMLanguageModel {
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
        format!("litellm/{}", self.model.id())
    }

    fn max_token_count(&self) -> usize {
        self.model.max_token_count()
    }

    fn max_output_tokens(&self) -> Option<u32> {
        self.model.max_output_tokens()
    }

    fn supports_tool_choice(&self) -> bool {
        match self.model.id() {
            _ => true,
        }
    }

    fn supports_images(&self) -> bool {
        false
    }

    fn count_tokens(
        &self,
        request: LanguageModelRequest,
        cx: &App,
    ) -> BoxFuture<'static, Result<usize>> {
        count_litellm_tokens(request, self.model.clone(), cx)
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
        let request = into_litellm(request, &self.model, self.max_output_tokens());
        let completions = self.stream_completion(request, cx);
        async move {
            let mapper = LiteLLMEventMapper::new();
            Ok(mapper.map_stream(completions.await?).boxed())
        }
        .boxed()
    }
}

pub fn into_litellm(
    request: LanguageModelRequest,
    model: &Model,
    max_output_tokens: Option<u32>,
) -> litellm::Request {
    let mut messages = Vec::new();
    for req_message in request.messages {
        for content in req_message.content {
            match content {
                MessageContent::Text(text) | MessageContent::Thinking { text, .. } => messages
                    .push(match req_message.role {
                        Role::User => litellm::RequestMessage::User { content: text },
                        Role::Assistant => litellm::RequestMessage::Assistant {
                            content: Some(text),
                            tool_calls: Vec::new(),
                        },
                        Role::System => litellm::RequestMessage::System { content: text },
                    }),
                MessageContent::RedactedThinking(_) => {}
                MessageContent::Image(_) => {}
                MessageContent::ToolUse(tool_use) => {
                    let tool_call = litellm::ToolCall {
                        id: tool_use.id.to_string(),
                        content: litellm::ToolCallContent::Function {
                            function: litellm::FunctionContent {
                                name: tool_use.name.to_string(),
                                arguments: serde_json::to_string(&tool_use.input)
                                    .unwrap_or_default(),
                            },
                        },
                    };

                    if let Some(litellm::RequestMessage::Assistant { tool_calls, .. }) =
                        messages.last_mut()
                    {
                        tool_calls.push(tool_call);
                    } else {
                        messages.push(litellm::RequestMessage::Assistant {
                            content: None,
                            tool_calls: vec![tool_call],
                        });
                    }
                }
                MessageContent::ToolResult(tool_result) => {
                    let content = match &tool_result.content {
                        LanguageModelToolResultContent::Text(text) => {
                          text.to_string()
                        }
                        LanguageModelToolResultContent::Image(_) => {
                          "[Tool responded with an image, but Zed doesn't support these in LiteLLM models yet]".to_string()
                        }
                    };

                    messages.push(litellm::RequestMessage::Tool {
                        content,
                        tool_call_id: tool_result.tool_use_id.to_string(),
                    });
                }
            }
        }
    }

    litellm::Request {
        model: model.id().into(),
        messages,
        stream: true,
        max_tokens: max_output_tokens,
        stop: request.stop,
        temperature: request.temperature,
        tools: request
            .tools
            .into_iter()
            .map(|tool| litellm::ToolDefinition::Function {
                function: litellm::FunctionDefinition {
                    name: tool.name,
                    description: Some(tool.description),
                    parameters: Some(tool.input_schema),
                },
            })
            .collect(),
        tool_choice: request.tool_choice.map(|choice| match choice {
            LanguageModelToolChoice::Auto => litellm::ToolChoice::Auto,
            LanguageModelToolChoice::Any => litellm::ToolChoice::Required,
            LanguageModelToolChoice::None => litellm::ToolChoice::None,
        }),
        parallel_tool_calls: Some(false),
    }
}

pub struct LiteLLMEventMapper {
    tool_calls_by_index: HashMap<usize, RawToolCall>,
}

impl LiteLLMEventMapper {
    pub fn new() -> Self {
        Self {
            tool_calls_by_index: HashMap::default(),
        }
    }

    pub fn map_stream(
        mut self,
        events: Pin<Box<dyn Send + Stream<Item = Result<ResponseStreamEvent>>>>,
    ) -> impl Stream<Item = Result<LanguageModelCompletionEvent, LanguageModelCompletionError>>
    {
        events.map(move |event| self.map_event(event))
    }

    pub fn map_event(
        &mut self,
        event: Result<ResponseStreamEvent>,
    ) -> Result<LanguageModelCompletionEvent, LanguageModelCompletionError> {
        let event = event.map_err(|error| LanguageModelCompletionError::other(error))?;

        if event.choices.is_empty() {
            return Err(LanguageModelCompletionError::other(anyhow!(
                "LiteLLM response is missing choices"
            )));
        }

        let choice = &event.choices[0];

        if let Some(content) = choice.delta.content.as_ref() {
            return Ok(LanguageModelCompletionEvent::Text(content.clone()));
        }

        if let Some(tool_calls) = choice.delta.tool_calls.as_ref() {
            for tool_call_chunk in tool_calls {
                let index = tool_call_chunk.index;
                let raw_tool_call =
                    self.tool_calls_by_index
                        .entry(index)
                        .or_insert_with(|| RawToolCall {
                            id: String::new(),
                            name: String::new(),
                            arguments: String::new(),
                        });

                if let Some(id) = &tool_call_chunk.id {
                    raw_tool_call.id = id.clone();
                }

                if let Some(function) = &tool_call_chunk.function {
                    if let Some(name) = &function.name {
                        raw_tool_call.name.push_str(name);
                    }
                    if let Some(arguments) = &function.arguments {
                        raw_tool_call.arguments.push_str(arguments);
                    }
                }
            }
        }

        if let Some(finish_reason) = choice.finish_reason.as_ref() {
            match finish_reason.as_str() {
                "stop" => return Ok(LanguageModelCompletionEvent::Stop(StopReason::EndTurn)),
                "length" => return Ok(LanguageModelCompletionEvent::Stop(StopReason::MaxTokens)),
                "tool_calls" => {
                    for (_, raw_tool_call) in self.tool_calls_by_index.drain() {
                        let input = serde_json::from_str(&raw_tool_call.arguments)
                            .unwrap_or_else(|_| serde_json::Value::Null);
                        return Ok(LanguageModelCompletionEvent::ToolUse(
                            LanguageModelToolUse {
                                id: raw_tool_call.id.into(),
                                name: raw_tool_call.name.into(),
                                is_input_complete: true,
                                raw_input: raw_tool_call.arguments.clone(),
                                input,
                            },
                        ));
                    }
                    return Ok(LanguageModelCompletionEvent::Stop(StopReason::ToolUse));
                }
                reason => {
                    return Ok(LanguageModelCompletionEvent::Stop(StopReason::Other(
                        reason.to_string(),
                    )));
                }
            }
        }

        Ok(LanguageModelCompletionEvent::Text(String::new()))
    }
}

struct RawToolCall {
    id: String,
    name: String,
    arguments: String,
}

pub fn count_litellm_tokens(
    request: LanguageModelRequest,
    _model: litellm::Model,
    cx: &App,
) -> BoxFuture<'static, Result<usize>> {
    // This is a simplified token counting implementation
    let text = request
        .messages
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|content| match content {
            MessageContent::Text(text) => Some(text.as_str()),
            MessageContent::Thinking { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ");

    let token_count = text.split_whitespace().count();
    async move { Ok(token_count) }.boxed()
}

struct ConfigurationView {
    api_key_editor: Entity<Editor>,
    state: Entity<State>,
    load_credentials_task: Option<Task<()>>,
}

impl ConfigurationView {
    fn new(state: Entity<State>, window: &mut gpui::Window, cx: &mut Context<Self>) -> Self {
        let api_key_editor = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text("sk-000000000000000000000000000000000000000000000000", cx);
            editor
        });

        cx.observe(&state, |_, _cx, cx| {
            cx.notify();
        })
        .detach();

        let load_credentials_task = Some(cx.spawn_in(window, {
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

    fn save_api_key(
        &mut self,
        _: &menu::Confirm,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        let api_key = self.api_key_editor.read(cx).text(cx);
        if api_key.is_empty() {
            return;
        }

        let state = self.state.clone();
        cx.spawn_in(window, async move |_, cx| {
            state
                .update(cx, |state, cx| state.set_api_key(api_key, cx))?
                .await
        })
        .detach_and_log_err(cx);

        cx.notify();
    }

    fn reset_api_key(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        self.api_key_editor
            .update(cx, |editor, cx| editor.set_text("", window, cx));

        let state = self.state.clone();
        cx.spawn_in(window, async move |_, cx| {
            state.update(cx, |state, cx| state.reset_api_key(cx))?.await
        })
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
    fn render(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let env_var_set = self.state.read(cx).api_key_from_env;

        if self.load_credentials_task.is_some() {
            div().child(Label::new("Loading credentials...")).into_any()
        } else if self.should_render_editor(cx) {
            v_flex()
                .size_full()
                .on_action(cx.listener(Self::save_api_key))
                .child(Label::new("To use Zed's assistant with LiteLLM, you need to add an API key. Follow these steps:"))
                .child(
                    List::new()
                        .child(InstructionListItem::new(
                            "Create an API key by visiting",
                            Some("LiteLLM's documentation"),
                            Some("https://docs.litellm.ai/docs/"),
                        ))
                        .child(InstructionListItem::text_only(
                            "Ensure your LiteLLM service is running",
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
                        .border_color(cx.theme().colors().border)
                        .rounded_sm()
                        .child(self.render_api_key_editor(cx)),
                )
                .child(
                    Label::new(
                        format!("You can also assign the {LITELLM_API_KEY_VAR} environment variable and restart Zed."),
                    )
                    .size(LabelSize::Small).color(Color::Muted),
                )
                .into_any()
        } else {
            h_flex()
                .mt_1()
                .p_1()
                .justify_between()
                .rounded_md()
                .border_1()
                .border_color(cx.theme().colors().border)
                .bg(cx.theme().colors().background)
                .child(
                    h_flex()
                        .gap_1()
                        .child(Icon::new(IconName::Check).color(Color::Success))
                        .child(Label::new(if env_var_set {
                            format!("API key set in {LITELLM_API_KEY_VAR} environment variable.")
                        } else {
                            "API key configured.".to_string()
                        })),
                )
                .child(
                    Button::new("reset-key", "Reset Key")
                        .label_size(LabelSize::Small)
                        .icon(Some(IconName::Trash))
                        .icon_size(IconSize::Small)
                        .icon_position(IconPosition::Start)
                        .disabled(env_var_set)
                        .when(env_var_set, |this| {
                            this.tooltip(Tooltip::text(format!("To reset your API key, unset the {LITELLM_API_KEY_VAR} environment variable.")))
                        })
                        .on_click(cx.listener(|this, _, window, cx| this.reset_api_key(window, cx))),
                )
                .into_any()
        }
    }
}

mod model;
pub use model::*;

use crate::{
    AuthenticateError, LanguageModel, LanguageModelCompletionEvent, LanguageModelId,
    LanguageModelName, LanguageModelProvider, LanguageModelProviderId, LanguageModelProviderName,
    LanguageModelProviderState, LanguageModelRequest, LanguageModelToolUse, RateLimiter, Role,
    StopReason,
};

use anyhow::{Context as _, Result, anyhow};
use collections::{BTreeMap, HashMap};
use credentials_provider::CredentialsProvider;
use editor::{Editor, EditorElement, EditorStyle};
use futures::Stream;
use futures::{
    future::BoxFuture,
    stream::BoxStream,
    FutureExt,
    StreamExt
};
use gpui::{
    div, h_flex, v_flex, AnyView, AppContext, Button, Context, EditorStyle, FontStyle, Icon,
    IconName, Label, Model, TextStyle, Tooltip, View, ViewContext, WhiteSpace, WindowContext,
};
use http_client::HttpClient;
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
                    String::from_utf8(api_key).context("invalid DeepSeek API key")?,
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
        let model = Model::Chat;
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

        models.insert("deepseek-chat", Model::Chat);
        models.insert("deepseek-reasoner", Model::Reasoner);

        for available_model in AllLanguageModelSettings::get_global(cx)
            .deepseek
            .available_models
            .iter()
        {
            models.insert(
                &available_model.name,
                Model::Custom {
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
    model: Model,
    state: Entity<State>,
    http_client: Arc<dyn HttpClient>,
    request_limiter: RateLimiter,
}

impl DeepSeekLanguageModel {
    fn stream_completion_internal(
        &self,
        request: Request,
        cx: &AsyncApp,
    ) -> BoxFuture<'static, Result<BoxStream<'static, Result<StreamResponse>>>> {
        let http_client = self.http_client.clone();
        let Ok((api_key, api_url)) = cx.read_entity(&self.state, |state, cx| {
            let settings = &AllLanguageModelSettings::get_global(cx).deepseek;
            (state.api_key.clone(), settings.api_url.clone())
        }) else {
            return futures::future::ready(Err(anyhow!("App state dropped"))).boxed();
        };

        let future = self.request_limiter.stream(async move {
            let api_key = api_key.ok_or_else(|| anyhow!("Missing DeepSeek API Key"))?;
            let request = stream_completion(http_client.as_ref(), &api_url, &api_key, request);
            let response = request.await?;
            Ok(response)
        });

        async move { Ok(future.await?.boxed()) }.boxed()
    }
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
        matches!(self.model, Model::Chat | Model::Reasoner)
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
) -> Request {
    let is_reasoner = model == "deepseek-reasoner";

    let len = request.messages.len();
    let merged_messages =
        request
            .messages
            .into_iter()
            .fold(Vec::with_capacity(len), |mut acc, msg| {
                let role = msg.role;
                let content = msg.string_contents();

                if is_reasoner {
                    if let Some(last_msg) = acc.last_mut() {
                        match (last_msg, role) {
                            (RequestMessage::User { content: last }, Role::User) => {
                                last.push(' ');
                                last.push_str(&content);
                                return acc;
                            }

                            (
                                RequestMessage::Assistant {
                                    content: last_content,
                                    ..
                                },
                                Role::Assistant,
                            ) => {
                                *last_content = last_content
                                    .take()
                                    .map(|c| {
                                        let mut s =
                                            String::with_capacity(c.len() + content.len() + 1);
                                        s.push_str(&c);
                                        s.push(' ');
                                        s.push_str(&content);
                                        s
                                    })
                                    .or(Some(content));

                                return acc;
                            }
                            _ => {}
                        }
                    }
                }

                acc.push(match role {
                    Role::User => RequestMessage::User { content },
                    Role::Assistant => RequestMessage::Assistant {
                        content: Some(content),
                        tool_calls: Vec::new(),
                    },
                    Role::System => RequestMessage::System { content },
                });
                acc
            });

    Request {
        model,
        messages: merged_messages,
        stream: true,
        max_tokens: max_output_tokens,
        temperature: if is_reasoner {
            None
        } else {
            Some(0.0)
        },
        response_format: None,
        tools: request
            .tools
            .into_iter()
            .map(|tool| ToolDefinition::Function {
                function: FunctionDefinition {
                    name: tool.name,
                    description: Some(tool.description),
                    parameters: Some(tool.input_schema),
                },
            })
            .collect(),
    }
}

fn map_deepseek_to_events(
    events: BoxStream<'static, Result<StreamResponse>>,
) -> impl Stream<Item = Result<LanguageModelCompletionEvent>> {
    #[derive(Default)]
    struct RawToolCall {
        id: String,
        name: String,
        arguments: String,
    }

    struct State {
        events: BoxStream<'static, Result<StreamResponse>>,
        tool_calls_by_index: HashMap<usize, RawToolCall>,
    }

    futures::stream::unfold(
        State {
            events,
            tool_calls_by_index: HashMap::default(),
        },
        |mut state| async move {
            while let Some(response) = state.events.next().await {
                match response {
                    Ok(response) => {
                        let mut events = Vec::new();
                        for choice in response.choices {
                            let delta = choice.delta;
                            let finish_reason = choice.finish_reason;

                            if let Some(content) = delta.content {
                                events.push(Ok(LanguageModelCompletionEvent::Text(content)));
                            }

                            if let Some(tool_calls) = delta.tool_calls {
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
                                            entry.name = name;
                                        }
                                        if let Some(arguments) = function.arguments {
                                            entry.arguments.push_str(&arguments);
                                        }
                                    }
                                }
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
                                                    Ok(input) => Ok(LanguageModelCompletionEvent::ToolUse(
                                                        LanguageModelToolUse {
                                                            id: tool_call.id.into(),
                                                            name: tool_call.name.into(),
                                                            input,
                                                        }
                                                    )),
                                                    Err(e) => Err(anyhow!("Failed to parse tool call arguments: {}", e)),
                                                }
                                            })
                                            .collect::<Vec<_>>();
                                        events.extend(tool_events);
                                        events.push(Ok(LanguageModelCompletionEvent::Stop(
                                            StopReason::ToolUse,
                                        )));
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

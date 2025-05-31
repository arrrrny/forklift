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

pub struct LiteLLMSettings {
    pub api_url: String,
    pub available_models: Vec<AvailableModel>,
}

pub struct AvailableModel {
    pub name: String,
    pub display_name: String,
    pub max_tokens: usize,
    pub max_output_tokens: usize,
    pub max_completion_tokens: usize,
    pub supports_tools: bool,
    pub recommended: bool,
}

pub struct LiteLLMLanguageModelProvider {
    http_client: HttpClient,
    state: State,
}

pub struct State {
    api_key: Option<String>,
    api_key_from_env: Option<String>,
    http_client: HttpClient,
    available_models: Vec<AvailableModel>,
    fetch_models_task: Option<Task>,
    _subscription: Option<Subscription>,
}

const LITELLM_API_KEY_VAR: &str = "LITELLM_API_KEY";

impl State {
    fn is_authenticated(&self) -> bool {
        self.api_key.is_some()
    }

    fn reset_api_key(&mut self) {
        self.api_key = None;
    }

    fn set_api_key(&mut self, key: String) {
        self.api_key = Some(key);
    }

    async fn authenticate(&mut self) -> Result<(), Error> {
        // Authentication logic
        Ok(())
    }

    async fn fetch_models(&mut self) -> Result<(), Error> {
        // Fetch models logic
        Ok(())
    }

    fn restart_fetch_models_task(&mut self) {
        // Restart fetch models task logic
    }
}

impl LiteLLMLanguageModelProvider {
    pub fn new(http_client: HttpClient) -> Self {
        Self {
            http_client,
            state: State {
                api_key: None,
                api_key_from_env: None,
                http_client: http_client.clone(),
                available_models: Vec::new(),
                fetch_models_task: None,
                _subscription: None,
            },
        }
    }

    fn create_language_model(&self, model: AvailableModel) -> LiteLLMLanguageModel {
        LiteLLMLanguageModel {
            id: model.name.clone(),
            model,
            state: self.state.clone(),
            http_client: self.http_client.clone(),
            request_limiter: RequestLimiter::new(),
        }
    }
}

impl LanguageModelProviderState for LiteLLMLanguageModelProvider {
    type ObservableEntity = State;

    fn observable_entity(&self) -> &Self::ObservableEntity {
        &self.state
    }
}

impl LanguageModelProvider for LiteLLMLanguageModelProvider {
    fn id(&self) -> &str {
        PROVIDER_ID
    }

    fn name(&self) -> &str {
        PROVIDER_NAME
    }

    fn icon(&self) -> Option<&str> {
        None
    }

    fn default_model(&self) -> Option<&AvailableModel> {
        self.state.available_models.first()
    }

    fn default_fast_model(&self) -> Option<&AvailableModel> {
        self.state.available_models.iter().find(|m| m.recommended)
    }

    fn provided_models(&self) -> Vec<&AvailableModel> {
        self.state.available_models.iter().collect()
    }

    fn recommended_models(&self) -> Vec<&AvailableModel> {
        self.state
            .available_models
            .iter()
            .filter(|m| m.recommended)
            .collect()
    }

    fn is_authenticated(&self) -> bool {
        self.state.is_authenticated()
    }

    async fn authenticate(&mut self) -> Result<(), Error> {
        self.state.authenticate().await
    }

    fn configuration_view(&self) -> ConfigurationView {
        ConfigurationView::new(self.state.clone())
    }

    fn reset_credentials(&mut self) {
        self.state.reset_api_key();
    }
}

pub struct LiteLLMLanguageModel {
    id: String,
    model: AvailableModel,
    state: State,
    http_client: HttpClient,
    request_limiter: RequestLimiter,
}

impl LiteLLMLanguageModel {
    async fn stream_completion(&self, input: &str) -> Result<String, Error> {
        // Stream completion logic
        Ok(String::new())
    }
}

impl LanguageModel for LiteLLMLanguageModel {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.model.display_name
    }

    fn provider_id(&self) -> &str {
        PROVIDER_ID
    }

    fn provider_name(&self) -> &str {
        PROVIDER_NAME
    }

    fn supports_tools(&self) -> bool {
        self.model.supports_tools
    }

    fn telemetry_id(&self) -> &str {
        &self.id
    }

    fn max_token_count(&self) -> usize {
        self.model.max_tokens
    }

    fn max_output_tokens(&self) -> usize {
        self.model.max_output_tokens
    }

    fn supports_tool_choice(&self) -> bool {
        self.model.supports_tools
    }

    fn supports_images(&self) -> bool {
        false
    }

    fn count_tokens(&self, input: &str) -> usize {
        input.len()
    }

    async fn stream_completion(&self, input: &str) -> Result<String, Error> {
        self.stream_completion(input).await
    }
}

pub fn into_litellm(settings: LiteLLMSettings) -> LiteLLMLanguageModelProvider {
    LiteLLMLanguageModelProvider::new(HttpClient::new())
}

pub struct LiteLLMEventMapper {
    tool_calls_by_index: HashMap<usize, RawToolCall>,
}

impl LiteLLMEventMapper {
    pub fn new() -> Self {
        Self {
            tool_calls_by_index: HashMap::new(),
        }
    }

    pub fn map_stream(&self, stream: &str) -> Vec<String> {
        // Map stream logic
        Vec::new()
    }

    pub fn map_event(&self, event: &str) -> Option<RawToolCall> {
        // Map event logic
        None
    }
}

struct RawToolCall {
    id: String,
    name: String,
    arguments: Vec<String>,
}

pub fn count_litellm_tokens(input: &str) -> usize {
    input.len()
}

struct ConfigurationView {
    api_key_editor: String,
    state: State,
    load_credentials_task: Option<Task>,
}

impl ConfigurationView {
    fn new(state: State) -> Self {
        Self {
            api_key_editor: String::new(),
            state,
            load_credentials_task: None,
        }
    }

    fn save_api_key(&mut self, key: String) {
        self.state.set_api_key(key);
    }

    fn reset_api_key(&mut self) {
        self.state.reset_api_key();
    }

    fn render_api_key_editor(&self) -> String {
        // Render API key editor logic
        String::new()
    }

    fn should_render_editor(&self) -> bool {
        self.state.api_key.is_none()
    }
}

impl Render for ConfigurationView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().child(self.render_api_key_editor())
    }
}

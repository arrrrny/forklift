// zed/crates/language_models/src/provider/openrouter.rs

use anyhow::{anyhow, Result};
use async_stream::stream;
use futures::Stream;
use gpui::MutableAppContext;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokenizers::Tokenizer;

use crate::{
    language_model::{LanguageModel, LanguageModelError, LanguageModelSettings},
    provider::{LanguageModelProvider, LanguageModelProviderState},
};

const PROVIDER_ID: &str = "openrouter";
const PROVIDER_NAME: &str = "OpenRouter";
const OPENROUTER_API_URL: &str = "https://openrouter.ai/api/v1";
const OPENROUTER_API_KEY_VAR: &str = "OPENROUTER_API_KEY";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct OpenRouterSettings {
    pub api_url: String,
    pub available_models: Vec<AvailableModel>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AvailableModel {
    pub name: String,
    pub display_name: String,
    pub max_tokens: usize,
}

#[derive(Clone)]
pub struct OpenRouterLanguageModelProvider {
    http_client: Client,
    state: Arc<State>,
}

#[derive(Clone)]
pub struct State {
    api_key: String,
    api_key_from_env: bool,
    _subscription: gpui::Subscription,
}

impl State {
    fn is_authenticated(&self) -> bool {
        !self.api_key.is_empty()
    }

    fn reset_api_key(&mut self, cx: &mut MutableAppContext) {
        self.set_api_key("".into(), false, cx);
    }

    fn set_api_key(&mut self, api_key: String, from_env: bool, cx: &mut MutableAppContext) {
        self.api_key = api_key;
        self.api_key_from_env = from_env;
        cx.notify();
    }

    async fn authenticate(&self) -> Result<()> {
        if self.is_authenticated() {
            return Ok(());
        }

        Err(anyhow!("OpenRouter API key is not set"))
    }
}

impl OpenRouterLanguageModelProvider {
    pub fn new(cx: &mut MutableAppContext) -> Self {
        let (api_key, api_key_from_env) = match std::env::var(OPENROUTER_API_KEY_VAR) {
            Ok(api_key) => (api_key, true),
            Err(_) => ("".into(), false),
        };

        let mut state = State {
            api_key,
            api_key_from_env,
            _subscription: gpui::Subscription::new(),
        };

        if state.api_key_from_env {
            state.authenticate().ok();
        }

        let state = Arc::new(state);
        let http_client = Client::new();

        Self {
            http_client,
            state,
        }
    }

    fn create_language_model(&self, model: String) -> Arc<dyn LanguageModel> {
        Arc::new(OpenRouterLanguageModel {
            id: model.clone(),
            model,
            state: self.state.clone(),
            http_client: self.http_client.clone(),
            request_limiter: None, // TODO: Add request limiter
        })
    }
}

impl LanguageModelProviderState for OpenRouterLanguageModelProvider {
    type ObservableEntity = State;

    fn observable_entity(&self) -> &Arc<Self::ObservableEntity> {
        &self.state
    }
}

impl LanguageModelProvider for OpenRouterLanguageModelProvider {
    fn id(&self) -> &'static str {
        PROVIDER_ID
    }

    fn name(&self) -> &'static str {
        PROVIDER_NAME
    }

    fn icon(&self) -> &'static str {
        ""
    }

    fn default_model(&self) -> &'static str {
        "openai/gpt-3.5-turbo"
    }

    fn default_fast_model(&self) -> &'static str {
        "openai/gpt-3.5-turbo"
    }

    fn provided_models(&self) -> Vec<&'static str> {
        vec!["openai/gpt-3.5-turbo", "openai/gpt-4"]
    }

    fn is_authenticated(&self) -> bool {
        self.state.is_authenticated()
    }

    fn authenticate(&self) -> Result<()> {
        self.state.authenticate().await
    }

    fn configuration_view(&self) -> gpui::AnyView {
        gpui::AnyView::Empty
    }

    fn reset_credentials(&self, cx: &mut MutableAppContext) {
        self.state.reset_api_key(cx);
    }
}

struct OpenRouterLanguageModel {
    id: String,
    model: String,
    state: Arc<State>,
    http_client: Client,
    request_limiter: Option<Arc<tokio::sync::Semaphore>>,
}

impl OpenRouterLanguageModel {
    async fn stream_completion(
        &self,
        prompt: String,
        tools: Option<String>,
    ) -> Result<Box<dyn Stream<Item = Result<String>> + Unpin>> {
        let api_key = self.state.api_key.clone();
        let model = self.model.clone();
        let http_client = self.http_client.clone();

        let request_body = json::object! {
            "model": model,
            "messages": [{
                "role": "user",
                "content": prompt,
            }],
            "stream": true,
            "tools": tools,
        };

        let request = http_client
            .post(format!("{}/chat/completions", OPENROUTER_API_URL))
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .body(request_body.dump());

        let response = request.send().await?;

        if !response.status().is_success() {
            let error_message = response.text().await?;
            return Err(anyhow!("OpenRouter API error: {}", error_message));
        }

        let stream = stream! {
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                let text = String::from_utf8(chunk.to_vec())?;
                yield Ok(text);
            }
        };

        Ok(Box::new(stream))
    }
}

impl LanguageModel for OpenRouterLanguageModel {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn name(&self) -> String {
        self.model.clone()
    }

    fn provider_id(&self) -> &'static str {
        PROVIDER_ID
    }

    fn provider_name(&self) -> &'static str {
        PROVIDER_NAME
    }

    fn supports_tools(&self) -> bool {
        true
    }

    fn tool_input_format(&self) -> crate::language_model::ToolInputFormat {
        crate::language_model::ToolInputFormat::OpenAI
    }

    fn telemetry_id(&self) -> String {
        format!("{}-{}", PROVIDER_ID, self.model)
    }

    fn max_token_count(&self) -> usize {
        4096 // TODO: Get this from the model metadata
    }

    fn count_tokens(&self, prompt: &str) -> Result<usize, LanguageModelError> {
        let tokenizer = Tokenizer::from_pretrained("bert-base-uncased", None).map_err(|e| {
            LanguageModelError::Other(anyhow::Error::msg(format!("Failed to load tokenizer: {}", e)))
        })?;
        let encoding = tokenizer.encode(prompt, false).map_err(|e| {
            LanguageModelError::Other(anyhow::Error::msg(format!("Failed to encode prompt: {}", e)))
        })?;
        Ok(encoding.len())
    }

    fn stream_completion(
        &self,
        prompt: String,
        tools: Option<String>,
    ) -> Result<Box<dyn Stream<Item = Result<String>> + Unpin>, LanguageModelError> {
        let model = self.clone();
        Ok(model.stream_completion(prompt, tools).await?)
    }
}

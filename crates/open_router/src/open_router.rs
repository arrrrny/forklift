use anyhow::{anyhow, Context as _, Result};
use bytes;
use futures::{
    AsyncBufReadExt, AsyncReadExt, StreamExt,
    io::BufReader,
    stream::{self, BoxStream},
};
use http_client::{AsyncBody, HttpClient, Method, Request as HttpRequest};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    convert::TryFrom,
};
use strum::EnumIter;

pub const OPEN_ROUTER_API_URL: &str = "https://openrouter.ai/api/v1";

fn is_none_or_empty<T: AsRef<[U]>, U>(opt: &Option<T>) -> bool {
    opt.as_ref().map_or(true, |v| v.as_ref().is_empty())
}

#[derive(Clone, Copy, Serialize, Deserialize, Debug, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
    Tool,
}

impl TryFrom<String> for Role {
    type Error = anyhow::Error;

    fn try_from(value: String) -> Result<Self> {
        match value.as_str() {
            "user" => Ok(Self::User),
            "assistant" => Ok(Self::Assistant),
            "system" => Ok(Self::System),
            "tool" => Ok(Self::Tool),
            _ => Err(anyhow!("invalid role '{value}'")),
        }
    }
}

impl From<Role> for String {
    fn from(val: Role) -> Self {
        match val {
            Role::User => "user".to_owned(),
            Role::Assistant => "assistant".to_owned(),
            Role::System => "system".to_owned(),
            Role::Tool => "tool".to_owned(),
        }
    }
}

#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, EnumIter)]
pub enum Model {
    #[serde(rename = "google/gemini-2.0-flash-exp:free")]
    #[default]
    Gemini20FlashExp,
    #[serde(rename = "google/gemini-2.5-pro-exp-03-25")]
    Gemini25ProExp0325,
    #[serde(rename = "qwen/qwen-turbo")]
    QwenTurbo,
    #[serde(rename = "meta-llama/llama-4-scout")]
    Llama4Scout,
    #[serde(rename = "qwen/qwen3-235b-a22b")]
    Qwen3235bA22b,
    #[serde(rename = "custom")]
    Custom {
        name: String,
        display_name: Option<String>,
        max_tokens: usize,
        max_output_tokens: Option<u32>,
        max_completion_tokens: Option<u32>,
    },
}

impl Model {
    pub fn default_fast() -> Self {
        Self::Gemini20FlashExp
    }
    
    pub fn from_id(id: &str) -> Result<Self> {
        match id {
            "google/gemini-2.0-flash-exp:free" => Ok(Self::Gemini20FlashExp),
            "google/gemini-2.5-pro-exp-03-25" => Ok(Self::Gemini25ProExp0325),
            "qwen/qwen-turbo" => Ok(Self::QwenTurbo),
            "meta-llama/llama-4-scout" => Ok(Self::Llama4Scout),
            "qwen/qwen3-235b-a22b" => Ok(Self::Qwen3235bA22b),
            _ => Err(anyhow!("invalid model id")),
        }
    }
    
    pub fn id(&self) -> &str {
        match self {
            Self::Gemini20FlashExp => "google/gemini-2.0-flash-exp:free",
            Self::Gemini25ProExp0325 => "google/gemini-2.5-pro-exp-03-25",
            Self::QwenTurbo => "qwen/qwen-turbo",
            Self::Llama4Scout => "meta-llama/llama-4-scout",
            Self::Qwen3235bA22b => "qwen/qwen3-235b-a22b",
            Self::Custom { name, .. } => name,
        }
    }
    
    pub fn display_name(&self) -> &str {
        match self {
            Self::Gemini20FlashExp => "Gemini 2.0 Flash (1M, tools)",
            Self::Gemini25ProExp0325 => "Gemini 2.5 Pro (1M, tools)",
            Self::QwenTurbo => "Qwen Turbo (1M, tools)",
            Self::Llama4Scout => "Llama 4 Scout (128K, tools)",
            Self::Qwen3235bA22b => "Qwen3 235B (tools)",
            Self::Custom {
                name, display_name, ..
            } => display_name.as_ref().unwrap_or(name),
        }
    }
    
    pub fn max_token_count(&self) -> usize {
        match self {
            Self::Gemini20FlashExp | Self::Gemini25ProExp0325 | Self::QwenTurbo => 1_000_000,
            Self::Llama4Scout => 128_000,
            Self::Qwen3235bA22b => 0,
            Self::Custom { max_tokens, .. } => *max_tokens,
        }
    }
    
    pub fn max_output_tokens(&self) -> Option<u32> {
        match self {
            Self::Custom {
                max_output_tokens, ..
            } => *max_output_tokens,
            _ => None,
        }
    }
    
    pub fn supports_parallel_tool_calls(&self) -> bool {
        true
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Request {
    pub model: String,
    pub messages: Vec<RequestMessage>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "is_none_or_empty")]
    pub stop: Option<Vec<String>>,
    pub temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CompletionRequest {
    pub model: String,
    pub prompt: String,
    pub max_tokens: Option<u32>,
    pub temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prediction: Option<Prediction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rewrite_speculation: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Prediction {
    Content { content: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolChoice {
    Auto,
    Required,
    None,
    Other(Value),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ToolDefinition {
    #[serde(rename = "function")]
    Function { function: FunctionDefinition },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FunctionDefinition {
    pub name: String,
    pub description: Option<String>,
    pub parameters: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "role", content = "content")]
pub enum RequestMessage {
    #[serde(rename = "assistant")]
    Assistant {
        content: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_calls: Option<Vec<ToolCall>>,
    },
    #[serde(rename = "user")]
    User {
        content: String,
    },
    #[serde(rename = "system")]
    System {
        content: String,
    },
    #[serde(rename = "tool")]
    Tool {
        content: String,
        tool_call_id: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(flatten)]
    pub content: ToolCallContent,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ToolCallContent {
    #[serde(rename = "function")]
    Function { function: FunctionContent },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FunctionContent {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Deserialize)]
pub struct ResponseMessageDelta {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallChunk>>,
}

#[derive(Debug, Deserialize)]
pub struct ToolCallChunk {
    pub index: usize,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    #[serde(rename = "type")]
    _type: Option<String>,
    #[serde(default)]
    pub function: Option<FunctionChunk>,
}

#[derive(Debug, Deserialize)]
pub struct FunctionChunk {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Usage {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub total_tokens: usize,
}

#[derive(Debug, Deserialize)]
pub struct ChoiceDelta {
    pub index: usize,
    pub delta: ResponseMessageDelta,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ResponseStreamResult {
    Ok(ResponseStreamEvent),
    Err { error: Value },
}

#[derive(Debug, Deserialize)]
pub struct ResponseStreamEvent {
    #[serde(default)]
    pub created: Option<usize>,
    pub model: String,
    pub choices: Vec<ChoiceDelta>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
pub struct CompletionResponse {
    pub id: String,
    pub object: String,
    pub created: usize,
    pub model: String,
    pub choices: Vec<CompletionChoice>,
    pub usage: Usage,
}

#[derive(Debug, Deserialize)]
pub struct CompletionChoice {
    pub text: String,
}

#[derive(Debug, Deserialize)]
pub struct Response {
    pub id: String,
    pub object: String,
    pub created: usize,
    pub model: String,
    pub choices: Vec<Choice>,
    pub usage: Usage,
}

#[derive(Debug, Deserialize)]
pub struct Choice {
    pub index: usize,
    pub message: Value,
    pub finish_reason: Option<String>,
}

pub async fn complete(
    http_client: &dyn HttpClient,
    api_key: &str,
    api_url: &str,
    request: &Request,
) -> Result<Response> {
    // Setup the request
    let http_request = HttpRequest::builder()
        .method(Method::POST)
        .uri(format!("{}/chat/completions", api_url))
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", api_key))
        .header("User-Agent", "Zed")
        .header("HTTP-Referer", "https://zed.dev");

    // Add the API request body
    let body = serde_json::to_vec(request).context("failed to serialize request")?;
    let bytes = bytes::Bytes::from(body);
    let body = AsyncBody::from_bytes(bytes);

    // Make the request
    let response = http_client
        .send(http_request.body(body).unwrap())
        .await
        .context("failed to send request to OpenRouter")?;

    // Stream in the response body
    let status = response.status();
    let mut reader = BufReader::new(response.into_body());
    let mut body = Vec::new();
    reader
        .read_to_end(&mut body)
        .await
        .context("failed to read response body from OpenRouter")?;

    // Handle non-success response
    if !status.is_success() {
        #[derive(Deserialize)]
        struct OpenRouterResponse {
            error: OpenRouterError,
        }

        #[derive(Deserialize)]
        struct OpenRouterError {
            message: String,
        }

        let error = serde_json::from_slice::<OpenRouterResponse>(&body)
            .map(|response| response.error.message)
            .unwrap_or_else(|_| format!("OpenRouter request failed: {}", status));

        return Err(anyhow!(error));
    }

    let response = serde_json::from_slice::<Response>(&body).context("failed to parse response")?;

    Ok(response)
}

pub async fn complete_text(
    http_client: &dyn HttpClient,
    api_key: &str,
    api_url: &str,
    request: &CompletionRequest,
) -> Result<CompletionResponse> {
    // Setup the request
    let http_request = HttpRequest::builder()
        .method(Method::POST)
        .uri(format!("{}/completions", api_url))
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", api_key))
        .header("User-Agent", "Zed")
        .header("HTTP-Referer", "https://zed.dev");

    // Add the API request body
    let body = serde_json::to_vec(request).context("failed to serialize request")?;
    let bytes = bytes::Bytes::from(body);
    let body = AsyncBody::from_bytes(bytes);

    // Make the request
    let response = http_client
        .send(http_request.body(body).unwrap())
        .await
        .context("failed to send request to OpenRouter")?;

    // Stream in the response body
    let status = response.status();
    let mut reader = BufReader::new(response.into_body());
    let mut body = Vec::new();
    reader
        .read_to_end(&mut body)
        .await
        .context("failed to read response body from OpenRouter")?;

    // Handle non-success response
    if !status.is_success() {
        #[derive(Deserialize)]
        struct OpenRouterResponse {
            error: OpenRouterError,
        }

        #[derive(Deserialize)]
        struct OpenRouterError {
            message: String,
        }

        let error = serde_json::from_slice::<OpenRouterResponse>(&body)
            .map(|response| response.error.message)
            .unwrap_or_else(|_| format!("OpenRouter request failed: {}", status));

        return Err(anyhow!(error));
    }

    let response =
        serde_json::from_slice::<CompletionResponse>(&body).context("failed to parse response")?;

    Ok(response)
}

fn adapt_response_to_stream(
    http_status: http_client::StatusCode,
    reader: BufReader<impl AsyncReadExt + Unpin + Send + 'static>,
) -> BoxStream<'static, Result<ResponseStreamResult>> {
    if http_status.is_success() {
        stream::unfold(
            (reader, Vec::new(), false),
            |(mut reader, mut buffer, mut is_done)| async move {
                if is_done {
                    return None;
                }

                let mut line = String::new();
                match reader.read_line(&mut line).await {
                    Ok(0) => {
                        is_done = true;
                        return Some((
                            Ok(ResponseStreamResult::Ok(ResponseStreamEvent {
                                created: None,
                                model: "".to_string(),
                                choices: Vec::new(),
                                usage: None,
                            })),
                            (reader, buffer, is_done),
                        ));
                    }
                    Ok(_) => {
                        line = line.trim_end().to_string();
                        const DATA_FIELD: &str = "data: ";
                        if line.is_empty() || !line.starts_with(DATA_FIELD) {
                            return Some((
                                Ok(ResponseStreamResult::Ok(ResponseStreamEvent {
                                    created: None,
                                    model: "".to_string(),
                                    choices: Vec::new(),
                                    usage: None,
                                })),
                                (reader, buffer, is_done),
                            ));
                        }

                        line = line[DATA_FIELD.len()..].to_string();
                        if line == "[DONE]" {
                            is_done = true;
                            return Some((
                                Ok(ResponseStreamResult::Ok(ResponseStreamEvent {
                                    created: None,
                                    model: "".to_string(),
                                    choices: Vec::new(),
                                    usage: None,
                                })),
                                (reader, buffer, is_done),
                            ));
                        }

                        match serde_json::from_str::<ResponseStreamResult>(&line) {
                            Ok(event) => Some((Ok(event), (reader, buffer, is_done))),
                            Err(e) => {
                                buffer.extend_from_slice(line.as_bytes());
                                Some((
                                    Err(anyhow!("failed to parse response: {}", e)),
                                    (reader, buffer, is_done),
                                ))
                            }
                        }
                    }
                    Err(e) => {
                        is_done = true;
                        Some((
                            Err(anyhow!("failed to read response: {}", e)),
                            (reader, buffer, is_done),
                        ))
                    }
                }
            },
        )
        .boxed()
    } else {
        stream::once(async move {
            let mut body = Vec::new();
            let mut reader = reader;

            match reader.read_to_end(&mut body).await {
                Ok(_) => {
                    #[derive(Deserialize)]
                    struct OpenRouterResponse {
                        error: OpenRouterError,
                    }

                    #[derive(Deserialize)]
                    struct OpenRouterError {
                        message: String,
                    }

                    let error = serde_json::from_slice::<OpenRouterResponse>(&body)
                        .map(|response| response.error.message)
                        .unwrap_or_else(|_| {
                            format!(
                                "OpenRouter request failed: {}",
                                std::str::from_utf8(&body).unwrap_or("<binary>")
                            )
                        });

                    Err(anyhow!(error))
                }
                Err(e) => Err(anyhow!("failed to read response body: {}", e)),
            }
        })
        .boxed()
    }
}

pub async fn stream_completion(
    http_client: &dyn HttpClient,
    api_key: &str,
    api_url: &str,
    request: &Request,
) -> BoxStream<'static, Result<ResponseStreamResult>> {
    // Build a request based on the initial parameters
    let mut streamed_request = request.clone();
    streamed_request.stream = true;

    let http_request = HttpRequest::builder()
        .method(Method::POST)
        .uri(format!("{}/chat/completions", api_url))
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", api_key))
        .header("User-Agent", "Zed")
        .header("HTTP-Referer", "https://zed.dev");

    // Add the API request body
    let body = match serde_json::to_vec(&streamed_request) {
        Ok(body) => {
            let bytes = bytes::Bytes::from(body);
            AsyncBody::from_bytes(bytes)
        },
        Err(e) => {
            return stream::once(async move { Err(anyhow!("failed to serialize request: {}", e)) })
                .boxed()
        }
    };

    // Make the request
    match http_client.send(http_request.body(body).unwrap()).await {
        Ok(response) => {
            let status = response.status();
            let reader = BufReader::new(response.into_body());
            adapt_response_to_stream(status, reader)
        }
        Err(e) => {
            stream::once(async move { Err(anyhow!("failed to send request to OpenRouter: {}", e)) })
                .boxed()
        }
    }
}

// Token counting function for OpenRouter
pub fn count_open_router_tokens(prompt_text: &str) -> usize {
    // OpenRouter token counting will depend on the specific model
    // This is a simplified approach that gives a reasonable approximation
    // For an accurate count, we would need to implement the tokenization algorithm for each model
    // This is a rough approximation based on GPT tokenizers
    
    let mut token_count = 0;
    
    // Simple GPT-like tokenization approximation
    // Count words and punctuation as separate tokens
    let mut in_word = false;
    
    for c in prompt_text.chars() {
        if c.is_whitespace() {
            if in_word {
                in_word = false;
            }
        } else if c.is_alphanumeric() {
            if !in_word {
                token_count += 1;
                in_word = true;
            }
        } else {
            // Punctuation or special character
            token_count += 1;
            in_word = false;
        }
    }
    
    // Adjust for token encoding inefficiencies
    token_count = (token_count as f32 * 0.75).ceil() as usize;
    
    token_count
}

// OpenRouter doesn't have embeddings in the same way as OpenAI, but we can add placeholders
pub enum OpenRouterEmbeddingModel {
    DefaultEmbedding,
    LargeEmbedding,
}

#[allow(dead_code)]
pub struct OpenRouterEmbeddingRequest {
    model: String,
    input: String,
}

pub struct OpenRouterEmbeddingResponse {
    pub data: Vec<OpenRouterEmbedding>,
}

pub struct OpenRouterEmbedding {
    pub embedding: Vec<f32>,
}

pub async fn embed(
    _http_client: &dyn HttpClient,
    _api_key: &str,
    _api_url: &str,
    _model: OpenRouterEmbeddingModel,
    _input: &str,
) -> Result<Vec<f32>> {
    // OpenRouter doesn't directly support embeddings like OpenAI
    // This would need to be implemented based on what OpenRouter actually supports
    // For now, returning an error to indicate this isn't implemented
    Err(anyhow!("Embeddings are not supported by OpenRouter API"))
}
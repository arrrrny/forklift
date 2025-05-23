use anyhow::{Result, anyhow};
use futures::{
    AsyncBufReadExt, AsyncReadExt,
    io::BufReader,
    stream::{BoxStream, StreamExt},
};
use http_client::{AsyncBody, HttpClient, Method, Request as HttpRequest};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::convert::TryFrom;

pub const LITELLM_API_URL: &str = "http://localhost:4000";

#[derive(Debug, Clone, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: Option<String>,
    pub max_tokens: Option<usize>,
    pub max_output_tokens: Option<u32>,
}

pub async fn fetch_models(
    client: &dyn HttpClient,
    _api_url: &str,
    api_key: &str,
) -> anyhow::Result<Vec<ModelInfo>> {
    let uri = format!("{}/v1/models", LITELLM_API_URL.trim_end_matches('/'));
    log::info!("Fetching models from: {}", uri);

    // Detailed logging for HTTP request building
    log::debug!(
        "Building HTTP GET request to {} with headers: Authorization: Bearer {}",
        uri,
        api_key
    );
    let request = HttpRequest::builder()
        .method(Method::GET)
        .uri(uri)
        .header("Authorization", format!("Bearer {}", api_key))
        .body(AsyncBody::empty())?;

    let mut response = match client.send(request).await {
        Ok(response) => response,
        Err(err) => {
            log::error!("Error sending request to LiteLLM API: {}", err);
            return Err(anyhow!("Error sending request to LiteLLM API: {}", err));
        }
    };

    if !response.status().is_success() {
        let mut error_body = String::new();
        response.body_mut().read_to_string(&mut error_body).await?;
        log::error!(
            "Failed to fetch models: {} - {}",
            response.status(),
            error_body
        );
        return Err(anyhow!(
            "Failed to fetch models: {} - {}",
            response.status(),
            error_body
        ));
    }

    let mut body = Vec::new();
    response.body_mut().read_to_end(&mut body).await?;

    let json: serde_json::Value = match serde_json::from_slice(&body) {
        Ok(json) => json,
        Err(err) => {
            let body_str = String::from_utf8_lossy(&body);
            log::error!(
                "Failed to parse JSON response: {} - Raw response: {}",
                err,
                body_str
            );
            return Err(anyhow!("Failed to parse JSON response: {}", err));
        }
    };

    // Handle different response formats - LiteLLM should follow OpenAI format
    // but let's be more robust
    if let Some(data_array) = json["data"].as_array() {
        let models = data_array
            .iter()
            .map(|m| {
                let model_info = serde_json::from_value(m.clone());
                if let Err(ref e) = model_info {
                    log::warn!("Failed to parse model info: {} - Raw data: {:?}", e, m);
                }
                model_info
            })
            .collect::<Result<Vec<ModelInfo>, _>>()?;

        log::info!(
            "Successfully fetched {} models from LiteLLM API",
            models.len()
        );
        Ok(models)
    } else {
        // Try an alternate format where the models might be at the root level
        log::warn!("Standard OpenAI format not found, trying alternate format");

        // If the response is directly an array of models
        if let Some(models_array) = json.as_array() {
            let models = models_array
                .iter()
                .map(|m| -> anyhow::Result<ModelInfo> {
                    // Try to extract id from the model object
                    let id = m["id"]
                        .as_str()
                        .or_else(|| m["name"].as_str())
                        .ok_or_else(|| anyhow!("Model missing id/name"))?
                        .to_string();

                    Ok(ModelInfo {
                        id,
                        name: m["name"].as_str().map(|s| s.to_string()),
                        max_tokens: m["max_tokens"].as_u64().map(|n| n as usize),
                        max_output_tokens: m["max_output_tokens"].as_u64().map(|n| n as u32),
                    })
                })
                .collect::<anyhow::Result<Vec<ModelInfo>>>()?;

            log::info!(
                "Successfully fetched {} models from alternate format",
                models.len()
            );
            Ok(models)
        } else {
            log::error!("Invalid models response format: {:?}", json);
            Err(anyhow!(
                "Invalid models response format - unable to find models array"
            ))
        }
    }
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
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Model {
    pub name: String,
    pub display_name: Option<String>,
    pub max_tokens: usize,
    pub max_output_tokens: Option<u32>,
    pub max_completion_tokens: Option<u32>,
    pub supports_tools: Option<bool>,
}

impl Model {
    pub fn max_output_tokens(&self) -> Option<u32> {
        self.max_output_tokens
    }

    pub fn default_fast() -> Self {
        Self::new(
            "litellm/auto",
            Some("Auto Router"),
            Some(2000000),
            None,
            None,
            Some(true),
        )
    }

    pub fn default() -> Self {
        Self::default_fast()
    }

    pub fn new(
        name: &str,
        display_name: Option<&str>,
        max_tokens: Option<usize>,
        max_output_tokens: Option<u32>,
        max_completion_tokens: Option<u32>,
        supports_tools: Option<bool>,
    ) -> Self {
        Self {
            name: name.to_owned(),
            display_name: display_name.map(|s| s.to_owned()),
            max_tokens: max_tokens.unwrap_or(2000000),
            max_output_tokens,
            max_completion_tokens,
            supports_tools,
        }
    }

    pub fn from_id(id: &str) -> Self {
        Self {
            name: id.to_owned(),
            display_name: None,
            max_tokens: 2000000,
            max_output_tokens: None,
            max_completion_tokens: None,
            supports_tools: None,
        }
    }

    pub fn id(&self) -> &str {
        &self.name
    }

    pub fn display_name(&self) -> &str {
        self.display_name.as_ref().unwrap_or(&self.name)
    }

    pub fn max_token_count(&self) -> usize {
        self.max_tokens
    }

    pub fn supports_tool_calls(&self) -> bool {
        self.supports_tools.unwrap_or(false)
    }

    pub fn supports_parallel_tool_calls(&self) -> bool {
        false
    }
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Request {
    pub model: String,
    pub messages: Vec<RequestMessage>,
    pub stream: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stop: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel_tool_calls: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseFormat {
    Text,
    #[serde(rename = "json_object")]
    JsonObject,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolChoice {
    Auto,
    Required,
    None,
    Other(ToolDefinition),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolDefinition {
    Function { function: FunctionDefinition },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FunctionDefinition {
    pub name: String,
    pub description: Option<String>,
    pub parameters: Option<Value>,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum RequestMessage {
    Assistant {
        content: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ToolCall>,
    },
    User {
        content: String,
    },
    System {
        content: String,
    },
    Tool {
        content: String,
        tool_call_id: String,
    },
    #[serde(rename = "function")]
    Function {
        content: String,
        name: String,
    },
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
pub struct ToolCall {
    pub id: String,
    #[serde(flatten)]
    pub content: ToolCallContent,
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ToolCallContent {
    Function { function: FunctionContent },
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq)]
pub struct FunctionContent {
    pub name: String,
    pub arguments: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Response {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<Choice>,
    pub usage: Usage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    #[serde(default)]
    pub prompt_cache_hit_tokens: u32,
    #[serde(default)]
    pub prompt_cache_miss_tokens: u32,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Choice {
    pub index: u32,
    pub message: RequestMessage,
    pub finish_reason: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct StreamResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<StreamChoice>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct StreamChoice {
    pub index: u32,
    pub delta: StreamDelta,
    pub finish_reason: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct StreamDelta {
    pub role: Option<Role>,
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallChunk>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ToolCallChunk {
    pub index: usize,
    pub id: Option<String>,
    pub function: Option<FunctionChunk>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct FunctionChunk {
    pub name: Option<String>,
    pub arguments: Option<String>,
}

pub async fn stream_completion(
    client: &dyn HttpClient,
    _api_url: &str,
    api_key: &str,
    request: Request,
) -> Result<BoxStream<'static, Result<StreamResponse>>> {
    let uri = format!(
        "{}/v1/chat/completions",
        LITELLM_API_URL.trim_end_matches('/')
    );
    let request_builder = HttpRequest::builder()
        .method(Method::POST)
        .uri(uri)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", api_key));

    // Detailed logging for HTTP request building and payload
    let body_json = serde_json::to_string(&request)?;
    log::debug!(
        "Building HTTP POST request to with headers: Content-Type: application/json, Authorization: Bearer {}",
        api_key
    );
    log::debug!("Request payload: {}", body_json);
    let request = request_builder.body(AsyncBody::from(body_json.clone()))?;
    let mut response = client.send(request).await?;

    if response.status().is_success() {
        let reader = BufReader::new(response.into_body());
        Ok(reader
            .lines()
            .filter_map(|line| async move {
                match line {
                    Ok(line) => {
                        let line = line.strip_prefix("data: ")?;
                        if line == "[DONE]" {
                            None
                        } else {
                            match serde_json::from_str(line) {
                                Ok(response) => Some(Ok(response)),
                                Err(error) => Some(Err(anyhow!(error))),
                            }
                        }
                    }
                    Err(error) => Some(Err(anyhow!(error))),
                }
            })
            .boxed())
    } else {
        let mut body = String::new();
        response.body_mut().read_to_string(&mut body).await?;

        #[derive(Deserialize)]
        struct LiteLLMResponseError {
            error: LiteLLMError,
        }
        #[derive(Deserialize)]
        struct LiteLLMError {
            message: String,
        }

        match serde_json::from_str::<LiteLLMResponseError>(&body) {
            Ok(response) if !response.error.message.is_empty() => Err(anyhow!(
                "Failed to connect to LiteLLM API: {}",
                response.error.message,
            )),
            _ => Err(anyhow!(
                "Failed to connect to LiteLLM API: {} {}",
                response.status(),
                body,
            )),
        }
    }
}

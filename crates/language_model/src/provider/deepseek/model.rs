use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use strum::{Display, EnumIter};

#[derive(Clone, Debug, Display, EnumIter)]
pub enum Model {
    Chat,
    Reasoner,
    Custom {
        name: String,
        display_name: Option<String>,
        max_tokens: usize,
        max_output_tokens: Option<u32>,
    },
}

impl Model {
    pub fn id(&self) -> &str {
        match self {
            Model::Chat => "deepseek-chat",
            Model::Reasoner => "deepseek-reasoner",
            Model::Custom { name, .. } => name,
        }
    }

    pub fn display_name(&self) -> &str {
        match self {
            Model::Chat => "DeepSeek Chat",
            Model::Reasoner => "DeepSeek Reasoner",
            Model::Custom {
                display_name: Some(name),
                ..
            } => name,
            Model::Custom { name, .. } => name,
        }
    }

    pub fn max_token_count(&self) -> usize {
        match self {
            Model::Chat => 32768,
            Model::Reasoner => 32768,
            Model::Custom { max_tokens, .. } => *max_tokens,
        }
    }

    pub fn max_output_tokens(&self) -> Option<u32> {
        match self {
            Model::Chat => Some(4096),
            Model::Reasoner => Some(4096),
            Model::Custom {
                max_output_tokens, ..
            } => *max_output_tokens,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Request {
    pub model: String,
    pub messages: Vec<RequestMessage>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDefinition>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum RequestMessage {
    #[serde(rename = "user")]
    User { content: String },
    #[serde(rename = "assistant")]
    Assistant {
        content: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        tool_calls: Vec<ToolCall>,
    },
    #[serde(rename = "system")]
    System { content: String },
}

#[derive(Debug, Serialize)]
pub struct ToolCall {
    pub id: String,
    pub function: FunctionCall,
}

#[derive(Debug, Serialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum ToolDefinition {
    #[serde(rename = "function")]
    Function { function: FunctionDefinition },
}

#[derive(Debug, Serialize)]
pub struct FunctionDefinition {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ResponseFormat {
    JsonObject,
}

#[derive(Debug, Deserialize)]
pub struct StreamResponse {
    pub id: Option<String>,
    pub choices: Vec<StreamChoice>,
}

#[derive(Debug, Deserialize)]
pub struct StreamChoice {
    pub index: usize,
    pub delta: Delta,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Delta {
    pub role: Option<String>,
    pub content: Option<String>,
    pub tool_calls: Option<Vec<StreamToolCall>>,
}

#[derive(Debug, Deserialize)]
pub struct StreamToolCall {
    pub index: usize,
    pub id: Option<String>,
    pub function: Option<StreamFunction>,
}

#[derive(Debug, Deserialize)]
pub struct StreamFunction {
    pub name: Option<String>,
    pub arguments: Option<String>,
}

#[derive(Debug)]
pub struct DeepSeekError {
    pub message: String,
}

impl fmt::Display for DeepSeekError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for DeepSeekError {}

pub async fn stream_completion(
    http: &dyn http_client::HttpClient,
    api_url: &str,
    api_key: &str,
    request: Request,
) -> Result<futures::stream::BoxStream<'static, Result<StreamResponse, anyhow::Error>>, anyhow::Error> {
    let response = http
        .post(api_url)
        .header("Authorization", &format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&request)?
        .send()
        .await?;

    if !response.status().is_success() {
        let error: Value = response.json().await?;
        return Err(anyhow::anyhow!(
            "DeepSeek API error: {}",
            error["error"]["message"]
                .as_str()
                .unwrap_or("Unknown error")
        ));
    }

    let stream = response
        .bytes_stream()
        .map_err(|e| anyhow::anyhow!("Stream error: {}", e))
        .map(|chunk| async {
            let chunk = chunk?;
            let text = String::from_utf8_lossy(&chunk);
            for line in text.lines() {
                if line.starts_with("data: ") {
                    let data = &line["data: ".len()..];
                    if data == "[DONE]" {
                        continue;
                    }
                    let response: StreamResponse = serde_json::from_str(data)?;
                    return Ok(response);
                }
            }
            Err(anyhow::anyhow!("Invalid response format"))
        })
        .filter_map(|f| async { f.await.ok() });

    Ok(Box::pin(stream))
}

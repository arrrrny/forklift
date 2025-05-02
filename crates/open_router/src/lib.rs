use std::fmt;

mod open_router;
mod model_traits;

pub use open_router::*;
pub use model_traits::*;

#[derive(Debug)]
pub enum OpenRouterError {
    ApiError(String),
    RateLimitExceeded {
        retry_after: Option<u64>,
    },
    InvalidApiKey,
    NetworkError(String),
    TokenLimitExceeded {
        model: String,
        requested: usize,
        limit: usize,
    },
}

impl fmt::Display for OpenRouterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApiError(msg) => write!(f, "OpenRouter API error: {}", msg),
            Self::RateLimitExceeded { retry_after } => {
                if let Some(secs) = retry_after {
                    write!(f, "Rate limit exceeded. Retry after {} seconds", secs)
                } else {
                    write!(f, "Rate limit exceeded")
                }
            }
            Self::InvalidApiKey => write!(f, "Invalid API key"),
            Self::NetworkError(msg) => write!(f, "Network error: {}", msg),
            Self::TokenLimitExceeded {
                model,
                requested,
                limit,
            } => write!(
                f,
                "Token limit exceeded for model {}. Requested: {}, Limit: {}",
                model, requested, limit
            ),
        }
    }
}

impl std::error::Error for OpenRouterError {}

// Pricing constants in USD per 1M tokens
pub mod pricing {
    pub const FREE_MODELS: f64 = 0.0;
    
    pub mod input {
        pub const QWEN_TURBO: f64 = 0.05;
        pub const LLAMA_SCOUT: f64 = 0.11;
        pub const QWEN3_235B: f64 = 0.20;
        pub const GEMINI_FLASH_THINKING: f64 = 0.15;
    }
    
    pub mod output {
        pub const QWEN_TURBO: f64 = 0.20;
        pub const LLAMA_SCOUT: f64 = 0.34;
        pub const QWEN3_235B: f64 = 0.80;
        pub const GEMINI_FLASH_THINKING: f64 = 3.50;
    }
}

// Model capabilities and limits
pub mod capabilities {
    pub struct ModelCapability {
        pub max_tokens: usize,
        pub supports_tools: bool,
        pub supports_streaming: bool,
        pub supports_function_calling: bool,
    }

    pub const GEMINI_FLASH_FREE: ModelCapability = ModelCapability {
        max_tokens: 1_000_000,
        supports_tools: true,
        supports_streaming: true,
        supports_function_calling: true,
    };

    pub const GEMINI_PRO_EXP: ModelCapability = ModelCapability {
        max_tokens: 1_000_000,
        supports_tools: true,
        supports_streaming: true,
        supports_function_calling: true,
    };

    // Add more model capabilities as needed
}

// Rate limiting configuration
pub mod rate_limits {
    use std::time::Duration;

    pub const DEFAULT_RPM: u32 = 60;  // Requests per minute
    pub const DEFAULT_TPM: u32 = 100_000;  // Tokens per minute
    
    pub const RETRY_AFTER_DEFAULT: Duration = Duration::from_secs(60);
    
    pub struct ModelRateLimit {
        pub rpm: u32,
        pub tpm: u32,
    }
    
    pub const FREE_MODELS: ModelRateLimit = ModelRateLimit {
        rpm: 30,
        tpm: 50_000,
    };
    
    pub const PAID_MODELS: ModelRateLimit = ModelRateLimit {
        rpm: 60,
        tpm: 100_000,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = OpenRouterError::TokenLimitExceeded {
            model: "test-model".to_string(),
            requested: 2000,
            limit: 1000,
        };
        assert_eq!(
            err.to_string(),
            "Token limit exceeded for model test-model. Requested: 2000, Limit: 1000"
        );
    }

    #[test]
    fn test_rate_limits() {
        assert!(rate_limits::FREE_MODELS.rpm < rate_limits::PAID_MODELS.rpm);
        assert!(rate_limits::FREE_MODELS.tpm < rate_limits::PAID_MODELS.tpm);
    }

    #[test]
    fn test_pricing() {
        assert_eq!(pricing::FREE_MODELS, 0.0);
        assert!(pricing::input::QWEN_TURBO < pricing::output::QWEN_TURBO);
        assert!(pricing::input::GEMINI_FLASH_THINKING < pricing::output::GEMINI_FLASH_THINKING);
    }
}
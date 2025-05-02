use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use std::collections::HashMap;

/// Model trait categorization and optimization system
#[derive(Debug, Clone, PartialEq)]
pub enum ModelTier {
    Free,
    Standard,
    Premium,
    Turbo,
}

#[derive(Debug, Clone)]
pub struct ModelTraits {
    pub tier: ModelTier,
    pub context_window: usize,
    pub training_cutoff: String,
    pub can_stream: bool,
    pub can_batch: bool,
    pub supports_tools: bool,
    pub typical_latency: Duration,
    pub cost_per_1m_tokens_input: f64,
    pub cost_per_1m_tokens_output: f64,
}

pub struct ModelStats {
    requests: Arc<RwLock<HashMap<String, Vec<Instant>>>>,
    tokens_used: Arc<RwLock<HashMap<String, Vec<(Instant, usize)>>>>,
}

impl ModelStats {
    pub fn new() -> Self {
        Self {
            requests: Arc::new(RwLock::new(HashMap::new())),
            tokens_used: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn record_request(&self, model_id: &str) {
        if let Ok(mut requests) = self.requests.write() {
            let model_requests = requests.entry(model_id.to_string())
                .or_insert_with(Vec::new);
            model_requests.push(Instant::now());
            
            // Clean up old entries (>1 hour)
            let one_hour_ago = Instant::now() - Duration::from_secs(3600);
            model_requests.retain(|&time| time > one_hour_ago);
        }
    }

    pub fn record_tokens(&self, model_id: &str, token_count: usize) {
        if let Ok(mut tokens) = self.tokens_used.write() {
            let model_tokens = tokens.entry(model_id.to_string())
                .or_insert_with(Vec::new);
            model_tokens.push((Instant::now(), token_count));
            
            // Clean up old entries (>1 hour)
            let one_hour_ago = Instant::now() - Duration::from_secs(3600);
            model_tokens.retain(|(time, _)| *time > one_hour_ago);
        }
    }

    pub fn get_rpm(&self, model_id: &str) -> usize {
        if let Ok(requests) = self.requests.read() {
            if let Some(model_requests) = requests.get(model_id) {
                let one_minute_ago = Instant::now() - Duration::from_secs(60);
                return model_requests.iter()
                    .filter(|&&time| time > one_minute_ago)
                    .count();
            }
        }
        0
    }

    pub fn get_tpm(&self, model_id: &str) -> usize {
        if let Ok(tokens) = self.tokens_used.read() {
            if let Some(model_tokens) = tokens.get(model_id) {
                let one_minute_ago = Instant::now() - Duration::from_secs(60);
                return model_tokens.iter()
                    .filter(|(time, _)| *time > one_minute_ago)
                    .map(|(_, count)| count)
                    .sum();
            }
        }
        0
    }
}

/// Trait implementations for specific models
impl ModelTraits {
    pub fn gemini_flash_free() -> Self {
        Self {
            tier: ModelTier::Free,
            context_window: 1_000_000,
            training_cutoff: "2024-03".to_string(),
            can_stream: true,
            can_batch: true,
            supports_tools: true,
            typical_latency: Duration::from_millis(500),
            cost_per_1m_tokens_input: 0.0,
            cost_per_1m_tokens_output: 0.0,
        }
    }

    pub fn gemini_pro_exp() -> Self {
        Self {
            tier: ModelTier::Premium,
            context_window: 1_000_000,
            training_cutoff: "2024-03".to_string(),
            can_stream: true,
            can_batch: true,
            supports_tools: true,
            typical_latency: Duration::from_millis(300),
            cost_per_1m_tokens_input: 0.0,
            cost_per_1m_tokens_output: 0.0,
        }
    }

    pub fn qwen_turbo() -> Self {
        Self {
            tier: ModelTier::Turbo,
            context_window: 1_000_000,
            training_cutoff: "2024-03".to_string(),
            can_stream: true,
            can_batch: true,
            supports_tools: true,
            typical_latency: Duration::from_millis(200),
            cost_per_1m_tokens_input: 0.05,
            cost_per_1m_tokens_output: 0.20,
        }
    }

    pub fn llama_scout() -> Self {
        Self {
            tier: ModelTier::Standard,
            context_window: 128_000,
            training_cutoff: "2024-03".to_string(),
            can_stream: true,
            can_batch: true,
            supports_tools: true,
            typical_latency: Duration::from_millis(400),
            cost_per_1m_tokens_input: 0.11,
            cost_per_1m_tokens_output: 0.34,
        }
    }
}

/// Smart request routing system
#[derive(Debug)]
pub struct RequestRouter {
    stats: ModelStats,
    fallback_chain: Vec<String>,
}

impl RequestRouter {
    pub fn new(fallback_models: Vec<String>) -> Self {
        Self {
            stats: ModelStats::new(),
            fallback_chain: fallback_models,
        }
    }

    pub fn select_model(&self, requested_model: &str, token_count: usize) -> Option<String> {
        // If the requested model is available and within limits, use it
        if self.is_model_available(requested_model, token_count) {
            return Some(requested_model.to_string());
        }

        // Try fallback models
        for model in &self.fallback_chain {
            if self.is_model_available(model, token_count) {
                return Some(model.clone());
            }
        }

        None
    }

    fn is_model_available(&self, model_id: &str, token_count: usize) -> bool {
        let rpm = self.stats.get_rpm(model_id);
        let tpm = self.stats.get_tpm(model_id);

        // Check against rate limits based on model tier
        match model_id {
            id if id.contains("free") => {
                rpm < 30 && (tpm + token_count) <= 50_000
            },
            _ => {
                rpm < 60 && (tpm + token_count) <= 100_000
            }
        }
    }

    pub fn record_usage(&self, model_id: &str, token_count: usize) {
        self.stats.record_request(model_id);
        self.stats.record_tokens(model_id, token_count);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_model_traits() {
        let model = ModelTraits::qwen_turbo();
        assert_eq!(model.tier, ModelTier::Turbo);
        assert!(model.supports_tools);
        assert!(model.cost_per_1m_tokens_output > model.cost_per_1m_tokens_input);
    }

    #[test]
    fn test_request_router() {
        let router = RequestRouter::new(vec![
            "google/gemini-2.0-flash-exp:free".to_string(),
            "qwen/qwen-turbo".to_string(),
        ]);

        // Test normal usage
        assert!(router.is_model_available("qwen/qwen-turbo", 1000));
        
        // Test rate limiting
        for _ in 0..65 {
            router.record_usage("qwen/qwen-turbo", 1000);
        }
        assert!(!router.is_model_available("qwen/qwen-turbo", 1000));
    }

    #[test]
    fn test_model_stats() {
        let stats = ModelStats::new();
        let model = "test-model";

        // Record some usage
        stats.record_request(model);
        stats.record_tokens(model, 1000);

        // Check immediate stats
        assert_eq!(stats.get_rpm(model), 1);
        assert_eq!(stats.get_tpm(model), 1000);

        // Test cleanup of old entries
        thread::sleep(Duration::from_secs(61));
        assert_eq!(stats.get_rpm(model), 0);
        assert_eq!(stats.get_tpm(model), 0);
    }
}
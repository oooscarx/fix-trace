use std::{fs, path::Path};

use serde::{Deserialize, Serialize};

use crate::error::AppError;

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct FixTraceConfig {
    pub model: ModelConfig,
    pub pricing: PricingConfig,
    pub budget: BudgetConfig,
    pub replay: ReplayConfig,
}

impl FixTraceConfig {
    pub fn load(path: &Path) -> Result<Self, AppError> {
        let source = fs::read_to_string(path).map_err(|source| AppError::ReadConfig {
            path: path.to_path_buf(),
            source,
        })?;
        let config: Self = toml::from_str(&source)?;
        config.validate()?;
        Ok(config)
    }

    pub fn to_toml(&self) -> Result<String, AppError> {
        Ok(toml::to_string_pretty(self)?)
    }

    fn validate(&self) -> Result<(), AppError> {
        if self.model.endpoint.trim().is_empty() {
            return Err(AppError::InvalidConfig(
                "model.endpoint cannot be empty".to_owned(),
            ));
        }
        if self.model.api_key_env.trim().is_empty() {
            return Err(AppError::InvalidConfig(
                "model.api_key_env cannot be empty".to_owned(),
            ));
        }
        if self.model.context_length == 0 || self.model.max_agent_steps == 0 {
            return Err(AppError::InvalidConfig(
                "model context length and max agent steps must be positive".to_owned(),
            ));
        }
        if !self.pricing.input_per_million_usd.is_finite()
            || !self.pricing.output_per_million_usd.is_finite()
            || self.pricing.input_per_million_usd < 0.0
            || self.pricing.output_per_million_usd < 0.0
        {
            return Err(AppError::InvalidConfig(
                "model prices must be finite and non-negative".to_owned(),
            ));
        }
        if self.budget.max_total_tokens == 0
            || !self.budget.max_cost_usd.is_finite()
            || self.budget.max_cost_usd < 0.0
        {
            return Err(AppError::InvalidConfig(
                "budget values must be positive and finite".to_owned(),
            ));
        }
        if self.replay.repetitions == 0 || self.replay.oracle_timeout_secs == 0 {
            return Err(AppError::InvalidConfig(
                "replay repetitions and timeout must be positive".to_owned(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct ModelConfig {
    pub provider: String,
    pub endpoint: String,
    pub api_key_env: String,
    pub model: String,
    pub api_style: String,
    pub context_length: u64,
    pub reasoning_mode: String,
    pub max_agent_steps: usize,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            provider: "openai-compatible".to_owned(),
            endpoint: "https://api.openai.com/v1".to_owned(),
            api_key_env: "FIXTRACE_API_KEY".to_owned(),
            model: "gpt-5-mini".to_owned(),
            api_style: "chat-completions".to_owned(),
            context_length: 32_768,
            reasoning_mode: "medium".to_owned(),
            max_agent_steps: 12,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct PricingConfig {
    pub input_per_million_usd: f64,
    pub output_per_million_usd: f64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct BudgetConfig {
    pub max_total_tokens: u64,
    pub max_cost_usd: f64,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            max_total_tokens: 100_000,
            max_cost_usd: 1.0,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct ReplayConfig {
    pub repetitions: u32,
    pub oracle_timeout_secs: u64,
    pub include_target: bool,
}

impl Default for ReplayConfig {
    fn default() -> Self {
        Self {
            repetitions: 3,
            oracle_timeout_secs: 120,
            include_target: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FixTraceConfig;

    #[test]
    fn default_config_round_trips_through_toml() {
        let expected = FixTraceConfig::default();
        let encoded = expected.to_toml().expect("default config should serialize");
        let decoded: FixTraceConfig =
            toml::from_str(&encoded).expect("serialized default should parse");

        assert_eq!(decoded, expected);
    }

    #[test]
    fn config_contains_required_course_settings() {
        let config = FixTraceConfig::default();

        assert_eq!(config.model.api_key_env, "FIXTRACE_API_KEY");
        assert_eq!(config.model.context_length, 32_768);
        assert_eq!(config.model.reasoning_mode, "medium");
        assert_eq!(config.budget.max_total_tokens, 100_000);
    }
}

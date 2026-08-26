use serde::{Deserialize, Serialize};

use crate::config::{BudgetConfig, PricingConfig};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl Usage {
    pub const fn total_tokens(self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum UsageObservation {
    Known { usage: Usage },
    Unknown,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct UsageSummary {
    pub calls: u64,
    pub unknown_usage_calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub input_cost_usd: f64,
    pub output_cost_usd: f64,
    pub total_cost_usd: f64,
}

impl UsageSummary {
    pub fn record(&mut self, observation: UsageObservation, pricing: &PricingConfig) {
        self.calls = self.calls.saturating_add(1);
        match observation {
            UsageObservation::Known { usage } => {
                self.input_tokens = self.input_tokens.saturating_add(usage.input_tokens);
                self.output_tokens = self.output_tokens.saturating_add(usage.output_tokens);
                self.total_tokens = self.total_tokens.saturating_add(usage.total_tokens());
                self.input_cost_usd +=
                    token_cost(usage.input_tokens, pricing.input_per_million_usd);
                self.output_cost_usd +=
                    token_cost(usage.output_tokens, pricing.output_per_million_usd);
                self.total_cost_usd = self.input_cost_usd + self.output_cost_usd;
            }
            UsageObservation::Unknown => {
                self.unknown_usage_calls = self.unknown_usage_calls.saturating_add(1);
            }
        }
    }

    pub fn exceeds(&self, budget: &BudgetConfig) -> bool {
        self.total_tokens >= budget.max_total_tokens || self.total_cost_usd >= budget.max_cost_usd
    }
}

pub fn token_cost(tokens: u64, price_per_million_usd: f64) -> f64 {
    (tokens as f64 / 1_000_000.0) * price_per_million_usd
}

#[cfg(test)]
mod tests {
    use super::{Usage, UsageObservation, UsageSummary, token_cost};
    use crate::config::PricingConfig;

    #[test]
    fn token_cost_formula_matches_configured_per_million_price() {
        assert!((token_cost(250_000, 2.0) - 0.5).abs() < f64::EPSILON);
        let mut summary = UsageSummary::default();
        summary.record(
            UsageObservation::Known {
                usage: Usage {
                    input_tokens: 1_000_000,
                    output_tokens: 500_000,
                },
            },
            &PricingConfig {
                input_per_million_usd: 1.5,
                output_per_million_usd: 4.0,
            },
        );
        assert!((summary.total_cost_usd - 3.5).abs() < f64::EPSILON);
    }
}

use crate::session::Session;
use std::fmt;

const DEFAULT_INPUT_COST_PER_MILLION: f64 = 15.0;
const DEFAULT_OUTPUT_COST_PER_MILLION: f64 = 75.0;
const DEFAULT_CACHE_CREATION_COST_PER_MILLION: f64 = 18.75;
const DEFAULT_CACHE_READ_COST_PER_MILLION: f64 = 1.5;

/// Per-million-token pricing used for cost estimation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPricing {
    pub input_cost_per_million: f64,
    pub output_cost_per_million: f64,
    pub cache_creation_cost_per_million: f64,
    pub cache_read_cost_per_million: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PricingSource {
    ExplicitProfile,
    ProviderCatalog,
    ImportedProviderCatalog,
    ReferenceEstimate,
    Unknown,
}

impl fmt::Display for PricingSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ExplicitProfile => "explicit-profile",
            Self::ProviderCatalog => "provider-catalog",
            Self::ImportedProviderCatalog => "imported-provider-catalog",
            Self::ReferenceEstimate => "reference-estimate",
            Self::Unknown => "unknown",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PricingResolution {
    pub actual: Option<ModelPricing>,
    pub reference: Option<ModelPricing>,
    pub source: PricingSource,
}

/// Resolves cost for an execution resource, never for a model name alone.
/// `explicit_actual` is the user-authoritative profile price. Reference data
/// is returned separately and never makes `actual` known.
#[must_use]
pub fn resolve_pricing(
    provider: &str,
    _endpoint: Option<&str>,
    model: &str,
    explicit_actual: Option<ModelPricing>,
    reference: Option<ModelPricing>,
) -> PricingResolution {
    if let Some(actual) = explicit_actual {
        return PricingResolution {
            actual: Some(actual),
            reference,
            source: PricingSource::ExplicitProfile,
        };
    }
    if let Some(actual) = provider_catalog_pricing(provider, model) {
        return PricingResolution {
            actual: Some(actual),
            reference,
            source: PricingSource::ProviderCatalog,
        };
    }
    if reference.is_some() {
        return PricingResolution {
            actual: None,
            reference,
            source: PricingSource::ReferenceEstimate,
        };
    }
    PricingResolution {
        actual: None,
        reference: None,
        source: PricingSource::Unknown,
    }
}

#[must_use]
pub fn provider_catalog_pricing(provider: &str, model: &str) -> Option<ModelPricing> {
    let provider = provider.trim().to_ascii_lowercase();
    if provider == "openai" {
        return openai_catalog_pricing(model);
    }
    if provider == "anthropic" {
        return anthropic_catalog_pricing(model);
    }
    None
}

fn openai_catalog_pricing(model: &str) -> Option<ModelPricing> {
    let normalized = model.to_ascii_lowercase();
    let (input, output) = if normalized == "gpt-5.6-luna" {
        (0.20, 1.20)
    } else if normalized == "gpt-5.4-mini" {
        (0.75, 4.50)
    } else if normalized == "gpt-5.4-nano" {
        (0.20, 1.25)
    } else if normalized == "gpt-4o-mini" {
        (0.15, 0.60)
    } else {
        return None;
    };
    Some(ModelPricing {
        input_cost_per_million: input,
        output_cost_per_million: output,
        cache_creation_cost_per_million: 0.0,
        cache_read_cost_per_million: 0.0,
    })
}

fn anthropic_catalog_pricing(model: &str) -> Option<ModelPricing> {
    let normalized = model.to_ascii_lowercase();
    if normalized.contains("haiku") {
        Some(ModelPricing {
            input_cost_per_million: 1.0,
            output_cost_per_million: 5.0,
            cache_creation_cost_per_million: 1.25,
            cache_read_cost_per_million: 0.1,
        })
    } else if normalized.contains("opus") {
        Some(ModelPricing {
            input_cost_per_million: 15.0,
            output_cost_per_million: 75.0,
            cache_creation_cost_per_million: 18.75,
            cache_read_cost_per_million: 1.5,
        })
    } else if normalized.contains("sonnet") {
        Some(ModelPricing::default_sonnet_tier())
    } else {
        None
    }
}

impl ModelPricing {
    #[must_use]
    pub const fn default_sonnet_tier() -> Self {
        Self {
            input_cost_per_million: DEFAULT_INPUT_COST_PER_MILLION,
            output_cost_per_million: DEFAULT_OUTPUT_COST_PER_MILLION,
            cache_creation_cost_per_million: DEFAULT_CACHE_CREATION_COST_PER_MILLION,
            cache_read_cost_per_million: DEFAULT_CACHE_READ_COST_PER_MILLION,
        }
    }
}

/// Token counters accumulated for a conversation turn or session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_creation_input_tokens: u32,
    pub cache_read_input_tokens: u32,
}

/// Estimated dollar cost derived from a [`TokenUsage`] sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UsageCostEstimate {
    pub input_cost_usd: f64,
    pub output_cost_usd: f64,
    pub cache_creation_cost_usd: f64,
    pub cache_read_cost_usd: f64,
}

impl UsageCostEstimate {
    #[must_use]
    pub fn total_cost_usd(self) -> f64 {
        self.input_cost_usd
            + self.output_cost_usd
            + self.cache_creation_cost_usd
            + self.cache_read_cost_usd
    }
}

/// Returns pricing metadata for a known model alias or family.
#[must_use]
pub fn pricing_for_model(model: &str) -> Option<ModelPricing> {
    provider_catalog_pricing(
        if model.to_ascii_lowercase().contains("claude") {
            "anthropic"
        } else {
            "openai"
        },
        model,
    )
}

impl TokenUsage {
    #[must_use]
    pub fn total_tokens(self) -> u32 {
        self.input_tokens
            + self.output_tokens
            + self.cache_creation_input_tokens
            + self.cache_read_input_tokens
    }

    #[must_use]
    pub fn estimate_cost_usd(self) -> UsageCostEstimate {
        self.estimate_cost_usd_with_pricing(ModelPricing::default_sonnet_tier())
    }

    #[must_use]
    pub fn estimate_cost_usd_with_pricing(self, pricing: ModelPricing) -> UsageCostEstimate {
        UsageCostEstimate {
            input_cost_usd: cost_for_tokens(self.input_tokens, pricing.input_cost_per_million),
            output_cost_usd: cost_for_tokens(self.output_tokens, pricing.output_cost_per_million),
            cache_creation_cost_usd: cost_for_tokens(
                self.cache_creation_input_tokens,
                pricing.cache_creation_cost_per_million,
            ),
            cache_read_cost_usd: cost_for_tokens(
                self.cache_read_input_tokens,
                pricing.cache_read_cost_per_million,
            ),
        }
    }

    #[must_use]
    pub fn summary_lines(self, label: &str) -> Vec<String> {
        self.summary_lines_for_model(label, None)
    }

    #[must_use]
    pub fn summary_lines_for_model(self, label: &str, model: Option<&str>) -> Vec<String> {
        let pricing = model.and_then(pricing_for_model);
        let model_suffix =
            model.map_or_else(String::new, |model_name| format!(" model={model_name}"));
        let Some(pricing) = pricing else {
            return vec![
                format!(
                    "{label}: total_tokens={} input={} output={} cache_write={} cache_read={} estimated_cost=unknown{}",
                    self.total_tokens(),
                    self.input_tokens,
                    self.output_tokens,
                    self.cache_creation_input_tokens,
                    self.cache_read_input_tokens,
                    model_suffix,
                ),
                "  cost breakdown: unavailable (actual execution pricing is unknown)".to_string(),
            ];
        };
        let cost = self.estimate_cost_usd_with_pricing(pricing);
        vec![
            format!(
                "{label}: total_tokens={} input={} output={} cache_write={} cache_read={} estimated_cost={}{}",
                self.total_tokens(),
                self.input_tokens,
                self.output_tokens,
                self.cache_creation_input_tokens,
                self.cache_read_input_tokens,
                format_usd(cost.total_cost_usd()),
                model_suffix,
            ),
            format!(
                "  cost breakdown: input={} output={} cache_write={} cache_read={}",
                format_usd(cost.input_cost_usd),
                format_usd(cost.output_cost_usd),
                format_usd(cost.cache_creation_cost_usd),
                format_usd(cost.cache_read_cost_usd),
            ),
        ]
    }
}

fn cost_for_tokens(tokens: u32, usd_per_million_tokens: f64) -> f64 {
    f64::from(tokens) / 1_000_000.0 * usd_per_million_tokens
}

#[must_use]
/// Formats a dollar-denominated value for CLI display.
pub fn format_usd(amount: f64) -> String {
    format!("${amount:.4}")
}

/// Aggregates token usage across a running session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UsageTracker {
    latest_turn: TokenUsage,
    cumulative: TokenUsage,
    turns: u32,
}

impl UsageTracker {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn from_session(session: &Session) -> Self {
        let mut tracker = Self::new();
        for message in &session.messages {
            if let Some(usage) = message.usage {
                tracker.record(usage);
            }
        }
        tracker
    }

    pub fn record(&mut self, usage: TokenUsage) {
        self.latest_turn = usage;
        self.cumulative.input_tokens += usage.input_tokens;
        self.cumulative.output_tokens += usage.output_tokens;
        self.cumulative.cache_creation_input_tokens += usage.cache_creation_input_tokens;
        self.cumulative.cache_read_input_tokens += usage.cache_read_input_tokens;
        self.turns += 1;
    }

    #[must_use]
    pub fn current_turn_usage(&self) -> TokenUsage {
        self.latest_turn
    }

    #[must_use]
    pub fn cumulative_usage(&self) -> TokenUsage {
        self.cumulative
    }

    #[must_use]
    pub fn turns(&self) -> u32 {
        self.turns
    }
}

#[cfg(test)]
mod tests {
    use super::{
        format_usd, pricing_for_model, resolve_pricing, ModelPricing, PricingSource, TokenUsage,
        UsageTracker,
    };
    use crate::session::{ContentBlock, ConversationMessage, MessageRole, Session};

    #[test]
    fn tracks_true_cumulative_usage() {
        let mut tracker = UsageTracker::new();
        tracker.record(TokenUsage {
            input_tokens: 10,
            output_tokens: 4,
            cache_creation_input_tokens: 2,
            cache_read_input_tokens: 1,
        });
        tracker.record(TokenUsage {
            input_tokens: 20,
            output_tokens: 6,
            cache_creation_input_tokens: 3,
            cache_read_input_tokens: 2,
        });

        assert_eq!(tracker.turns(), 2);
        assert_eq!(tracker.current_turn_usage().input_tokens, 20);
        assert_eq!(tracker.current_turn_usage().output_tokens, 6);
        assert_eq!(tracker.cumulative_usage().output_tokens, 10);
        assert_eq!(tracker.cumulative_usage().input_tokens, 30);
        assert_eq!(tracker.cumulative_usage().total_tokens(), 48);
    }

    #[test]
    fn computes_cost_summary_lines() {
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 500_000,
            cache_creation_input_tokens: 100_000,
            cache_read_input_tokens: 200_000,
        };

        let cost = usage.estimate_cost_usd();
        assert_eq!(format_usd(cost.input_cost_usd), "$15.0000");
        assert_eq!(format_usd(cost.output_cost_usd), "$37.5000");
        let lines = usage.summary_lines_for_model("usage", Some("claude-sonnet-4-20250514"));
        assert!(lines[0].contains("estimated_cost=$54.6750"));
        assert!(lines[0].contains("model=claude-sonnet-4-20250514"));
        assert!(lines[1].contains("cache_read=$0.3000"));
    }

    #[test]
    fn supports_model_specific_pricing() {
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 500_000,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        };

        let haiku = pricing_for_model("claude-haiku-4-5-20251001").expect("haiku pricing");
        let opus = pricing_for_model("claude-opus-4-6").expect("opus pricing");
        let haiku_cost = usage.estimate_cost_usd_with_pricing(haiku);
        let opus_cost = usage.estimate_cost_usd_with_pricing(opus);
        assert_eq!(format_usd(haiku_cost.total_cost_usd()), "$3.5000");
        assert_eq!(format_usd(opus_cost.total_cost_usd()), "$52.5000");
    }

    #[test]
    fn supports_configured_openai_model_pricing() {
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            ..TokenUsage::default()
        };
        let mini = pricing_for_model("gpt-5.4-mini").expect("mini pricing");
        let cost = usage.estimate_cost_usd_with_pricing(mini);
        assert_eq!(format_usd(cost.input_cost_usd), "$0.7500");
        assert_eq!(format_usd(cost.output_cost_usd), "$4.5000");
        assert_eq!(format_usd(cost.total_cost_usd()), "$5.2500");
    }

    #[test]
    fn marks_unknown_model_pricing_as_unknown() {
        let usage = TokenUsage {
            input_tokens: 100,
            output_tokens: 100,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        };
        let lines = usage.summary_lines_for_model("usage", Some("custom-model"));
        assert!(lines[0].contains("estimated_cost=unknown"));
        assert!(lines[1].contains("pricing is unknown"));
    }

    #[test]
    fn explicit_profile_price_beats_provider_catalog() {
        let explicit = ModelPricing {
            input_cost_per_million: 0.123,
            output_cost_per_million: 0.456,
            cache_creation_cost_per_million: 0.0,
            cache_read_cost_per_million: 0.0,
        };
        let resolution = resolve_pricing("openai", None, "gpt-5.4-mini", Some(explicit), None);
        assert_eq!(resolution.source, PricingSource::ExplicitProfile);
        assert_eq!(resolution.actual, Some(explicit));
    }

    #[test]
    fn provider_catalog_requires_provider_identity() {
        let together = ModelPricing {
            input_cost_per_million: 1.0,
            output_cost_per_million: 2.0,
            cache_creation_cost_per_million: 0.0,
            cache_read_cost_per_million: 0.0,
        };
        let fireworks = ModelPricing {
            input_cost_per_million: 3.0,
            output_cost_per_million: 4.0,
            cache_creation_cost_per_million: 0.0,
            cache_read_cost_per_million: 0.0,
        };
        let together_resolution =
            resolve_pricing("together", None, "model-x", Some(together), None);
        let fireworks_resolution =
            resolve_pricing("fireworks", None, "model-x", Some(fireworks), None);
        assert_ne!(together_resolution.actual, fireworks_resolution.actual);
    }

    #[test]
    fn reference_price_is_not_actual_price() {
        let reference = ModelPricing {
            input_cost_per_million: 0.5,
            output_cost_per_million: 1.0,
            cache_creation_cost_per_million: 0.0,
            cache_read_cost_per_million: 0.0,
        };
        let resolution = resolve_pricing("ollama", None, "qwen", None, Some(reference));
        assert_eq!(resolution.source, PricingSource::ReferenceEstimate);
        assert!(resolution.actual.is_none());
        assert_eq!(resolution.reference, Some(reference));
    }

    #[test]
    fn unknown_provider_model_remains_unknown() {
        let resolution = resolve_pricing("unknown-provider", None, "unknown-model", None, None);
        assert_eq!(resolution.source, PricingSource::Unknown);
        assert!(resolution.actual.is_none());
    }

    #[test]
    fn reconstructs_usage_from_session_messages() {
        let mut session = Session::new();
        session.messages = vec![ConversationMessage {
            role: MessageRole::Assistant,
            blocks: vec![ContentBlock::Text {
                text: "done".to_string(),
            }],
            usage: Some(TokenUsage {
                input_tokens: 5,
                output_tokens: 2,
                cache_creation_input_tokens: 1,
                cache_read_input_tokens: 0,
            }),
        }];

        let tracker = UsageTracker::from_session(&session);
        assert_eq!(tracker.turns(), 1);
        assert_eq!(tracker.cumulative_usage().total_tokens(), 8);
    }
}

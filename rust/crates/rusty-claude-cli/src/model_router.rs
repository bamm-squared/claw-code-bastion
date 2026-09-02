//! User-controlled model resources and deterministic capability routing.
//!
//! This module selects a cognitive resource; it does not construct provider
//! clients, grant permissions, or make network requests. Provider dispatch
//! remains owned by the existing runtime.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ModelRole {
    Writer,
    Evaluator,
    Planner,
    Other,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum PrivacyClass {
    Local,
    Confidential,
    Remote,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Capability {
    pub coding: u8,
    pub reasoning: u8,
    pub agent_tool_use: u8,
    pub planning: u8,
    pub evaluation: u8,
    pub context_window: u32,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[allow(clippy::struct_field_names)]
pub struct Pricing {
    /// Actual price in microdollars per million tokens.
    pub actual_cost_known: bool,
    pub actual_input_micros: u64,
    pub actual_output_micros: u64,
    /// Optional comparison-only price, never used as actual spend.
    pub reference_input_micros: Option<u64>,
    pub reference_output_micros: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelProfile {
    pub id: String,
    pub provider: String,
    pub model: String,
    pub endpoint: Option<String>,
    pub reasoning_profile: Option<String>,
    pub privacy: PrivacyClass,
    pub capability: Capability,
    pub pricing: Pricing,
    pub expected_latency_ms: Option<u64>,
    pub observed_reliability_percent: Option<u8>,
    pub user_preference: i32,
    pub enabled: bool,
}

impl ModelProfile {
    #[must_use]
    pub fn unknown(
        id: impl Into<String>,
        provider: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            provider: provider.into(),
            model: model.into(),
            endpoint: None,
            reasoning_profile: None,
            privacy: PrivacyClass::Remote,
            capability: Capability {
                context_window: 8_192,
                ..Capability::default()
            },
            pricing: Pricing::default(),
            expected_latency_ms: None,
            observed_reliability_percent: None,
            user_preference: 0,
            enabled: true,
        }
    }

    #[must_use]
    pub fn legacy(model: impl Into<String>) -> Self {
        let model = model.into();
        let local = std::env::var_os("OLLAMA_HOST").is_some()
            || std::env::var("OPENAI_BASE_URL")
                .is_ok_and(|url| url.contains("localhost") || url.contains("127.0.0.1"));
        Self {
            id: "legacy-default".to_string(),
            provider: "configured".to_string(),
            model,
            endpoint: None,
            reasoning_profile: None,
            privacy: if local {
                PrivacyClass::Local
            } else {
                PrivacyClass::Remote
            },
            capability: Capability {
                coding: 100,
                reasoning: 100,
                agent_tool_use: 100,
                planning: 100,
                evaluation: 100,
                context_window: 200_000,
            },
            pricing: Pricing {
                actual_cost_known: false,
                ..Pricing::default()
            },
            expected_latency_ms: None,
            observed_reliability_percent: None,
            user_preference: 0,
            enabled: true,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelPool {
    pub profiles: Vec<ModelProfile>,
}

impl ModelPool {
    #[must_use]
    pub fn one(profile: ModelProfile) -> Self {
        Self {
            profiles: vec![profile],
        }
    }

    /// Reads the optional user pool from merged settings. Invalid optional
    /// entries are ignored, preserving the legacy model fallback rather than
    /// making provider startup or authority decisions from partial metadata.
    #[must_use]
    pub fn from_runtime_config(config: &runtime::RuntimeConfig, legacy_model: &str) -> Self {
        let json = config.as_json().render();
        let value = serde_json::from_str::<Value>(&json).ok();
        let profiles = value
            .as_ref()
            .and_then(|root| root.get("modelResources"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(parse_profile)
            .collect::<Vec<_>>();
        if profiles.is_empty() {
            Self::one(ModelProfile::legacy(legacy_model))
        } else {
            Self { profiles }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct RoutingPolicy {
    pub local_only: bool,
    pub allow_remote: bool,
    pub allow_confidential: bool,
    pub preferred_provider: Option<String>,
    pub forced_profile: Option<String>,
    pub minimum_margin: u8,
    pub disable_automatic: bool,
}

impl Default for RoutingPolicy {
    fn default() -> Self {
        Self {
            local_only: false,
            allow_remote: true,
            allow_confidential: true,
            preferred_provider: None,
            forced_profile: None,
            minimum_margin: 8,
            disable_automatic: false,
        }
    }
}

impl RoutingPolicy {
    #[must_use]
    pub fn from_runtime_config(config: &runtime::RuntimeConfig) -> Self {
        let mut policy = Self::default();
        let value = serde_json::from_str::<Value>(&config.as_json().render()).ok();
        let Some(root) = value.as_ref().and_then(Value::as_object) else {
            return policy;
        };
        let Some(settings) = root.get("routing").and_then(Value::as_object) else {
            return policy;
        };
        policy.local_only = bool_field(settings, "localOnly", policy.local_only);
        policy.allow_remote = bool_field(settings, "allowRemote", policy.allow_remote);
        policy.allow_confidential =
            bool_field(settings, "allowConfidential", policy.allow_confidential);
        policy.disable_automatic =
            bool_field(settings, "disableAutomatic", policy.disable_automatic);
        policy.preferred_provider = string_field(settings, "preferredProvider");
        policy.forced_profile = string_field(settings, "forcedProfile");
        policy.minimum_margin = settings
            .get("minimumMargin")
            .and_then(Value::as_u64)
            .and_then(|value| u8::try_from(value).ok())
            .unwrap_or(policy.minimum_margin);
        policy
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskSignals {
    pub ambiguity: u8,
    pub impacted_modules: u8,
    pub dependency_depth: u8,
    pub security_sensitive: bool,
    pub concurrency_sensitive: bool,
    pub public_api_change: bool,
    pub unresolved_relationships: u8,
    pub expected_input_tokens: u32,
    pub expected_output_tokens: u32,
    pub context_window: u32,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CapabilityRequirement {
    pub coding: u8,
    pub reasoning: u8,
    pub agent_tool_use: u8,
    pub planning: u8,
    pub evaluation: u8,
    pub context_window: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DifficultyEstimate {
    pub role: ModelRole,
    pub requirement: CapabilityRequirement,
    pub safety_margin: u8,
    pub rationale: DifficultyRationale,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct DifficultyRationale {
    pub ambiguity: u8,
    pub scope: u8,
    pub risk: u8,
    pub unresolved: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Rejection {
    pub profile_id: String,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RouteDecision {
    pub selected: Option<ModelProfile>,
    pub reason: String,
    pub rejections: Vec<Rejection>,
    pub estimate: DifficultyEstimate,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct EscalationPackage {
    pub original_requirement: String,
    pub task_plan: String,
    pub candidate_summary: String,
    pub expected_contracts: String,
    pub evaluation_findings: String,
    pub validation_evidence: String,
    pub repository_intelligence: String,
    pub unresolved_questions: Vec<String>,
}

pub struct ModelRouter;

impl ModelRouter {
    #[must_use]
    pub fn estimate(
        role: ModelRole,
        signals: TaskSignals,
        policy: &RoutingPolicy,
    ) -> DifficultyEstimate {
        let scope = signals
            .impacted_modules
            .saturating_mul(4)
            .saturating_add(signals.dependency_depth.saturating_mul(3));
        let risk = (u8::from(signals.security_sensitive) * 18)
            .saturating_add(u8::from(signals.concurrency_sensitive) * 18)
            .saturating_add(u8::from(signals.public_api_change) * 10);
        let unresolved = signals.unresolved_relationships.saturating_mul(5);
        let base = 25u8
            .saturating_add(signals.ambiguity / 2)
            .saturating_add(scope.min(30))
            .saturating_add(risk.min(30))
            .saturating_add(unresolved.min(20));
        let requirement = match role {
            ModelRole::Writer => CapabilityRequirement {
                coding: base,
                reasoning: base.saturating_sub(5),
                agent_tool_use: 35u8.saturating_add(scope.min(25)),
                planning: 25u8.saturating_add(signals.ambiguity / 3),
                evaluation: 15,
                context_window: signals.context_window.max(8_192),
            },
            ModelRole::Evaluator => CapabilityRequirement {
                coding: 15,
                reasoning: base,
                agent_tool_use: 10,
                planning: 20u8.saturating_add(signals.ambiguity / 3),
                evaluation: 35u8.saturating_add(risk.min(25)),
                context_window: signals.context_window.max(8_192),
            },
            ModelRole::Planner => CapabilityRequirement {
                coding: 15,
                reasoning: base,
                agent_tool_use: 15,
                planning: 35u8.saturating_add(signals.ambiguity / 3),
                evaluation: 20,
                context_window: signals.context_window.max(8_192),
            },
            ModelRole::Other => CapabilityRequirement {
                coding: base / 2,
                reasoning: base,
                agent_tool_use: 20,
                planning: 20,
                evaluation: 20,
                context_window: signals.context_window.max(8_192),
            },
        };
        DifficultyEstimate {
            role,
            requirement,
            safety_margin: policy.minimum_margin.max(8),
            rationale: DifficultyRationale {
                ambiguity: signals.ambiguity,
                scope,
                risk,
                unresolved,
            },
        }
    }

    #[must_use]
    pub fn route(
        pool: &ModelPool,
        role: ModelRole,
        signals: TaskSignals,
        policy: &RoutingPolicy,
    ) -> RouteDecision {
        let estimate = Self::estimate(role, signals, policy);
        let mut eligible = Vec::new();
        let mut rejections = Vec::new();
        for profile in &pool.profiles {
            if !profile.enabled {
                rejections.push(reject(profile, "disabled by user configuration"));
            } else if let Some(forced) = &policy.forced_profile {
                if &profile.id != forced {
                    rejections.push(reject(profile, "another profile is forced"));
                } else if !policy_allows(profile, policy) {
                    rejections.push(reject(
                        profile,
                        "forced profile is ineligible under privacy policy",
                    ));
                } else if !capable(profile, estimate) {
                    rejections.push(reject(
                        profile,
                        "forced profile does not clear capability threshold",
                    ));
                } else {
                    eligible.push(profile);
                }
            } else if !policy_allows(profile, policy) {
                rejections.push(reject(profile, "ineligible under privacy policy"));
            } else if !capable(profile, estimate) {
                rejections.push(reject(profile, "below capability threshold"));
            } else {
                eligible.push(profile);
            }
        }
        eligible.sort_by_key(|profile| {
            (
                estimated_cost(profile, signals),
                u8::from(
                    policy
                        .preferred_provider
                        .as_deref()
                        .is_some_and(|provider| profile.provider != provider),
                ),
                profile.expected_latency_ms.unwrap_or(u64::MAX),
                u8::MAX - profile.observed_reliability_percent.unwrap_or(50),
                -profile.user_preference,
            )
        });
        let selected = eligible.first().map(|profile| (*profile).clone());
        let reason = selected.as_ref().map_or_else(
            || "No enabled, policy-eligible profile clears the capability threshold.".to_string(),
            |profile| format!(
                "Selected {}: policy eligible, capability clears threshold, lowest expected actual cost among eligible profiles.",
                profile.id
            ),
        );
        RouteDecision {
            selected,
            reason,
            rejections,
            estimate,
        }
    }

    #[must_use]
    pub fn escalation(
        pool: &ModelPool,
        role: ModelRole,
        signals: TaskSignals,
        policy: RoutingPolicy,
    ) -> RouteDecision {
        let stronger = RoutingPolicy {
            minimum_margin: policy.minimum_margin.saturating_add(15),
            ..policy
        };
        Self::route(pool, role, signals, &stronger)
    }
}

fn reject(profile: &ModelProfile, reason: &str) -> Rejection {
    Rejection {
        profile_id: profile.id.clone(),
        reason: reason.to_string(),
    }
}

fn bool_field(settings: &serde_json::Map<String, Value>, name: &str, default: bool) -> bool {
    settings
        .get(name)
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

fn string_field(settings: &serde_json::Map<String, Value>, name: &str) -> Option<String> {
    settings
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn parse_profile(value: &Value) -> Option<ModelProfile> {
    let object = value.as_object()?;
    let model = object.get("model")?.as_str()?.trim();
    if model.is_empty() {
        return None;
    }
    let capability = object.get("capability").and_then(Value::as_object);
    let pricing = object.get("pricing").and_then(Value::as_object);
    let privacy = match object
        .get("privacy")
        .and_then(Value::as_str)
        .unwrap_or("remote")
        .to_ascii_lowercase()
        .as_str()
    {
        "local" => PrivacyClass::Local,
        "confidential" => PrivacyClass::Confidential,
        _ => PrivacyClass::Remote,
    };
    let profile = ModelProfile {
        id: object
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or(model)
            .to_string(),
        provider: object
            .get("provider")
            .and_then(Value::as_str)
            .unwrap_or("configured")
            .to_string(),
        model: model.to_string(),
        endpoint: object
            .get("endpoint")
            .and_then(Value::as_str)
            .map(str::to_string),
        reasoning_profile: object
            .get("reasoningProfile")
            .and_then(Value::as_str)
            .map(str::to_string),
        privacy,
        capability: Capability {
            coding: number_field(capability, "coding", 50),
            reasoning: number_field(capability, "reasoning", 50),
            agent_tool_use: number_field(capability, "agentToolUse", 50),
            planning: number_field(capability, "planning", 50),
            evaluation: number_field(capability, "evaluation", 50),
            context_window: number_field_u32(capability, "contextWindow", 8_192),
        },
        pricing: Pricing {
            actual_cost_known: pricing
                .and_then(|value| value.get("actualCostKnown"))
                .and_then(Value::as_bool)
                .unwrap_or(pricing.is_some_and(|value| {
                    value.get("actualInputMicros").is_some()
                        || value.get("actualOutputMicros").is_some()
                })),
            actual_input_micros: number_field_u64(pricing, "actualInputMicros", 0),
            actual_output_micros: number_field_u64(pricing, "actualOutputMicros", 0),
            reference_input_micros: pricing
                .and_then(|value| value.get("referenceInputMicros"))
                .and_then(Value::as_u64),
            reference_output_micros: pricing
                .and_then(|value| value.get("referenceOutputMicros"))
                .and_then(Value::as_u64),
        },
        expected_latency_ms: object.get("expectedLatencyMs").and_then(Value::as_u64),
        observed_reliability_percent: object
            .get("observedReliabilityPercent")
            .and_then(Value::as_u64)
            .and_then(|value| u8::try_from(value).ok()),
        user_preference: object
            .get("userPreference")
            .and_then(Value::as_i64)
            .and_then(|value| i32::try_from(value).ok())
            .unwrap_or(0),
        enabled: object
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(true),
    };
    Some(profile)
}

fn number_field(source: Option<&serde_json::Map<String, Value>>, name: &str, default: u8) -> u8 {
    number_field_u64(source, name, u64::from(default))
        .try_into()
        .unwrap_or(default)
}

fn number_field_u32(
    source: Option<&serde_json::Map<String, Value>>,
    name: &str,
    default: u32,
) -> u32 {
    number_field_u64(source, name, u64::from(default))
        .try_into()
        .unwrap_or(default)
}

fn number_field_u64(
    source: Option<&serde_json::Map<String, Value>>,
    name: &str,
    default: u64,
) -> u64 {
    source
        .and_then(|value| value.get(name))
        .and_then(Value::as_u64)
        .unwrap_or(default)
}

fn policy_allows(profile: &ModelProfile, policy: &RoutingPolicy) -> bool {
    if policy.local_only && profile.privacy != PrivacyClass::Local {
        return false;
    }
    if !policy.allow_remote && profile.privacy == PrivacyClass::Remote {
        return false;
    }
    if !policy.allow_confidential && profile.privacy == PrivacyClass::Confidential {
        return false;
    }
    true
}

fn capable(profile: &ModelProfile, estimate: DifficultyEstimate) -> bool {
    let required = estimate.requirement;
    let margin = estimate.safety_margin;
    profile.capability.coding >= required.coding.saturating_add(margin)
        && profile.capability.reasoning >= required.reasoning.saturating_add(margin)
        && profile.capability.agent_tool_use >= required.agent_tool_use.saturating_add(margin)
        && profile.capability.planning >= required.planning.saturating_add(margin)
        && profile.capability.evaluation >= required.evaluation.saturating_add(margin)
        && profile.capability.context_window >= required.context_window
}

fn estimated_cost(profile: &ModelProfile, signals: TaskSignals) -> u64 {
    if !profile.pricing.actual_cost_known {
        return u64::MAX;
    }
    let input = u64::from(signals.expected_input_tokens);
    let output = u64::from(signals.expected_output_tokens);
    input * profile.pricing.actual_input_micros + output * profile.pricing.actual_output_micros
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(id: &str, privacy: PrivacyClass, capability: u8, cost: u64) -> ModelProfile {
        ModelProfile {
            id: id.to_string(),
            provider: "test".to_string(),
            model: id.to_string(),
            endpoint: None,
            reasoning_profile: Some("default".to_string()),
            privacy,
            capability: Capability {
                coding: capability,
                reasoning: capability,
                agent_tool_use: capability,
                planning: capability,
                evaluation: capability,
                context_window: 64_000,
            },
            pricing: Pricing {
                actual_cost_known: true,
                actual_input_micros: cost,
                actual_output_micros: cost,
                ..Pricing::default()
            },
            expected_latency_ms: Some(10),
            observed_reliability_percent: Some(90),
            user_preference: 0,
            enabled: true,
        }
    }

    fn signals() -> TaskSignals {
        TaskSignals {
            context_window: 64_000,
            expected_input_tokens: 1_000,
            expected_output_tokens: 1_000,
            ..TaskSignals::default()
        }
    }

    #[test]
    fn one_model_pool_routes_all_roles() {
        let pool = ModelPool::one(profile("only", PrivacyClass::Local, 100, 0));
        let policy = RoutingPolicy {
            allow_remote: false,
            allow_confidential: false,
            ..RoutingPolicy::default()
        };
        for role in [ModelRole::Writer, ModelRole::Evaluator, ModelRole::Planner] {
            assert_eq!(
                ModelRouter::route(&pool, role, signals(), &policy)
                    .selected
                    .unwrap()
                    .id,
                "only"
            );
        }
    }

    #[test]
    fn privacy_filters_before_capability_and_cost() {
        let pool = ModelPool {
            profiles: vec![
                profile("remote-cheap", PrivacyClass::Remote, 100, 0),
                profile("local", PrivacyClass::Local, 100, 20),
            ],
        };
        let decision = ModelRouter::route(
            &pool,
            ModelRole::Writer,
            signals(),
            &RoutingPolicy {
                local_only: true,
                ..RoutingPolicy::default()
            },
        );
        assert_eq!(decision.selected.unwrap().id, "local");
        assert!(decision
            .rejections
            .iter()
            .any(|item| item.profile_id == "remote-cheap"));
    }

    #[test]
    fn cheapest_capable_profile_wins() {
        let pool = ModelPool {
            profiles: vec![
                profile("strong", PrivacyClass::Local, 100, 50),
                profile("economical", PrivacyClass::Local, 100, 1),
            ],
        };
        let decision = ModelRouter::route(
            &pool,
            ModelRole::Writer,
            signals(),
            &RoutingPolicy::default(),
        );
        assert_eq!(decision.selected.unwrap().id, "economical");
    }

    #[test]
    fn evaluator_can_route_independently() {
        let mut writer = profile("writer", PrivacyClass::Local, 70, 1);
        writer.capability.evaluation = 30;
        let pool = ModelPool {
            profiles: vec![writer, profile("evaluator", PrivacyClass::Local, 100, 5)],
        };
        let policy = RoutingPolicy::default();
        assert_eq!(
            ModelRouter::route(&pool, ModelRole::Writer, signals(), &policy)
                .selected
                .unwrap()
                .id,
            "writer"
        );
        assert_eq!(
            ModelRouter::route(&pool, ModelRole::Evaluator, signals(), &policy)
                .selected
                .unwrap()
                .id,
            "evaluator"
        );
    }

    #[test]
    fn forced_profile_and_no_capable_result_are_explicit() {
        let pool = ModelPool::one(profile("weak", PrivacyClass::Local, 10, 0));
        let decision = ModelRouter::route(
            &pool,
            ModelRole::Writer,
            signals(),
            &RoutingPolicy {
                forced_profile: Some("weak".to_string()),
                ..RoutingPolicy::default()
            },
        );
        assert!(decision.selected.is_none());
        assert!(decision.reason.contains("No enabled"));
    }

    #[test]
    fn local_zero_cost_tie_uses_reliability_and_latency() {
        let mut slow = profile("slow", PrivacyClass::Local, 100, 0);
        slow.expected_latency_ms = Some(100);
        let fast = profile("fast", PrivacyClass::Local, 100, 0);
        let decision = ModelRouter::route(
            &ModelPool {
                profiles: vec![slow, fast],
            },
            ModelRole::Writer,
            signals(),
            &RoutingPolicy::default(),
        );
        assert_eq!(decision.selected.unwrap().id, "fast");
    }
}

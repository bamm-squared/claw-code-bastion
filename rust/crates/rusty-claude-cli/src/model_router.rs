//! User-controlled model resources and deterministic capability routing.
//!
//! This module selects a cognitive resource; it does not construct provider
//! clients, grant permissions, or make network requests. Provider dispatch
//! remains owned by the existing runtime.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

pub const CALIBRATION_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum CalibrationEvidenceKind {
    #[default]
    LocalCalibration,
    ObservedOutcome,
}

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

    /// Returns a pool whose capabilities include only the supplied evidence.
    /// The configured profile is never modified, so user priors remain
    /// inspectable and can be replaced without losing their provenance.
    #[must_use]
    pub fn with_calibration(&self, calibration: &CalibrationStore) -> Self {
        Self {
            profiles: self
                .profiles
                .iter()
                .map(|profile| calibration.effective_profile(profile))
                .collect(),
        }
    }
}

/// Stable identity for calibration evidence. A model name alone is not enough:
/// endpoint and reasoning settings can materially change behavior.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CalibrationIdentity {
    pub provider: String,
    pub model: String,
    pub endpoint: Option<String>,
    pub reasoning_profile: Option<String>,
}

impl CalibrationIdentity {
    #[must_use]
    pub fn from_profile(profile: &ModelProfile) -> Self {
        Self {
            provider: profile.provider.clone(),
            model: profile.model.clone(),
            endpoint: profile.endpoint.clone(),
            reasoning_profile: profile.reasoning_profile.clone(),
        }
    }
}

/// A compact, source-only observation. It deliberately contains no prompt,
/// source, candidate, or evaluator transcript.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct CalibrationObservation {
    pub profile_id: String,
    pub identity: CalibrationIdentity,
    pub role: ModelRole,
    pub corpus_version: String,
    pub bastion_version: String,
    pub runtime_identity: String,
    pub difficulty_bucket: u8,
    pub first_pass_success: bool,
    pub validation_passed: bool,
    pub evaluation_passed: Option<bool>,
    pub rework_required: bool,
    pub escalation_required: bool,
    pub elapsed_ms: Option<u64>,
    #[serde(default)]
    pub evidence_kind: CalibrationEvidenceKind,
    #[serde(default)]
    pub recorded_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct OutcomeMetadata {
    pub profile_id: String,
    pub identity: CalibrationIdentity,
    pub role: ModelRole,
    pub runtime_identity: String,
    pub difficulty_bucket: u8,
    pub first_pass_success: bool,
    pub validation_passed: bool,
    pub evaluation_passed: Option<bool>,
    pub rework_required: bool,
    pub escalation_required: bool,
    pub elapsed_ms: Option<u64>,
    pub recorded_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CalibrationCase {
    pub id: &'static str,
    pub role: ModelRole,
    pub difficulty_bucket: u8,
    pub description: &'static str,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CalibrationRunSummary {
    pub observations: Vec<CalibrationObservation>,
    pub first_pass_successes: usize,
    pub cases_run: usize,
    pub infrastructure_failures: usize,
}

pub type CalibrationCaseExecutor =
    dyn Fn(&CalibrationCase) -> Result<CalibrationCaseResult, String>;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct CalibrationCaseResult {
    pub first_pass_success: bool,
    pub validation_passed: bool,
    pub evaluation_passed: Option<bool>,
    pub rework_required: bool,
    pub escalation_required: bool,
    pub elapsed_ms: Option<u64>,
}

/// Small, versioned, non-sensitive calibration corpus. The executor is
/// supplied by the caller so the router does not own provider/network code.
#[must_use]
pub fn default_calibration_cases() -> Vec<CalibrationCase> {
    vec![
        CalibrationCase {
            id: "writer-mechanical",
            role: ModelRole::Writer,
            difficulty_bucket: 1,
            description: "make a bounded mechanical code change",
        },
        CalibrationCase {
            id: "writer-cross-module",
            role: ModelRole::Writer,
            difficulty_bucket: 3,
            description: "change behavior across two modules and preserve a caller",
        },
        CalibrationCase {
            id: "writer-risk-sensitive",
            role: ModelRole::Writer,
            difficulty_bucket: 5,
            description: "reason about a security or concurrency constraint",
        },
        CalibrationCase {
            id: "evaluator-complete",
            role: ModelRole::Evaluator,
            difficulty_bucket: 1,
            description: "classify a complete candidate from bounded evidence",
        },
        CalibrationCase {
            id: "evaluator-obvious-gap",
            role: ModelRole::Evaluator,
            difficulty_bucket: 2,
            description: "identify an explicit missing requirement",
        },
        CalibrationCase {
            id: "evaluator-subtle-gap",
            role: ModelRole::Evaluator,
            difficulty_bucket: 4,
            description: "identify an untested semantic omission",
        },
    ]
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExternalBenchmarkPrior {
    pub source: String,
    pub source_version: String,
    pub source_date: String,
    pub identity: CalibrationIdentity,
    pub role: ModelRole,
    pub capability: Capability,
    pub reference_input_micros: Option<u64>,
    pub reference_output_micros: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct CalibrationStore {
    pub schema_version: u32,
    pub observations: Vec<CalibrationObservation>,
    pub external_priors: Vec<ExternalBenchmarkPrior>,
}

impl CalibrationStore {
    #[must_use]
    pub fn new() -> Self {
        Self {
            schema_version: CALIBRATION_SCHEMA_VERSION,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn from_runtime_config(config: &runtime::RuntimeConfig) -> Self {
        let value = serde_json::from_str::<Value>(&config.as_json().render()).ok();
        value
            .as_ref()
            .and_then(|root| root.get("modelCalibration"))
            .and_then(|value| serde_json::from_value::<Self>(value.clone()).ok())
            .filter(|store| store.schema_version == CALIBRATION_SCHEMA_VERSION)
            .unwrap_or_default()
    }

    pub fn record(&mut self, observation: CalibrationObservation) -> Result<(), String> {
        if observation.profile_id.trim().is_empty()
            || observation.identity.provider.trim().is_empty()
            || observation.identity.model.trim().is_empty()
        {
            return Err("calibration observation has an incomplete profile identity".to_string());
        }
        self.schema_version = CALIBRATION_SCHEMA_VERSION;
        self.observations.push(observation);
        Ok(())
    }

    pub fn record_outcome(
        &mut self,
        outcome: OutcomeMetadata,
        private_mode: bool,
    ) -> Result<(), String> {
        if private_mode {
            return Err("private mode forbids durable outcome calibration".to_string());
        }
        self.record(CalibrationObservation {
            profile_id: outcome.profile_id,
            identity: outcome.identity,
            role: outcome.role,
            corpus_version: "production-outcome".to_string(),
            bastion_version: "runtime".to_string(),
            runtime_identity: outcome.runtime_identity,
            difficulty_bucket: outcome.difficulty_bucket,
            first_pass_success: outcome.first_pass_success,
            validation_passed: outcome.validation_passed,
            evaluation_passed: outcome.evaluation_passed,
            rework_required: outcome.rework_required,
            escalation_required: outcome.escalation_required,
            elapsed_ms: outcome.elapsed_ms,
            evidence_kind: CalibrationEvidenceKind::ObservedOutcome,
            recorded_at: outcome.recorded_at,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn run_local_calibration<F>(
        &mut self,
        profile: &ModelProfile,
        corpus_version: &str,
        bastion_version: &str,
        runtime_identity: &str,
        cases: &[CalibrationCase],
        execute: &F,
        recorded_at: &str,
    ) -> Result<CalibrationRunSummary, String>
    where
        F: Fn(&CalibrationCase) -> Result<CalibrationCaseResult, String>,
    {
        let identity = CalibrationIdentity::from_profile(profile);
        self.observations.retain(|observation| {
            !(observation.evidence_kind == CalibrationEvidenceKind::LocalCalibration
                && observation.profile_id == profile.id
                && observation.identity == identity
                && observation.corpus_version == corpus_version)
        });
        let mut summary = CalibrationRunSummary::default();
        for case in cases {
            summary.cases_run += 1;
            let Ok(result) = execute(case) else {
                summary.infrastructure_failures += 1;
                continue;
            };
            let observation = CalibrationObservation {
                profile_id: profile.id.clone(),
                identity: CalibrationIdentity::from_profile(profile),
                role: case.role,
                corpus_version: corpus_version.to_string(),
                bastion_version: bastion_version.to_string(),
                runtime_identity: runtime_identity.to_string(),
                difficulty_bucket: case.difficulty_bucket,
                first_pass_success: result.first_pass_success,
                validation_passed: result.validation_passed,
                evaluation_passed: result.evaluation_passed,
                rework_required: result.rework_required,
                escalation_required: result.escalation_required,
                elapsed_ms: result.elapsed_ms,
                evidence_kind: CalibrationEvidenceKind::LocalCalibration,
                recorded_at: recorded_at.to_string(),
            };
            if result.first_pass_success {
                summary.first_pass_successes += 1;
            }
            self.record(observation.clone())?;
            summary.observations.push(observation);
        }
        Ok(summary)
    }

    pub fn import_external_prior(&mut self, prior: ExternalBenchmarkPrior) -> Result<(), String> {
        if prior.source.trim().is_empty()
            || prior.identity.provider.trim().is_empty()
            || prior.identity.model.trim().is_empty()
        {
            return Err("external prior has an incomplete source or profile identity".to_string());
        }
        self.schema_version = CALIBRATION_SCHEMA_VERSION;
        self.external_priors.push(prior);
        Ok(())
    }

    /// Imports an explicit metadata document. This method performs no network
    /// access; callers decide if and when a file is obtained.
    pub fn import_external_json(&mut self, json: &str) -> Result<usize, String> {
        let document = serde_json::from_str::<CalibrationStore>(json)
            .map_err(|error| format!("invalid calibration document: {error}"))?;
        if document.schema_version != CALIBRATION_SCHEMA_VERSION {
            return Err(format!(
                "unsupported calibration schema {}",
                document.schema_version
            ));
        }
        let count = document.external_priors.len();
        for prior in document.external_priors {
            self.import_external_prior(prior)?;
        }
        Ok(count)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = serde_json::to_vec_pretty(self).map_err(|error| error.to_string())?;
        std::fs::write(path, text).map_err(|error| error.to_string())
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
        let store = serde_json::from_str::<Self>(&text).map_err(|error| error.to_string())?;
        if store.schema_version != CALIBRATION_SCHEMA_VERSION {
            return Err(format!(
                "unsupported calibration schema {}",
                store.schema_version
            ));
        }
        Ok(store)
    }

    pub fn clear(&mut self) {
        self.observations.clear();
        self.external_priors.clear();
    }

    pub fn clear_local(&mut self, profile_id: Option<&str>) {
        self.observations.retain(|observation| {
            observation.evidence_kind != CalibrationEvidenceKind::LocalCalibration
                || profile_id.is_some_and(|id| observation.profile_id != id)
        });
    }

    #[must_use]
    pub fn summary(&self) -> Vec<String> {
        let mut rows = self
            .observations
            .iter()
            .map(|observation| {
                format!(
                    "{} {:?} bucket={} first_pass={} validation={} rework={} escalation={} evidence={:?}",
                    observation.profile_id,
                    observation.role,
                    observation.difficulty_bucket,
                    observation.first_pass_success,
                    observation.validation_passed,
                    observation.rework_required,
                    observation.escalation_required,
                    observation.evidence_kind,
                )
            })
            .collect::<Vec<_>>();
        rows.sort();
        rows
    }

    #[must_use]
    pub fn effective_profile(&self, profile: &ModelProfile) -> ModelProfile {
        let identity = CalibrationIdentity::from_profile(profile);
        let mut effective = profile.clone();
        for role in [
            ModelRole::Writer,
            ModelRole::Evaluator,
            ModelRole::Planner,
            ModelRole::Other,
        ] {
            let observations = self
                .observations
                .iter()
                .filter(|observation| {
                    observation.profile_id == profile.id
                        && observation.identity == identity
                        && observation.role == role
                })
                .collect::<Vec<_>>();
            let prior = self
                .external_priors
                .iter()
                .filter(|prior| prior.identity == identity && prior.role == role)
                .max_by_key(|prior| prior.source_date.as_str());
            let Some(capability) =
                self.effective_capability(profile.capability, role, &observations, prior)
            else {
                continue;
            };
            // A profile has one capability vector, while evidence is role-specific.
            // Apply the evidence-supported value without mutating the configured prior.
            effective.capability.coding = capability.coding;
            effective.capability.reasoning = capability.reasoning;
            effective.capability.agent_tool_use = capability.agent_tool_use;
            effective.capability.planning = capability.planning;
            effective.capability.evaluation = capability.evaluation;
        }
        effective
    }

    fn effective_capability(
        &self,
        configured: Capability,
        role: ModelRole,
        observations: &[&CalibrationObservation],
        external: Option<&ExternalBenchmarkPrior>,
    ) -> Option<Capability> {
        let external_score = external.map(|prior| {
            let values = match role {
                ModelRole::Writer => [prior.capability.coding, prior.capability.agent_tool_use],
                ModelRole::Evaluator => [prior.capability.evaluation, prior.capability.reasoning],
                ModelRole::Planner => [prior.capability.planning, prior.capability.reasoning],
                ModelRole::Other => [
                    prior.capability.reasoning,
                    prior.capability.context_window.min(100) as u8,
                ],
            };
            u16::midpoint(u16::from(values[0]), u16::from(values[1]))
        });
        let mut successes = external_score.map_or(0, u32::from);
        let mut attempts = external.map_or(0, |_| 1u32);
        for observation in observations {
            successes += u32::from(observation.first_pass_success);
            attempts += 1;
        }
        if attempts == 0 {
            return None;
        }
        // Beta(1,1) smoothing and a five-observation cap keep sparse data
        // conservative. Local observations outweigh a single imported prior.
        let observed = ((successes + 1) * 100 / (attempts + 2)).min(100) as u8;
        let weight = attempts.min(5);
        let blend = |prior: u8| {
            let numerator = u32::from(prior) * (5 - weight) + u32::from(observed) * weight;
            u8::try_from(numerator / 5).unwrap_or(u8::MAX)
        };
        let mut capability = configured;
        match role {
            ModelRole::Writer => {
                capability.coding = blend(configured.coding);
                capability.agent_tool_use = blend(configured.agent_tool_use);
            }
            ModelRole::Evaluator => capability.evaluation = blend(configured.evaluation),
            ModelRole::Planner => capability.planning = blend(configured.planning),
            ModelRole::Other => {}
        }
        capability.reasoning = blend(configured.reasoning);
        Some(capability)
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

#[must_use]
pub fn difficulty_bucket(estimate: DifficultyEstimate) -> u8 {
    let requirement = estimate.requirement;
    let score = (u16::from(requirement.coding)
        + u16::from(requirement.reasoning)
        + u16::from(requirement.agent_tool_use)
        + u16::from(requirement.planning)
        + u16::from(requirement.evaluation))
        / 5;
    match score {
        0..=39 => 1,
        40..=59 => 2,
        60..=79 => 3,
        80..=94 => 4,
        _ => 5,
    }
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
    pub fn route_with_calibration(
        pool: &ModelPool,
        calibration: &CalibrationStore,
        role: ModelRole,
        signals: TaskSignals,
        policy: &RoutingPolicy,
    ) -> RouteDecision {
        let effective_pool = pool.with_calibration(calibration);
        Self::route(&effective_pool, role, signals, policy)
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

    #[must_use]
    pub fn escalation_with_calibration(
        pool: &ModelPool,
        calibration: &CalibrationStore,
        role: ModelRole,
        signals: TaskSignals,
        policy: RoutingPolicy,
    ) -> RouteDecision {
        let effective_pool = pool.with_calibration(calibration);
        Self::escalation(&effective_pool, role, signals, policy)
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

    fn observation(
        profile: &ModelProfile,
        role: ModelRole,
        success: bool,
    ) -> CalibrationObservation {
        CalibrationObservation {
            profile_id: profile.id.clone(),
            identity: CalibrationIdentity::from_profile(profile),
            role,
            corpus_version: "fixture-v1".to_string(),
            bastion_version: "test".to_string(),
            runtime_identity: "mock-runtime-v1".to_string(),
            difficulty_bucket: 2,
            first_pass_success: success,
            validation_passed: success,
            evaluation_passed: Some(success),
            rework_required: !success,
            escalation_required: false,
            elapsed_ms: Some(10),
            evidence_kind: CalibrationEvidenceKind::LocalCalibration,
            recorded_at: "2026-09-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn calibration_preserves_configured_prior_and_is_conservative_for_sparse_data() {
        let profile = profile("calibrated", PrivacyClass::Local, 80, 1);
        let mut store = CalibrationStore::new();
        store
            .record(observation(&profile, ModelRole::Writer, false))
            .unwrap();
        let effective = store.effective_profile(&profile);
        assert_eq!(profile.capability.coding, 80);
        assert!(effective.capability.coding < 80);
        assert!(effective.capability.coding > 0);
    }

    #[test]
    fn repeated_local_evidence_can_cross_a_routing_boundary() {
        let weak = profile("local", PrivacyClass::Local, 40, 0);
        let strong = profile("strong", PrivacyClass::Local, 90, 10);
        let mut store = CalibrationStore::new();
        for _ in 0..5 {
            store
                .record(observation(&weak, ModelRole::Writer, true))
                .unwrap();
        }
        let decision = ModelRouter::route_with_calibration(
            &ModelPool {
                profiles: vec![weak, strong],
            },
            &store,
            ModelRole::Writer,
            signals(),
            &RoutingPolicy::default(),
        );
        assert_eq!(decision.selected.unwrap().id, "local");
    }

    #[test]
    fn calibration_is_role_specific_and_reasoning_profile_specific() {
        let mut profile = profile("same", PrivacyClass::Local, 80, 1);
        profile.reasoning_profile = Some("low".to_string());
        let mut store = CalibrationStore::new();
        store
            .record(observation(&profile, ModelRole::Evaluator, false))
            .unwrap();
        let mut high = profile.clone();
        high.reasoning_profile = Some("high".to_string());
        assert_eq!(store.effective_profile(&high), high);
        assert_eq!(store.effective_profile(&profile).capability.coding, 80);
        assert!(store.effective_profile(&profile).capability.evaluation < 80);
    }

    #[test]
    fn external_prior_import_is_explicit_and_keeps_reference_price_separate() {
        let profile = profile("custom", PrivacyClass::Local, 50, 0);
        let prior = ExternalBenchmarkPrior {
            source: "fixture".to_string(),
            source_version: "2026-01".to_string(),
            source_date: "2026-01-01".to_string(),
            identity: CalibrationIdentity::from_profile(&profile),
            role: ModelRole::Writer,
            capability: Capability {
                coding: 90,
                reasoning: 90,
                agent_tool_use: 90,
                planning: 90,
                evaluation: 90,
                context_window: 64_000,
            },
            reference_input_micros: Some(123),
            reference_output_micros: Some(456),
        };
        let document = serde_json::to_string(&CalibrationStore {
            schema_version: CALIBRATION_SCHEMA_VERSION,
            observations: Vec::new(),
            external_priors: vec![prior],
        })
        .unwrap();
        let mut store = CalibrationStore::new();
        assert_eq!(store.import_external_json(&document).unwrap(), 1);
        assert_eq!(store.external_priors[0].reference_input_micros, Some(123));
        assert_eq!(profile.pricing.actual_input_micros, 0);
    }

    #[test]
    fn policy_filtering_still_precedes_calibrated_capability() {
        let remote = profile("remote", PrivacyClass::Remote, 100, 0);
        let local = profile("local", PrivacyClass::Local, 70, 20);
        let mut store = CalibrationStore::new();
        for _ in 0..5 {
            store
                .record(observation(&remote, ModelRole::Writer, true))
                .unwrap();
        }
        let decision = ModelRouter::route_with_calibration(
            &ModelPool {
                profiles: vec![remote, local],
            },
            &store,
            ModelRole::Writer,
            signals(),
            &RoutingPolicy {
                local_only: true,
                ..RoutingPolicy::default()
            },
        );
        assert_eq!(decision.selected.unwrap().id, "local");
    }

    #[test]
    fn local_calibration_runner_records_versioned_role_specific_observations() {
        let profile = profile("fixture", PrivacyClass::Local, 70, 0);
        let mut store = CalibrationStore::new();
        let cases = default_calibration_cases();
        let summary = store
            .run_local_calibration(
                &profile,
                "calibration-v1",
                "bastion-test",
                "mock-runtime-v1",
                &cases,
                &|case| {
                    Ok(CalibrationCaseResult {
                        first_pass_success: case.difficulty_bucket < 4,
                        validation_passed: case.difficulty_bucket < 4,
                        evaluation_passed: Some(case.difficulty_bucket < 4),
                        rework_required: case.difficulty_bucket >= 4,
                        ..CalibrationCaseResult::default()
                    })
                },
                "2026-09-01T00:00:00Z",
            )
            .unwrap();
        assert_eq!(summary.cases_run, 6);
        assert_eq!(summary.first_pass_successes, 4);
        assert!(summary
            .observations
            .iter()
            .all(|item| item.corpus_version == "calibration-v1"));
        assert_eq!(
            summary
                .observations
                .iter()
                .filter(|item| item.role == ModelRole::Evaluator)
                .count(),
            3
        );
    }

    #[test]
    fn private_outcome_ingestion_is_rejected_without_mutating_store() {
        let profile = profile("private", PrivacyClass::Local, 70, 0);
        let mut store = CalibrationStore::new();
        let outcome = OutcomeMetadata {
            profile_id: profile.id.clone(),
            identity: CalibrationIdentity::from_profile(&profile),
            role: ModelRole::Writer,
            runtime_identity: "runtime".to_string(),
            difficulty_bucket: 2,
            first_pass_success: true,
            validation_passed: true,
            evaluation_passed: Some(true),
            rework_required: false,
            escalation_required: false,
            elapsed_ms: Some(1),
            recorded_at: "now".to_string(),
        };
        assert!(store.record_outcome(outcome, true).is_err());
        assert!(store.observations.is_empty());
    }
}

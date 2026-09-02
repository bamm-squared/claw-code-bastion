//! Independent, phased evaluation of advisory task requirements.
//!
//! Evaluation is deliberately separate from writing and validation. The
//! deterministic phase is conservative; a future model evaluator can consume
//! [`EvaluationRequest`] without receiving the writer's conversation history.

use crate::task_plan::{ExpectedContract, PlanItemStatus, TaskPlan};
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequirementState {
    Satisfied,
    PartiallySatisfied,
    MissingEvidence,
    GapFound,
    Uncertain,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvaluationFinding {
    pub requirement_id: String,
    pub state: RequirementState,
    pub finding: String,
    pub evidence: String,
    pub confidence: &'static str,
    pub rework_recommended: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvaluationReport {
    pub deterministic: bool,
    pub requirements: Vec<EvaluationFinding>,
    pub validation_passed: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvaluationRequest {
    pub original_requirement: String,
    pub contracts: Vec<ExpectedContract>,
    pub unresolved_requirement_ids: Vec<String>,
    pub changed_paths: Vec<String>,
    pub validation_summary: String,
    pub graph_evidence: Vec<String>,
}

pub struct RequirementEvaluator;

impl RequirementEvaluator {
    #[must_use]
    pub fn deterministic(
        plan: &TaskPlan,
        changed_paths: &[String],
        validation_passed: bool,
    ) -> EvaluationReport {
        let requirements = if plan.items.is_empty() {
            vec![EvaluationFinding {
                requirement_id: "task".to_string(),
                state: RequirementState::MissingEvidence,
                finding: "No structured requirement was available for evaluation.".to_string(),
                evidence: "The original request remains available for semantic evaluation."
                    .to_string(),
                confidence: "exact",
                rework_recommended: true,
            }]
        } else {
            plan.items
                .iter()
                .map(|item| {
                    if changed_paths.is_empty() {
                        EvaluationFinding {
                            requirement_id: item.id.clone(),
                            state: RequirementState::MissingEvidence,
                            finding: "No candidate change demonstrates this requirement."
                                .to_string(),
                            evidence: "CandidateChangeSet contains no changed paths.".to_string(),
                            confidence: "exact",
                            rework_recommended: true,
                        }
                    } else if !validation_passed {
                        EvaluationFinding {
                            requirement_id: item.id.clone(),
                            state: RequirementState::Uncertain,
                            finding:
                                "Candidate changes exist, but trusted validation did not pass."
                                    .to_string(),
                            evidence: format!("Changed paths: {}", changed_paths.join(", ")),
                            confidence: "exact",
                            rework_recommended: false,
                        }
                    } else {
                        EvaluationFinding {
                            requirement_id: item.id.clone(),
                            state: RequirementState::Uncertain,
                            finding: "Semantic satisfaction still requires independent judgment."
                                .to_string(),
                            evidence: format!("Changed paths: {}", changed_paths.join(", ")),
                            confidence: "unknown",
                            rework_recommended: false,
                        }
                    }
                })
                .collect()
        };
        EvaluationReport {
            deterministic: false,
            requirements,
            validation_passed,
            error: None,
        }
    }

    #[must_use]
    pub fn request(
        plan: &TaskPlan,
        report: &EvaluationReport,
        changed_paths: &[String],
        validation_summary: &str,
    ) -> EvaluationRequest {
        EvaluationRequest {
            original_requirement: plan.original_request.clone(),
            contracts: plan.contracts.clone(),
            unresolved_requirement_ids: report
                .requirements
                .iter()
                .filter(|finding| finding.state != RequirementState::Satisfied)
                .map(|finding| finding.requirement_id.clone())
                .collect(),
            changed_paths: changed_paths.to_vec(),
            validation_summary: validation_summary.to_string(),
            graph_evidence: plan.known_impact.clone(),
        }
    }

    #[must_use]
    pub fn render_request(request: &EvaluationRequest) -> String {
        let mut output = String::from("[Independent Requirement Evaluation]\n");
        output.push_str("Original requirement: ");
        output.push_str(&request.original_requirement);
        output.push_str("\nUnresolved requirements: ");
        output.push_str(&request.unresolved_requirement_ids.join(", "));
        output.push_str("\nCandidate paths: ");
        output.push_str(&request.changed_paths.join(", "));
        output.push_str("\nValidation evidence: ");
        output.push_str(&request.validation_summary);
        if !request.graph_evidence.is_empty() {
            output.push_str("\nGraph evidence:\n");
            for evidence in &request.graph_evidence {
                output.push_str("- ");
                output.push_str(evidence);
                output.push('\n');
            }
        }
        output.push_str(
            "The writer conversation is intentionally excluded; inspect exact source as needed.\n",
        );
        output
    }
}

#[derive(Debug, Deserialize)]
struct ModelEvaluationEnvelope {
    requirements: Vec<ModelEvaluationFinding>,
}

#[derive(Debug, Deserialize)]
struct ModelEvaluationFinding {
    requirement_id: String,
    state: String,
    finding: String,
    evidence: String,
    confidence: String,
    rework_recommended: bool,
}

impl RequirementEvaluator {
    pub fn from_model_response(
        response: &str,
        validation_passed: bool,
    ) -> Result<EvaluationReport, String> {
        let envelope: ModelEvaluationEnvelope = serde_json::from_str(response.trim())
            .map_err(|error| format!("malformed evaluator response: {error}"))?;
        if envelope.requirements.is_empty() {
            return Err("evaluator returned no requirement findings".to_string());
        }
        let requirements = envelope
            .requirements
            .into_iter()
            .map(|finding| {
                let state = match finding.state.to_ascii_lowercase().as_str() {
                    "satisfied" => RequirementState::Satisfied,
                    "partially_satisfied" | "partially satisfied" => {
                        RequirementState::PartiallySatisfied
                    }
                    "missing_evidence" | "missing evidence" => RequirementState::MissingEvidence,
                    "gap_found" | "gap found" => RequirementState::GapFound,
                    "uncertain" => RequirementState::Uncertain,
                    other => return Err(format!("unknown evaluator state: {other}")),
                };
                Ok(EvaluationFinding {
                    requirement_id: finding.requirement_id,
                    state,
                    finding: finding.finding,
                    evidence: finding.evidence,
                    confidence: Box::leak(finding.confidence.into_boxed_str()),
                    rework_recommended: finding.rework_recommended,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(EvaluationReport {
            deterministic: false,
            requirements,
            validation_passed,
            error: None,
        })
    }

    pub fn unavailable(validation_passed: bool, reason: impl Into<String>) -> EvaluationReport {
        EvaluationReport {
            deterministic: false,
            requirements: vec![EvaluationFinding {
                requirement_id: "evaluation".to_string(),
                state: RequirementState::Uncertain,
                finding: "Independent semantic evaluation was unavailable.".to_string(),
                evidence: reason.into(),
                confidence: "unknown",
                rework_recommended: true,
            }],
            validation_passed,
            error: Some("independent semantic evaluation unavailable".to_string()),
        }
    }
}

impl EvaluationReport {
    #[must_use]
    pub fn has_rework_finding(&self) -> bool {
        self.requirements
            .iter()
            .any(|finding| finding.rework_recommended)
    }

    #[must_use]
    pub fn summary(&self) -> String {
        let summary = self
            .requirements
            .iter()
            .map(|finding| {
                format!(
                    "{}: {} ({})",
                    finding.requirement_id,
                    state_label(finding.state),
                    finding.finding
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        self.error.as_ref().map_or(summary.clone(), |error| {
            format!("{summary}\nevaluator error: {error}")
        })
    }
}

fn state_label(state: RequirementState) -> &'static str {
    match state {
        RequirementState::Satisfied => "satisfied",
        RequirementState::PartiallySatisfied => "partially satisfied",
        RequirementState::MissingEvidence => "missing evidence",
        RequirementState::GapFound => "gap found",
        RequirementState::Uncertain => "uncertain",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> TaskPlan {
        TaskPlan::from_request("Preserve private mode while updating the provider", None)
    }

    #[test]
    fn no_candidate_evidence_is_not_evaluated_as_success() {
        let report = RequirementEvaluator::deterministic(&plan(), &[], true);
        assert!(report
            .requirements
            .iter()
            .all(|finding| finding.state == RequirementState::MissingEvidence));
        assert!(report.has_rework_finding());
    }

    #[test]
    fn validation_and_requirement_evaluation_are_independent() {
        let report =
            RequirementEvaluator::deterministic(&plan(), &["src/provider.rs".into()], true);
        assert!(report.validation_passed);
        assert!(report
            .requirements
            .iter()
            .all(|finding| finding.state == RequirementState::Uncertain));
        assert!(!report.has_rework_finding());
    }

    #[test]
    fn failed_validation_does_not_become_requirement_success() {
        let report =
            RequirementEvaluator::deterministic(&plan(), &["src/provider.rs".into()], false);
        assert!(!report.validation_passed);
        assert!(report
            .requirements
            .iter()
            .all(|finding| finding.state == RequirementState::Uncertain));
    }

    #[test]
    fn fresh_request_excludes_writer_history() {
        let report =
            RequirementEvaluator::deterministic(&plan(), &["src/provider.rs".into()], true);
        let request =
            RequirementEvaluator::request(&plan(), &report, &["src/provider.rs".into()], "PASS");
        let rendered = RequirementEvaluator::render_request(&request);
        assert!(rendered.contains("Original requirement"));
        assert!(rendered.contains("writer conversation is intentionally excluded"));
        assert!(!rendered.contains("assistant history"));
    }

    #[test]
    fn evaluator_states_are_not_plan_implementation_states() {
        assert_ne!(
            RequirementState::Satisfied as u8,
            PlanItemStatus::Implemented as u8
        );
    }

    #[test]
    fn malformed_model_response_is_not_success() {
        assert!(RequirementEvaluator::from_model_response("not json", true).is_err());
    }

    #[test]
    fn structured_model_response_preserves_requirement_state() {
        let report = RequirementEvaluator::from_model_response(
            r#"{"requirements":[{"requirement_id":"r1","state":"gap_found","finding":"missing path","evidence":"no change","confidence":"high","rework_recommended":true}]}"#,
            true,
        )
        .expect("valid evaluator response");
        assert_eq!(report.requirements[0].state, RequirementState::GapFound);
        assert!(report.has_rework_finding());
    }
}

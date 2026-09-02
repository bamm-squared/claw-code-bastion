//! Deterministic, advisory planning state for the candidate writer.
//!
//! This module deliberately does not call a model or inspect candidate
//! authority. It turns the user's request and already-selected repository
//! facts into a compact working hypothesis.

use serde::{Deserialize, Serialize};

const MAX_REQUEST_BYTES: usize = 512;
const MAX_STATEMENT_BYTES: usize = 240;
const MAX_ITEMS: usize = 6;
const MAX_CONTRACTS: usize = 6;
const MAX_IMPACT_LINES: usize = 4;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum PlanItemStatus {
    Proposed,
    NeedsResearch,
    Active,
    Blocked,
    Implemented,
    EvaluationFailed,
    Verified,
    Dropped,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlanItem {
    pub id: String,
    pub statement: String,
    pub status: PlanItemStatus,
    pub provenance: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExpectedContract {
    pub id: String,
    pub expectation: String,
    pub basis: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskPlan {
    pub original_request: String,
    pub revision: u32,
    pub items: Vec<PlanItem>,
    pub contracts: Vec<ExpectedContract>,
    pub known_impact: Vec<String>,
    pub open_questions: Vec<String>,
}

impl TaskPlan {
    pub fn invalidate_after_candidate_restore(&mut self) {
        let mut changed = false;
        for item in &mut self.items {
            if matches!(
                item.status,
                PlanItemStatus::Implemented
                    | PlanItemStatus::EvaluationFailed
                    | PlanItemStatus::Verified
            ) {
                item.status = PlanItemStatus::NeedsResearch;
                item.provenance =
                    "candidate checkpoint restored; candidate-dependent state must be reconfirmed"
                        .to_string();
                changed = true;
            }
        }
        if changed {
            self.open_questions
                .push("Reconfirm the plan against the restored candidate state.".to_string());
            self.open_questions.truncate(MAX_CONTRACTS);
            self.revision = self.revision.saturating_add(1);
        }
    }

    #[must_use]
    pub fn from_request(request: &str, repository_context: Option<&str>) -> Self {
        let mut plan = Self {
            original_request: truncate(request.trim(), MAX_REQUEST_BYTES),
            ..Self::default()
        };
        plan.rebuild_from_request(repository_context);
        plan
    }

    pub fn update(&mut self, request: &str, repository_context: Option<&str>) {
        let request = truncate(request.trim(), MAX_REQUEST_BYTES);
        if self.original_request != request {
            self.original_request = request;
            self.revision = self.revision.saturating_add(1);
            self.rebuild_from_request(repository_context);
        } else if self.items.is_empty() {
            self.rebuild_from_request(repository_context);
        }
    }

    fn rebuild_from_request(&mut self, repository_context: Option<&str>) {
        self.items = clauses(&self.original_request)
            .into_iter()
            .take(MAX_ITEMS)
            .enumerate()
            .map(|(index, statement)| PlanItem {
                id: format!("item-{}", index + 1),
                statement,
                status: PlanItemStatus::Proposed,
                provenance: "user request".to_string(),
            })
            .collect();
        self.contracts = clauses(&self.original_request)
            .into_iter()
            .filter(|clause| contains_constraint_language(clause))
            .take(MAX_CONTRACTS)
            .enumerate()
            .map(|(index, expectation)| ExpectedContract {
                id: format!("contract-{}", index + 1),
                expectation,
                basis: "user constraint".to_string(),
            })
            .collect();
        self.known_impact = repository_context
            .map(|context| {
                context
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .take(MAX_IMPACT_LINES)
                    .map(|line| truncate(line, MAX_STATEMENT_BYTES))
                    .collect()
            })
            .unwrap_or_default();
        self.open_questions = if self.known_impact.is_empty() {
            vec!["Exact affected code remains to be confirmed from source.".to_string()]
        } else {
            vec!["Confirm the selected repository relationships against exact source.".to_string()]
        };
    }

    pub fn add_discovered_item(&mut self, statement: &str, provenance: &str) {
        let statement = truncate(statement.trim(), MAX_STATEMENT_BYTES);
        if statement.is_empty() || self.items.iter().any(|item| item.statement == statement) {
            return;
        }
        self.items.push(PlanItem {
            id: format!("item-{}", self.items.len() + 1),
            statement,
            status: PlanItemStatus::NeedsResearch,
            provenance: truncate(provenance, MAX_STATEMENT_BYTES),
        });
        self.revision = self.revision.saturating_add(1);
    }

    pub fn mark_implemented(&mut self, item_id: &str, provenance: &str) {
        if let Some(item) = self.items.iter_mut().find(|item| item.id == item_id) {
            item.status = PlanItemStatus::Implemented;
            item.provenance = truncate(provenance, MAX_STATEMENT_BYTES);
            self.revision = self.revision.saturating_add(1);
        }
    }

    pub fn reopen_for_evaluation(&mut self, item_id: &str, reason: &str) {
        if let Some(item) = self.items.iter_mut().find(|item| item.id == item_id) {
            item.status = PlanItemStatus::NeedsResearch;
            item.provenance = truncate(reason, MAX_STATEMENT_BYTES);
            self.revision = self.revision.saturating_add(1);
        }
        self.open_questions.push(format!(
            "Evaluation follow-up for {item_id}: {}",
            truncate(reason, 160)
        ));
        self.open_questions.truncate(MAX_CONTRACTS);
    }

    pub fn mark_validation_failure(&mut self, reason: &str) {
        for item in &mut self.items {
            if matches!(
                item.status,
                PlanItemStatus::Implemented | PlanItemStatus::Active
            ) {
                item.status = PlanItemStatus::EvaluationFailed;
            }
        }
        self.open_questions.push(format!(
            "Validation requires follow-up: {}",
            truncate(reason, 160)
        ));
        self.open_questions.truncate(MAX_CONTRACTS);
        self.revision = self.revision.saturating_add(1);
    }

    #[must_use]
    pub fn render(&self) -> String {
        let mut output = String::from("[Advisory Task Plan]\n");
        output.push_str("Goal: ");
        output.push_str(&self.original_request);
        output.push_str("\nWork items:\n");
        if self.items.is_empty() {
            output.push_str("- clarify the requested outcome [needs research]\n");
        } else {
            for item in &self.items {
                output.push_str("- ");
                output.push_str(&item.statement);
                output.push_str(" [");
                output.push_str(status_label(&item.status));
                output.push_str("]\n");
            }
        }
        if !self.contracts.is_empty() {
            output.push_str("Expected contracts:\n");
            for contract in &self.contracts {
                output.push_str("- ");
                output.push_str(&contract.expectation);
                output.push_str(" [");
                output.push_str(&contract.basis);
                output.push_str("]\n");
            }
        }
        if !self.known_impact.is_empty() {
            output.push_str("Known impact evidence:\n");
            for line in &self.known_impact {
                output.push_str("- ");
                output.push_str(line);
                output.push('\n');
            }
        }
        output.push_str("Open questions:\n");
        for question in &self.open_questions {
            output.push_str("- ");
            output.push_str(question);
            output.push('\n');
        }
        output.push_str(
            "Advisory only: source, trusted validation, and Review/Apply remain authoritative.\n",
        );
        output
    }
}

fn clauses(request: &str) -> Vec<String> {
    request
        .split(|character: char| {
            character == '.' || character == '!' || character == '?' || character == '\n'
        })
        .map(str::trim)
        .filter(|clause| clause.len() > 8)
        .map(|clause| truncate(clause, MAX_STATEMENT_BYTES))
        .collect()
}

fn contains_constraint_language(clause: &str) -> bool {
    let lower = clause.to_ascii_lowercase();
    [
        "must",
        "must not",
        "do not",
        "without",
        "preserve",
        "unchanged",
        "retain",
        "keep",
    ]
    .iter()
    .any(|word| lower.contains(word))
}

fn status_label(status: &PlanItemStatus) -> &'static str {
    match status {
        PlanItemStatus::Proposed => "proposed",
        PlanItemStatus::NeedsResearch => "needs research",
        PlanItemStatus::Active => "active",
        PlanItemStatus::Blocked => "blocked",
        PlanItemStatus::Implemented => "implemented, not verified",
        PlanItemStatus::EvaluationFailed => "evaluation failed",
        PlanItemStatus::Verified => "verified",
        PlanItemStatus::Dropped => "dropped",
    }
}

fn truncate(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_plan_preserves_request_and_constraints() {
        let plan = TaskPlan::from_request(
            "Update provider handling. Preserve private mode and do not change Apply.",
            None,
        );
        assert!(plan.original_request.contains("Update provider handling"));
        assert_eq!(plan.items.len(), 2);
        assert_eq!(plan.contracts.len(), 1);
        assert!(plan.render().contains("Advisory only"));
    }

    #[test]
    fn graph_evidence_is_bounded_and_provenance_is_visible() {
        let plan = TaskPlan::from_request(
            "Fix the provider path",
            Some("src/provider.rs\ndefines: ProviderChain\nreferences: session.rs"),
        );
        let rendered = plan.render();
        assert!(rendered.contains("src/provider.rs"));
        assert!(rendered.contains("Known impact evidence"));
    }

    #[test]
    fn discovery_revision_and_validation_failure_remain_distinct() {
        let mut plan = TaskPlan::from_request("Implement the change", None);
        plan.add_discovered_item("Inspect the session restoration path", "impact analysis");
        assert_eq!(plan.items[1].status, PlanItemStatus::NeedsResearch);
        plan.mark_implemented("item-1", "candidate writer");
        assert_eq!(plan.items[0].status, PlanItemStatus::Implemented);
        plan.mark_validation_failure("trusted check failed");
        assert_eq!(plan.items[0].status, PlanItemStatus::EvaluationFailed);
        assert!(plan.render().contains("evaluation failed"));
    }

    #[test]
    fn same_request_update_is_stable() {
        let mut plan = TaskPlan::from_request("Inspect provider.rs", None);
        let revision = plan.revision;
        plan.update("Inspect provider.rs", None);
        assert_eq!(plan.revision, revision);
    }

    #[test]
    fn evaluation_reopens_item_without_claiming_verification() {
        let mut plan = TaskPlan::from_request("Update the provider path", None);
        plan.mark_implemented("item-1", "candidate writer");
        plan.reopen_for_evaluation("item-1", "cross-module behavior remains incomplete");
        assert_eq!(plan.items[0].status, PlanItemStatus::NeedsResearch);
        assert!(plan
            .open_questions
            .iter()
            .any(|question| question.contains("Evaluation follow-up")));
    }

    #[test]
    fn checkpoint_restore_reopens_candidate_dependent_plan_state() {
        let mut plan = TaskPlan::from_request("Update the provider path", None);
        plan.mark_implemented("item-1", "candidate writer");
        let revision = plan.revision;

        plan.invalidate_after_candidate_restore();

        assert_eq!(plan.items[0].status, PlanItemStatus::NeedsResearch);
        assert!(plan.revision > revision);
        assert!(plan
            .open_questions
            .iter()
            .any(|question| question.contains("restored candidate")));
    }
}

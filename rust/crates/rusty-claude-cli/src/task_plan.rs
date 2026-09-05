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
const MAX_SCOPE_FILES: usize = 8;
const MAX_SCOPE_GUIDANCE: usize = 8;

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
    #[serde(default)]
    pub verification_boundary: VerificationBoundary,
    #[serde(default)]
    pub verification_basis: String,
    #[serde(default)]
    pub status: ContractStatus,
    #[serde(default)]
    pub evidence: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompletionAuditGap {
    pub contract_id: String,
    pub expectation: String,
    pub boundary: VerificationBoundary,
    pub reason: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum VerificationBoundary {
    #[default]
    UnitBehavior,
    PublicApi,
    StateTransition,
    Integration,
    ProcessInteraction,
    Compatibility,
    ErrorPath,
    Persistence,
}

impl VerificationBoundary {
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::UnitBehavior => "unit behavior",
            Self::PublicApi => "public API",
            Self::StateTransition => "state transition",
            Self::Integration => "integration",
            Self::ProcessInteraction => "process/interaction",
            Self::Compatibility => "compatibility",
            Self::ErrorPath => "error path",
            Self::Persistence => "persistence",
        }
    }

    #[must_use]
    pub fn rationale(&self) -> &'static str {
        match self {
            Self::UnitBehavior => {
                "the obligation describes local behavior that can be exercised directly"
            }
            Self::PublicApi => "the obligation concerns the behavior or shape exposed to callers",
            Self::StateTransition => {
                "the obligation depends on ordering, state, or repeated operations"
            }
            Self::Integration => "the obligation crosses module or subsystem boundaries",
            Self::ProcessInteraction => {
                "the obligation depends on a process, terminal, or user interaction boundary"
            }
            Self::Compatibility => {
                "the obligation preserves existing callers or established behavior"
            }
            Self::ErrorPath => {
                "the obligation concerns rejection, cancellation, or recovery behavior"
            }
            Self::Persistence => "the obligation must survive a storage or reload boundary",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum ContractStatus {
    #[default]
    Unverified,
    CandidateEvidence,
    Verified,
    Unresolved,
}

impl ContractStatus {
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Unverified => "unverified",
            Self::CandidateEvidence => "candidate evidence only",
            Self::Verified => "verified",
            Self::Unresolved => "unresolved",
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct TaskPlan {
    /// The complete user request. This is authoritative input for evaluation;
    /// it is never replaced by the bounded model/display representation.
    #[serde(default)]
    pub full_request: String,
    /// Bounded request text used in compact planning displays.
    pub original_request: String,
    pub revision: u32,
    pub items: Vec<PlanItem>,
    pub contracts: Vec<ExpectedContract>,
    pub known_impact: Vec<String>,
    #[serde(default)]
    pub repository_files: Vec<String>,
    #[serde(default)]
    pub primary_repository_files: Vec<String>,
    #[serde(default)]
    pub implementation_surface_guidance: Vec<String>,
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
        for contract in &mut self.contracts {
            if contract.status != ContractStatus::Unverified || !contract.evidence.is_empty() {
                contract.status = ContractStatus::Unverified;
                contract.evidence.clear();
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
        let full_request = request.trim().to_string();
        let mut plan = Self {
            original_request: truncate(&full_request, MAX_REQUEST_BYTES),
            full_request,
            ..Self::default()
        };
        plan.rebuild_from_request(repository_context);
        plan
    }

    pub fn update(&mut self, request: &str, repository_context: Option<&str>) {
        let request = request.trim().to_string();
        if self.full_request != request {
            self.full_request = request;
            self.original_request = truncate(&self.full_request, MAX_REQUEST_BYTES);
            self.revision = self.revision.saturating_add(1);
            self.rebuild_from_request(repository_context);
        } else if self.items.is_empty() {
            self.rebuild_from_request(repository_context);
        }
    }

    fn rebuild_from_request(&mut self, repository_context: Option<&str>) {
        self.items = clauses(&self.full_request)
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
        self.contracts = clauses(&self.full_request)
            .into_iter()
            .take(MAX_CONTRACTS)
            .enumerate()
            .map(|(index, expectation)| {
                let basis = if contains_constraint_language(expectation.as_str()) {
                    "user constraint".to_string()
                } else {
                    "user requirement".to_string()
                };
                ExpectedContract {
                    id: format!("contract-{}", index + 1),
                    verification_boundary: infer_verification_boundary(&expectation),
                    verification_basis: format!(
                        "{basis}; {}",
                        infer_verification_boundary(&expectation).rationale()
                    ),
                    expectation,
                    basis,
                    status: ContractStatus::default(),
                    evidence: String::new(),
                }
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

    #[must_use]
    pub fn authoritative_request(&self) -> &str {
        if self.full_request.is_empty() {
            &self.original_request
        } else {
            &self.full_request
        }
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

    pub fn set_repository_scope(
        &mut self,
        files: &[String],
        primary_files: &[String],
        guidance: &[String],
    ) {
        self.repository_files = files.iter().take(MAX_SCOPE_FILES).cloned().collect();
        self.primary_repository_files = primary_files
            .iter()
            .take(MAX_SCOPE_FILES)
            .cloned()
            .collect();
        self.implementation_surface_guidance = guidance
            .iter()
            .take(MAX_SCOPE_GUIDANCE)
            .map(|line| truncate(line, MAX_STATEMENT_BYTES))
            .collect();
    }

    pub fn record_candidate_evidence(&mut self, changed_paths: &[String]) {
        let evidence = if changed_paths.is_empty() {
            "No candidate paths changed; implementation evidence is absent.".to_string()
        } else {
            format!("Candidate changed paths: {}", changed_paths.join(", "))
        };
        for contract in &mut self.contracts {
            if contract.status != ContractStatus::Verified {
                contract.status = ContractStatus::CandidateEvidence;
                contract.evidence = truncate(
                    &format!(
                        "{evidence}; planned boundary: {}. Candidate edits are not behavioral proof at this boundary.",
                        contract.verification_boundary.label()
                    ),
                    MAX_STATEMENT_BYTES,
                );
            }
        }
        self.revision = self.revision.saturating_add(1);
    }

    #[must_use]
    pub fn completion_audit_gaps(
        &self,
        changed_paths: &[String],
        missing_evidence: &[String],
    ) -> Vec<CompletionAuditGap> {
        let has_test_change = changed_paths.iter().any(|path| is_test_path(path));
        let evidence_is_incomplete = changed_paths.is_empty() || !has_test_change;
        if !evidence_is_incomplete {
            return Vec::new();
        }
        self.contracts
            .iter()
            .filter(|contract| contract.status != ContractStatus::Verified)
            .take(MAX_CONTRACTS)
            .map(|contract| {
                let detail = missing_evidence
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "no changed test surface was identified".to_string());
                CompletionAuditGap {
                    contract_id: contract.id.clone(),
                    expectation: contract.expectation.clone(),
                    boundary: contract.verification_boundary.clone(),
                    reason: format!(
                        "{} requires {} evidence, but the current candidate provides only implementation activity: {}",
                        contract.id,
                        contract.verification_boundary.label(),
                        detail
                    ),
                }
            })
            .collect()
    }

    pub fn reopen_for_completion_audit(&mut self, gap: &CompletionAuditGap) {
        let item_id = contract_item_id(&gap.contract_id);
        if let Some(item) = self
            .items
            .iter_mut()
            .find(|item| item.id == item_id.as_deref().unwrap_or(&gap.contract_id))
        {
            item.status = PlanItemStatus::NeedsResearch;
            item.provenance = truncate(&gap.reason, MAX_STATEMENT_BYTES);
        }
        if let Some(contract) = self
            .contracts
            .iter_mut()
            .find(|contract| contract.id == gap.contract_id)
        {
            contract.status = ContractStatus::Unresolved;
            contract.evidence = truncate(&gap.reason, MAX_STATEMENT_BYTES);
        }
        if !self
            .open_questions
            .iter()
            .any(|question| question.starts_with("Completion evidence follow-up"))
        {
            self.open_questions.push(format!(
                "Completion evidence follow-up for {}: {}",
                gap.contract_id,
                truncate(&gap.reason, 160)
            ));
        }
        self.open_questions.truncate(MAX_CONTRACTS);
        self.revision = self.revision.saturating_add(1);
    }

    pub fn record_contract_evidence(&mut self, contract_id: &str, verified: bool, evidence: &str) {
        let item_id = contract_item_id(contract_id);
        if let Some(contract) = self
            .contracts
            .iter_mut()
            .find(|contract| contract.id == contract_id)
        {
            contract.status = if verified {
                ContractStatus::Verified
            } else {
                ContractStatus::Unresolved
            };
            contract.evidence = truncate(evidence, MAX_STATEMENT_BYTES);
        }
        if let Some(item) = self
            .items
            .iter_mut()
            .find(|item| item.id == item_id.as_deref().unwrap_or(contract_id))
        {
            item.status = if verified {
                PlanItemStatus::Verified
            } else {
                PlanItemStatus::NeedsResearch
            };
            item.provenance = truncate(evidence, MAX_STATEMENT_BYTES);
        }
        self.revision = self.revision.saturating_add(1);
    }

    #[must_use]
    pub fn all_contracts_verified(&self) -> bool {
        !self.contracts.is_empty()
            && self
                .contracts
                .iter()
                .all(|contract| contract.status == ContractStatus::Verified)
    }

    pub fn reopen_for_evaluation(&mut self, item_id: &str, reason: &str) {
        let plan_item_id = contract_item_id(item_id);
        if let Some(item) = self
            .items
            .iter_mut()
            .find(|item| item.id == plan_item_id.as_deref().unwrap_or(item_id))
        {
            item.status = PlanItemStatus::NeedsResearch;
            item.provenance = truncate(reason, MAX_STATEMENT_BYTES);
            self.revision = self.revision.saturating_add(1);
        }
        if let Some(contract) = self.contracts.iter_mut().find(|contract| {
            contract.id == item_id
                || contract.id
                    == format!(
                        "contract-{}",
                        item_id.strip_prefix("item-").unwrap_or(item_id)
                    )
        }) {
            contract.status = ContractStatus::Unresolved;
            contract.evidence = truncate(reason, MAX_STATEMENT_BYTES);
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
        for contract in &mut self.contracts {
            contract.status = ContractStatus::Unresolved;
            contract.evidence = truncate(reason, MAX_STATEMENT_BYTES);
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
                output.push_str("; ");
                output.push_str("boundary: ");
                output.push_str(contract.verification_boundary.label());
                output.push_str("; ");
                output.push_str(contract.status.label());
                output.push_str("]\n");
                output.push_str("  verification basis: ");
                output.push_str(&contract.verification_basis);
                output.push('\n');
                if !contract.evidence.is_empty() {
                    output.push_str("  evidence: ");
                    output.push_str(&contract.evidence);
                    output.push('\n');
                }
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
        if !self.repository_files.is_empty() || !self.implementation_surface_guidance.is_empty() {
            output.push_str("Implementation scope hypothesis:\n");
            for path in &self.repository_files {
                output.push_str("- selected repository file: ");
                output.push_str(path);
                output.push('\n');
            }
            for guidance in &self.implementation_surface_guidance {
                output.push_str("- surface evidence: ");
                output.push_str(guidance);
                output.push('\n');
            }
            output.push_str(
                "Scope is a bounded hypothesis; verify it before changing similarly named alternate surfaces.\n",
            );
        }
        output.push_str(
            "Completion evidence audit: account for every work item and expected contract before declaring completion. Candidate edits establish implementation activity, not behavioral proof; verify behavior at its actual interaction boundary. If evidence is incomplete, keep the obligation unresolved and continue or report the uncertainty.\n",
        );
        output.push_str(
            "Verification planning: choose evidence that exercises each contract at its planned boundary; a nearby helper test is not sufficient when the obligation is public, integrated, stateful, persistent, error-facing, or interactive. Update the plan when a test surface cannot establish the obligation.\n",
        );
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

fn contract_item_id(contract_id: &str) -> Option<String> {
    contract_id
        .strip_prefix("contract-")
        .map(|number| format!("item-{number}"))
}

fn is_test_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("/test")
        || lower.starts_with("test/")
        || lower.starts_with("tests/")
        || lower.ends_with("_test.rs")
}

fn infer_verification_boundary(expectation: &str) -> VerificationBoundary {
    let lower = expectation.to_ascii_lowercase();
    if [
        "interactive",
        "stdin",
        "stdout",
        "terminal",
        "prompt",
        "process",
        "cli",
        "eof",
        "input",
    ]
    .iter()
    .any(|term| lower.contains(term))
    {
        VerificationBoundary::ProcessInteraction
    } else if [
        "compatib",
        "backward",
        "legacy",
        "existing caller",
        "public surface",
        "preserve",
        "unchanged",
    ]
    .iter()
    .any(|term| lower.contains(term))
    {
        VerificationBoundary::Compatibility
    } else if ["persist", "storage", "save", "load", "reload", "survive"]
        .iter()
        .any(|term| lower.contains(term))
    {
        VerificationBoundary::Persistence
    } else if [
        "state",
        "transition",
        "sequence",
        "ordering",
        "order-dependent",
        "idempot",
        "retry",
        "history",
    ]
    .iter()
    .any(|term| lower.contains(term))
    {
        VerificationBoundary::StateTransition
    } else if ["across", "integration", "end-to-end", "subsystem", "module"]
        .iter()
        .any(|term| lower.contains(term))
    {
        VerificationBoundary::Integration
    } else if [
        "error", "invalid", "reject", "cancel", "failure", "recover", "must not",
    ]
    .iter()
    .any(|term| lower.contains(term))
    {
        VerificationBoundary::ErrorPath
    } else if ["api", "caller", "signature", "public"]
        .iter()
        .any(|term| lower.contains(term))
    {
        VerificationBoundary::PublicApi
    } else {
        VerificationBoundary::UnitBehavior
    }
}

fn clauses(request: &str) -> Vec<String> {
    request
        .split(|character: char| {
            character == '.' || character == '!' || character == '?' || character == '\n'
        })
        .flat_map(expand_requirement_clause)
        .map(|clause| clause.trim().to_string())
        .filter(|clause| clause.len() > 8)
        .map(|clause| truncate(&clause, MAX_STATEMENT_BYTES))
        .collect()
}

fn expand_requirement_clause(clause: &str) -> Vec<String> {
    let Some((prefix, list)) = clause.split_once(" for ") else {
        return vec![clause.to_string()];
    };
    let entries = list
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(|entry| entry.strip_prefix("and ").unwrap_or(entry))
        .collect::<Vec<_>>();
    if entries.len() < 2 {
        return vec![clause.to_string()];
    }
    entries
        .into_iter()
        .map(|entry| format!("{prefix} for {entry}"))
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
        assert_eq!(plan.contracts.len(), 2);
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
    fn repository_scope_is_rendered_as_a_hypothesis() {
        let mut plan = TaskPlan::from_request("Update the implementation", None);
        plan.set_repository_scope(
            &["rust/src/lib.rs".to_string()],
            &["rust/src/lib.rs".to_string()],
            &["manifest-backed project: Cargo rust/Cargo.toml".to_string()],
        );
        let rendered = plan.render();
        assert!(rendered.contains("Implementation scope hypothesis"));
        assert!(rendered.contains("rust/src/lib.rs"));
        assert!(rendered.contains("Scope is a bounded hypothesis"));
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

    #[test]
    fn authoritative_request_is_not_limited_by_display_bound() {
        let request = format!(
            "{} Update the provider behavior. Preserve private mode.",
            "context ".repeat(80)
        );
        let plan = TaskPlan::from_request(&request, None);

        assert!(plan.original_request.len() <= MAX_REQUEST_BYTES);
        assert_eq!(plan.authoritative_request(), request.trim());
        assert!(plan
            .authoritative_request()
            .ends_with("Preserve private mode."));
        assert!(plan
            .contracts
            .iter()
            .any(|contract| contract.expectation.contains("Preserve private mode")));
    }

    #[test]
    fn behavioral_list_becomes_bounded_traceable_contracts() {
        let plan = TaskPlan::from_request(
            "Harden the tool for free-form answers, choices, invalid input, and cancellation.",
            None,
        );
        assert_eq!(plan.contracts.len(), 4);
        assert!(plan
            .contracts
            .iter()
            .all(|contract| contract.basis == "user requirement"));
        assert!(!plan.all_contracts_verified());
    }

    #[test]
    fn candidate_evidence_is_not_verified_without_evaluation() {
        let mut plan = TaskPlan::from_request("Update the behavior", None);
        plan.record_candidate_evidence(&["src/tool.rs".to_string()]);
        assert_eq!(plan.contracts[0].status, ContractStatus::CandidateEvidence);
        assert!(!plan.all_contracts_verified());
        assert!(plan.render().contains("Completion evidence audit"));
    }

    #[test]
    fn completion_audit_reopens_unproven_behavioral_contract() {
        let mut plan = TaskPlan::from_request("Handle interactive stdin input", None);
        plan.record_candidate_evidence(&["src/tool.rs".to_string()]);
        let gaps = plan.completion_audit_gaps(
            &["src/tool.rs".to_string()],
            &["No interaction evidence".to_string()],
        );

        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].boundary, VerificationBoundary::ProcessInteraction);
        plan.reopen_for_completion_audit(&gaps[0]);
        assert_eq!(plan.contracts[0].status, ContractStatus::Unresolved);
        assert_eq!(plan.items[0].status, PlanItemStatus::NeedsResearch);
        assert!(plan.render().contains("Completion evidence follow-up"));
    }

    #[test]
    fn completion_audit_accepts_a_changed_test_surface_without_unknown_evidence() {
        let mut plan = TaskPlan::from_request("Update the local behavior", None);
        plan.record_candidate_evidence(&["src/tool.rs".to_string(), "tests/tool.rs".to_string()]);
        assert!(plan
            .completion_audit_gaps(
                &["src/tool.rs".to_string(), "tests/tool.rs".to_string()],
                &[],
            )
            .is_empty());
    }

    #[test]
    fn verification_boundary_is_derived_with_user_requirement_provenance() {
        let plan = TaskPlan::from_request(
            "Handle interactive stdin input. Preserve compatibility. Reject invalid values.",
            None,
        );
        assert_eq!(
            plan.contracts[0].verification_boundary,
            VerificationBoundary::ProcessInteraction
        );
        assert_eq!(
            plan.contracts[1].verification_boundary,
            VerificationBoundary::Compatibility
        );
        assert_eq!(
            plan.contracts[2].verification_boundary,
            VerificationBoundary::ErrorPath
        );
        assert!(plan.contracts[0]
            .verification_basis
            .contains("user requirement"));
        assert!(plan.render().contains("Verification planning"));
    }

    #[test]
    fn insufficient_contract_evidence_reopens_matching_plan_item() {
        let mut plan = TaskPlan::from_request("Handle interactive input.", None);
        plan.mark_implemented("item-1", "candidate writer");
        plan.record_candidate_evidence(&["src/tool.rs".to_string()]);
        plan.record_contract_evidence("contract-1", false, "helper test only");
        assert_eq!(plan.contracts[0].status, ContractStatus::Unresolved);
        assert_eq!(plan.items[0].status, PlanItemStatus::NeedsResearch);
        assert_eq!(plan.items[0].provenance, "helper test only");
    }
}

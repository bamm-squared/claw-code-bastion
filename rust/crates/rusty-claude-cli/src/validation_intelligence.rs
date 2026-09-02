//! Deterministic validation-relevance evidence for the independent evaluator.
//!
//! This module recommends evidence; it never executes checks and never turns
//! graph facts into validation authority. Absent test edges are reported as
//! unknown evidence rather than proof that a change is incorrect.

use crate::task_plan::ExpectedContract;

const MAX_ITEMS: usize = 12;
const MAX_BYTES: usize = 3_200;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidationEvidencePlan {
    pub changed_areas: Vec<String>,
    pub known_relationships: Vec<String>,
    pub missing_evidence: Vec<String>,
}

impl ValidationEvidencePlan {
    #[must_use]
    pub fn render(&self) -> String {
        let mut output = String::from(
            "Validation scope: full configured validation remains mandatory; this is relevance guidance only.\n",
        );
        append_section(&mut output, "Changed areas", &self.changed_areas);
        append_section(
            &mut output,
            "Known deterministic evidence",
            &self.known_relationships,
        );
        append_section(
            &mut output,
            "Missing or unknown evidence",
            &self.missing_evidence,
        );
        if output.len() > MAX_BYTES {
            output.truncate(MAX_BYTES);
            output.push_str("\n[validation relevance truncated]\n");
        }
        output
    }
}

#[must_use]
pub fn analyze(
    changed_paths: &[String],
    repository_context: Option<&str>,
    contracts: &[ExpectedContract],
) -> ValidationEvidencePlan {
    let changed_areas = changed_paths
        .iter()
        .take(MAX_ITEMS)
        .map(|path| format!("candidate changed: {path}"))
        .collect::<Vec<_>>();
    let mut known_relationships = Vec::new();
    let mut missing_evidence = Vec::new();

    for path in changed_paths.iter().take(MAX_ITEMS) {
        let lines = repository_context
            .into_iter()
            .flat_map(str::lines)
            .filter(|line| line.contains(path))
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .take(4)
            .collect::<Vec<_>>();
        if lines.is_empty() {
            missing_evidence.push(format!(
                "No deterministic repository relationship was found for {path}; relevant test coverage is unknown."
            ));
        } else {
            for line in lines {
                known_relationships.push(format!("{path}: {line}"));
            }
        }
        if !is_test_path(path) && !has_explicit_test_relationship(repository_context, path) {
            missing_evidence.push(format!(
                "No explicit deterministic test-to-code relationship is available for {path}."
            ));
        }
    }

    for contract in contracts.iter().take(MAX_ITEMS) {
        if contains_evidence_expectation(&contract.expectation) && missing_evidence.is_empty() {
            missing_evidence.push(format!(
                "{} expects validation evidence, but no missing-evidence signal was recorded.",
                contract.id
            ));
        }
    }
    if changed_paths.is_empty() {
        missing_evidence.push(
            "No candidate changes are available for validation relevance analysis.".to_string(),
        );
    }
    ValidationEvidencePlan {
        changed_areas,
        known_relationships,
        missing_evidence,
    }
}

fn append_section(output: &mut String, title: &str, items: &[String]) {
    output.push_str(title);
    output.push_str(":\n");
    if items.is_empty() {
        output.push_str("- none\n");
    } else {
        for item in items.iter().take(MAX_ITEMS) {
            output.push_str("- ");
            output.push_str(item);
            output.push('\n');
        }
    }
}

fn is_test_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("/test") || lower.starts_with("test/") || lower.ends_with("_test.rs")
}

fn has_explicit_test_relationship(context: Option<&str>, path: &str) -> bool {
    context.into_iter().flat_map(str::lines).any(|line| {
        let lower = line.to_ascii_lowercase();
        line.contains(path)
            && (lower.contains("test") || lower.contains("validation"))
            && !lower.starts_with("file:")
    })
}

fn contains_evidence_expectation(expectation: &str) -> bool {
    let lower = expectation.to_ascii_lowercase();
    ["test", "validation", "evidence", "coverage"]
        .iter()
        .any(|term| lower.contains(term))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_unknown_test_evidence_without_claiming_failure() {
        let plan = analyze(
            &["src/session.rs".to_string()],
            Some("file: src/session.rs\npackage: app\nreferences: src/provider.rs\n"),
            &[],
        );
        assert!(plan
            .missing_evidence
            .iter()
            .any(|item| item.contains("test-to-code")));
        assert!(plan
            .render()
            .contains("full configured validation remains mandatory"));
    }

    #[test]
    fn preserves_explicit_test_relationship_as_known_evidence() {
        let plan = analyze(
            &["src/session.rs".to_string()],
            Some("file: src/session.rs\ntests: src/session.rs <- tests/session.rs\n"),
            &[],
        );
        assert!(plan
            .known_relationships
            .iter()
            .any(|item| item.contains("tests: src/session.rs <- tests/session.rs")));
        assert!(!plan
            .missing_evidence
            .iter()
            .any(|item| item.contains("test-to-code")));
    }
}

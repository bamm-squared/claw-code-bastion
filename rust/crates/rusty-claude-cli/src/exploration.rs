//! Bounded, advisory read-only exploration before candidate writing.
//!
//! Explorer jobs receive deterministic evidence and a focused question. They
//! have no tool surface and therefore cannot mutate a candidate or canonical
//! state. Provider dispatch is supplied by the caller.

use crate::model_router::{ModelProfile, TaskSignals};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::thread;
use std::time::Instant;

const MAX_EXPLORERS: usize = 4;
const MAX_FINDINGS: usize = 12;
const MAX_CONTEXT_BYTES: usize = 12_000;

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum ExplorerKind {
    #[default]
    Architecture,
    Tests,
    Dependencies,
    Risk,
}

impl ExplorerKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Architecture => "architecture",
            Self::Tests => "tests",
            Self::Dependencies => "dependencies",
            Self::Risk => "risk",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExplorerQuestion {
    pub kind: ExplorerKind,
    pub prompt: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExplorerFinding {
    pub kind: String,
    pub subject: String,
    pub claim: String,
    pub evidence: String,
    pub confidence: u8,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExplorerResult {
    pub kind: ExplorerKind,
    pub profile_id: String,
    pub findings: Vec<ExplorerFinding>,
    pub error: Option<String>,
    pub elapsed_ms: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExplorationSynthesis {
    pub launched: usize,
    pub results: Vec<ExplorerResult>,
    pub context: String,
}

pub fn should_explore(signals: TaskSignals) -> bool {
    signals.impacted_modules >= 2
        || signals.dependency_depth >= 2
        || signals.unresolved_relationships > 0
        || signals.ambiguity >= 40
        || signals.security_sensitive
        || signals.concurrency_sensitive
        || signals.public_api_change
}

pub fn questions_for(signals: TaskSignals) -> Vec<ExplorerQuestion> {
    if !should_explore(signals) {
        return Vec::new();
    }
    let mut questions = Vec::new();
    if signals.impacted_modules >= 2 || signals.ambiguity >= 40 {
        questions.push(ExplorerQuestion {
            kind: ExplorerKind::Architecture,
            prompt: "Which execution paths and module boundaries are likely affected? Identify concrete repository evidence and uncertainty; do not propose edits.".to_string(),
        });
    }
    if signals.impacted_modules >= 2 || signals.unresolved_relationships > 0 {
        questions.push(ExplorerQuestion {
            kind: ExplorerKind::Dependencies,
            prompt: "Which callers, dependents, or package relationships should the writer inspect? Distinguish known references from unresolved possibilities.".to_string(),
        });
    }
    if signals.impacted_modules >= 2 || signals.public_api_change || signals.ambiguity >= 40 {
        questions.push(ExplorerQuestion {
            kind: ExplorerKind::Tests,
            prompt: "Which existing tests or validation paths exercise the requested behavior, and what evidence is missing? Use repository facts where available.".to_string(),
        });
    }
    if signals.security_sensitive || signals.concurrency_sensitive {
        questions.push(ExplorerQuestion {
            kind: ExplorerKind::Risk,
            prompt: "What security, privacy, concurrency, or lifecycle risks could the current plan miss? Report bounded evidence and unresolved questions, not certainty.".to_string(),
        });
    }
    questions.truncate(MAX_EXPLORERS);
    questions
}

#[allow(clippy::needless_pass_by_value)]
pub fn run_parallel<F>(
    jobs: Vec<(ExplorerQuestion, ModelProfile)>,
    evidence: String,
    max_concurrent: usize,
    executor: F,
) -> ExplorationSynthesis
where
    F: Fn(&ExplorerQuestion, &ModelProfile, &str) -> Result<Vec<ExplorerFinding>, String>
        + Send
        + Sync,
{
    run_parallel_with_budget(jobs, evidence, max_concurrent, None, executor)
}

#[allow(clippy::needless_pass_by_value)]
pub fn run_parallel_with_budget<F>(
    jobs: Vec<(ExplorerQuestion, ModelProfile)>,
    evidence: String,
    max_concurrent: usize,
    budget: Option<std::time::Duration>,
    executor: F,
) -> ExplorationSynthesis
where
    F: Fn(&ExplorerQuestion, &ModelProfile, &str) -> Result<Vec<ExplorerFinding>, String>
        + Send
        + Sync,
{
    let launched = jobs.len();
    if jobs.is_empty() {
        return ExplorationSynthesis::default();
    }
    let limit = max_concurrent.clamp(1, MAX_EXPLORERS).min(jobs.len());
    let executor = Arc::new(executor);
    let started = Instant::now();
    let mut results = Vec::with_capacity(jobs.len());
    let mut launched = 0;
    for group in jobs.chunks(limit) {
        if budget.is_some_and(|limit| started.elapsed() >= limit) {
            break;
        }
        let group = if limit == 1 { &group[..1] } else { group };
        launched += group.len();
        let group_results = thread::scope(|scope| {
            let handles = group
                .iter()
                .map(|(question, profile)| {
                    let executor = Arc::clone(&executor);
                    let evidence = evidence.clone();
                    scope.spawn(move || {
                        let started = Instant::now();
                        let outcome = executor(question, profile, &evidence);
                        ExplorerResult {
                            kind: question.kind,
                            profile_id: profile.id.clone(),
                            findings: outcome.clone().unwrap_or_default(),
                            error: outcome.err(),
                            elapsed_ms: u64::try_from(started.elapsed().as_millis())
                                .unwrap_or(u64::MAX),
                        }
                    })
                })
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>()
        });
        results.extend(group_results);
    }
    results.sort_by(|a, b| a.kind.cmp(&b.kind).then(a.profile_id.cmp(&b.profile_id)));
    let context = render_context(&results);
    ExplorationSynthesis {
        launched,
        results,
        context,
    }
}

fn render_context(results: &[ExplorerResult]) -> String {
    let mut unique = BTreeMap::<(String, String), Vec<&ExplorerFinding>>::new();
    for result in results {
        for finding in &result.findings {
            if finding.subject.trim().is_empty() || finding.claim.trim().is_empty() {
                continue;
            }
            unique
                .entry((finding.subject.clone(), finding.claim.clone()))
                .or_default()
                .push(finding);
        }
    }
    let mut output = String::from("[Bounded read-only exploration synthesis]\n");
    let mut subjects = BTreeMap::<String, Vec<(String, Vec<&ExplorerFinding>)>>::new();
    for ((subject, claim), findings) in unique {
        subjects.entry(subject).or_default().push((claim, findings));
    }
    for (subject, claims) in subjects {
        let _ = std::fmt::Write::write_fmt(&mut output, format_args!("- {subject}:\n"));
        for (claim, findings) in claims {
            let confidence = findings.iter().map(|f| f.confidence).max().unwrap_or(0);
            let evidence = findings
                .iter()
                .map(|f| f.evidence.as_str())
                .find(|value| !value.trim().is_empty())
                .unwrap_or("no additional evidence");
            let _ = std::fmt::Write::write_fmt(
                &mut output,
                format_args!("  * {claim} (confidence {confidence}; evidence: {evidence})\n"),
            );
        }
    }
    for result in results.iter().filter(|result| result.error.is_some()) {
        let _ = std::fmt::Write::write_fmt(
            &mut output,
            format_args!(
                "- {} exploration unavailable; retain uncertainty.\n",
                result.kind.label()
            ),
        );
    }
    output.truncate(output.len().min(MAX_CONTEXT_BYTES));
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn signals() -> TaskSignals {
        TaskSignals {
            impacted_modules: 3,
            dependency_depth: 2,
            ..TaskSignals::default()
        }
    }

    #[test]
    fn simple_tasks_skip_exploration() {
        assert!(!should_explore(TaskSignals::default()));
        assert!(questions_for(TaskSignals::default()).is_empty());
    }

    #[test]
    fn difficult_tasks_get_distinct_questions() {
        let questions = questions_for(signals());
        assert!(questions.len() >= 3);
        assert!(questions
            .windows(2)
            .all(|pair| pair[0].kind != pair[1].kind));
    }

    #[test]
    fn parallel_runner_is_failure_isolated_and_deduplicates() {
        let profile = ModelProfile::legacy("local-model");
        let jobs = questions_for(signals())
            .into_iter()
            .map(|question| (question, profile.clone()))
            .collect::<Vec<_>>();
        let synthesis = run_parallel(jobs, "facts".to_string(), 3, |question, _, _| {
            if question.kind == ExplorerKind::Dependencies {
                return Err("provider failure".to_string());
            }
            std::thread::sleep(Duration::from_millis(5));
            Ok(vec![ExplorerFinding {
                subject: "src/a.rs".to_string(),
                claim: "inspect caller".to_string(),
                evidence: "graph reference".to_string(),
                confidence: 70,
                ..ExplorerFinding::default()
            }])
        });
        assert_eq!(synthesis.launched, 3);
        assert_eq!(
            synthesis
                .results
                .iter()
                .filter(|r| r.error.is_some())
                .count(),
            1
        );
        assert_eq!(synthesis.context.matches("inspect caller").count(), 1);
        assert!(synthesis.context.len() < MAX_CONTEXT_BYTES);
    }

    #[test]
    fn advisory_budget_keeps_completed_findings_and_skips_remaining_jobs() {
        let profile = ModelProfile::legacy("local-model");
        let jobs = questions_for(signals())
            .into_iter()
            .map(|question| (question, profile.clone()))
            .collect::<Vec<_>>();
        let synthesis = run_parallel_with_budget(
            jobs,
            "facts".to_string(),
            1,
            Some(Duration::from_millis(1)),
            |_, _, _| {
                std::thread::sleep(Duration::from_millis(5));
                Ok(vec![ExplorerFinding {
                    subject: "src/a.rs".to_string(),
                    claim: "completed finding".to_string(),
                    ..ExplorerFinding::default()
                }])
            },
        );
        assert_eq!(synthesis.launched, 1);
        assert_eq!(synthesis.results.len(), 1);
        assert!(synthesis.context.contains("completed finding"));
    }
}

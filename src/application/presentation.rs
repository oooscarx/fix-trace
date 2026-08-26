use fixtrace_presenter::{
    ActionPresentationInput, DiagnosisPresentationInput, SessionSummaryInput, SourceSessionStatus,
    TrialPresentationInput, UsagePresentationInput, present_action, present_diagnosis,
    present_session_summary, present_trial, present_usage,
};
use fixtrace_protocol::{
    ActionView, DependencyEdgeView, DependencyGraphView, DependencyNodeView, DiagnosisView,
    EvidenceClassificationView, EvidenceView, TrialAttemptView, TrialClassification, TrialView,
    UsageSummary,
};

use crate::{
    agent::diagnosis::{Diagnosis, EvidenceClassification},
    config::FixTraceConfig,
    domain::{
        action::{Action, ActionKind},
        session::{SessionRecord, SessionStatus},
        trial::{Trial, TrialOutcome},
    },
    llm::usage::UsageSummary as CoreUsageSummary,
    minimize::dependency::DependencyGraph,
};

pub(super) fn session_summary(session: &SessionRecord) -> fixtrace_protocol::SessionSummary {
    present_session_summary(SessionSummaryInput {
        id: session.id,
        project_name: session.project_name.clone(),
        status: match session.status {
            SessionStatus::Recording => SourceSessionStatus::Recording,
            SessionStatus::ReadyForAnalysis => SourceSessionStatus::ReadyForAnalysis,
            SessionStatus::Analyzed => SourceSessionStatus::Analyzed,
            SessionStatus::Cancelled => SourceSessionStatus::Cancelled,
            SessionStatus::Invalid => SourceSessionStatus::Invalid,
        },
        active_task_id: None,
        parent_session_id: session.parent_session_id,
        archived: session.archived,
        created_at: session.created_at,
        updated_at: session.updated_at,
    })
}

pub(super) fn action_view(action: &Action) -> ActionView {
    let (kind, summary) = match &action.kind {
        ActionKind::ShellCommand { command } => ("shell_command", command.clone()),
        ActionKind::FilePatch { files } => (
            "file_patch",
            format!("Patch {} file{}", files.len(), plural(files.len())),
        ),
        ActionKind::SetEnvironment { key, .. } => {
            ("set_environment", format!("Set environment variable {key}"))
        }
        ActionKind::UnsetEnvironment { key } => (
            "unset_environment",
            format!("Unset environment variable {key}"),
        ),
        ActionKind::ChangeDirectory { path } => (
            "change_directory",
            format!("Change directory to /{}", path.display()),
        ),
    };
    present_action(ActionPresentationInput {
        id: action.id,
        original_order: action.original_order,
        kind: kind.to_owned(),
        cwd: if action.cwd_before.as_os_str().is_empty() {
            "/".to_owned()
        } else {
            format!("/{}", action.cwd_before.display())
        },
        summary,
        replayable: action.replayable,
        note: action.note.clone(),
    })
}

pub(super) fn trial_view(trial: &Trial) -> TrialView {
    let attempts = trial
        .repetitions
        .iter()
        .map(|attempt| {
            let (passed, exit_code, duration_ms, summary) = match &attempt.oracle {
                Some(oracle) => (
                    Some(oracle.passed()),
                    oracle.exit_code,
                    oracle.duration_ms,
                    if oracle.passed() {
                        "Oracle passed".to_owned()
                    } else if oracle.cancelled {
                        "Oracle cancelled".to_owned()
                    } else if oracle.timed_out {
                        "Oracle timed out".to_owned()
                    } else {
                        "Oracle failed".to_owned()
                    },
                ),
                None => (
                    None,
                    None,
                    attempt
                        .actions
                        .iter()
                        .map(|action| action.duration_ms)
                        .sum(),
                    attempt
                        .error
                        .clone()
                        .unwrap_or_else(|| "No Oracle evidence".to_owned()),
                ),
            };
            TrialAttemptView {
                index: attempt.index,
                passed,
                exit_code,
                duration_ms,
                summary,
            }
        })
        .collect();
    present_trial(TrialPresentationInput {
        id: trial.id,
        action_ids: trial.action_ids.clone(),
        classification: trial_classification(&trial.outcome),
        attempts,
    })
}

pub(super) fn trial_classification(outcome: &TrialOutcome) -> TrialClassification {
    match outcome {
        TrialOutcome::StablePass => TrialClassification::StablePass,
        TrialOutcome::StableFail => TrialClassification::StableFail,
        TrialOutcome::Flaky => TrialClassification::Flaky,
        TrialOutcome::Unresolved => TrialClassification::Unresolved,
        TrialOutcome::Cancelled => TrialClassification::Cancelled,
    }
}

pub(super) fn diagnosis_view(diagnosis: &Diagnosis) -> DiagnosisView {
    present_diagnosis(DiagnosisPresentationInput {
        statement: diagnosis.statement.clone(),
        minimal_action_ids: diagnosis.minimal_action_ids.clone(),
        evidence: diagnosis
            .evidence
            .iter()
            .map(|evidence| EvidenceView {
                claim: evidence.claim.clone(),
                classification: match evidence.classification {
                    EvidenceClassification::Necessary => EvidenceClassificationView::Necessary,
                    EvidenceClassification::Removable => EvidenceClassificationView::Removable,
                    EvidenceClassification::Uncertain => EvidenceClassificationView::Uncertain,
                    EvidenceClassification::Untested => EvidenceClassificationView::Untested,
                    EvidenceClassification::NonReplayable => {
                        EvidenceClassificationView::NonReplayable
                    }
                },
                action_ids: evidence.action_ids.clone(),
                trial_ids: evidence.trial_ids.clone(),
            })
            .collect(),
        limitations: diagnosis.limitations.clone(),
    })
}

pub(super) fn usage_view(usage: &CoreUsageSummary, config: &FixTraceConfig) -> UsageSummary {
    present_usage(UsagePresentationInput {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        total_cost_usd: usage.total_cost_usd,
        token_limit: config.budget.max_total_tokens,
        cost_limit_usd: config.budget.max_cost_usd,
        exact: usage.unknown_usage_calls == 0,
    })
}

pub(super) fn dependency_graph_view(
    graph: &DependencyGraph,
    actions: &[Action],
    minimal_action_ids: &[u64],
) -> DependencyGraphView {
    let minimal: std::collections::BTreeSet<_> = minimal_action_ids.iter().copied().collect();
    let nodes = actions
        .iter()
        .map(|action| DependencyNodeView {
            action_id: action.id,
            label: action_view(action).summary,
            in_minimal_set: minimal.contains(&action.id),
        })
        .collect();
    let edges = graph
        .hard_dependencies
        .iter()
        .flat_map(|(action_id, dependencies)| {
            dependencies.iter().map(|dependency| DependencyEdgeView {
                from_action_id: *dependency,
                to_action_id: *action_id,
                reason: "hard dependency".to_owned(),
            })
        })
        .collect();
    DependencyGraphView { nodes, edges }
}

const fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

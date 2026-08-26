use chrono::{DateTime, Utc};
use fixtrace_protocol::{
    ActionView, ApprovalView, DependencyGraphView, DiagnosisView, DiffView,
    EvidenceClassificationView, EvidenceView, SessionStatusView, SessionSummary, SessionView,
    TaskStatus, TaskSummary, TimelineItem, TrialAttemptView, TrialClassification, TrialView,
    UsageSummary,
};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceSessionStatus {
    Recording,
    ReadyForAnalysis,
    Analyzing,
    Analyzed,
    Cancelled,
    Invalid,
    Archived,
}

pub struct SessionSummaryInput {
    pub id: Uuid,
    pub project_name: String,
    pub project_path: String,
    pub status: SourceSessionStatus,
    pub active_task_id: Option<Uuid>,
    pub parent_session_id: Option<Uuid>,
    pub archived: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub fn present_session_summary(input: SessionSummaryInput) -> SessionSummary {
    SessionSummary {
        id: input.id,
        project_name: input.project_name,
        project_path: input.project_path,
        status: match input.status {
            SourceSessionStatus::Recording => SessionStatusView::Recording,
            SourceSessionStatus::ReadyForAnalysis => SessionStatusView::ReadyForAnalysis,
            SourceSessionStatus::Analyzing => SessionStatusView::Analyzing,
            SourceSessionStatus::Analyzed => SessionStatusView::Analyzed,
            SourceSessionStatus::Cancelled => SessionStatusView::Cancelled,
            SourceSessionStatus::Invalid => SessionStatusView::Invalid,
            SourceSessionStatus::Archived => SessionStatusView::Archived,
        },
        active_task_id: input.active_task_id,
        parent_session_id: input.parent_session_id,
        archived: input.archived,
        created_at: input.created_at,
        updated_at: input.updated_at,
    }
}

pub struct ActionPresentationInput {
    pub id: u64,
    pub original_order: u64,
    pub kind: String,
    pub cwd: String,
    pub summary: String,
    pub replayable: bool,
    pub reads: Vec<String>,
    pub writes: Vec<String>,
    pub resource_access_opaque: bool,
    pub note: Option<String>,
}

pub fn present_action(input: ActionPresentationInput) -> ActionView {
    ActionView {
        id: input.id,
        original_order: input.original_order,
        kind: input.kind,
        cwd: input.cwd,
        summary: input.summary,
        replayable: input.replayable,
        can_rerun: input.replayable,
        reads: input.reads,
        writes: input.writes,
        resource_access_opaque: input.resource_access_opaque,
        note: input.note,
    }
}

pub struct TrialPresentationInput {
    pub id: Uuid,
    pub action_ids: Vec<u64>,
    pub classification: TrialClassification,
    pub attempts: Vec<TrialAttemptView>,
}

pub fn present_trial(input: TrialPresentationInput) -> TrialView {
    let trial_summary = format!(
        "{} · {} action{} · {} repetition{}",
        classification_label(&input.classification),
        input.action_ids.len(),
        plural(input.action_ids.len()),
        input.attempts.len(),
        plural(input.attempts.len())
    );
    TrialView {
        id: input.id,
        action_ids: input.action_ids,
        can_rerun: input.classification != TrialClassification::Cancelled,
        classification: input.classification,
        attempts: input.attempts,
        trial_summary,
    }
}

pub struct DiagnosisPresentationInput {
    pub statement: String,
    pub minimal_action_ids: Vec<u64>,
    pub evidence: Vec<EvidenceView>,
    pub limitations: Vec<String>,
}

pub fn present_diagnosis(input: DiagnosisPresentationInput) -> DiagnosisView {
    let confidence = if input.evidence.is_empty() {
        "unknown"
    } else if input.limitations.is_empty()
        && input.evidence.iter().all(|evidence| {
            matches!(
                evidence.classification,
                EvidenceClassificationView::Necessary | EvidenceClassificationView::Removable
            )
        })
    {
        "high"
    } else {
        "limited"
    };
    let diagnosis_summary = format!(
        "{} minimal action{} · {} evidence item{} · {} confidence",
        input.minimal_action_ids.len(),
        plural(input.minimal_action_ids.len()),
        input.evidence.len(),
        plural(input.evidence.len()),
        confidence
    );
    DiagnosisView {
        statement: input.statement,
        minimal_action_ids: input.minimal_action_ids,
        evidence: input.evidence,
        limitations: input.limitations,
        confidence: confidence.to_owned(),
        diagnosis_summary,
    }
}

pub struct UsagePresentationInput {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_cost_usd: f64,
    pub token_limit: u64,
    pub cost_limit_usd: f64,
    pub exact: bool,
}

pub fn present_usage(input: UsagePresentationInput) -> UsageSummary {
    let total_tokens = input.input_tokens.saturating_add(input.output_tokens);
    let token_ratio = if input.token_limit == 0 {
        0.0
    } else {
        total_tokens as f64 / input.token_limit as f64
    };
    let cost_ratio = if input.cost_limit_usd <= 0.0 {
        0.0
    } else {
        input.total_cost_usd / input.cost_limit_usd
    };
    UsageSummary {
        input_tokens: input.input_tokens,
        output_tokens: input.output_tokens,
        total_tokens,
        total_cost_usd: input.total_cost_usd,
        token_limit: input.token_limit,
        cost_limit_usd: input.cost_limit_usd,
        budget_ratio: token_ratio.max(cost_ratio),
        exact: input.exact,
    }
}

pub fn task_is_cancellable(status: TaskStatus) -> bool {
    matches!(
        status,
        TaskStatus::Queued | TaskStatus::Running | TaskStatus::WaitingForApproval
    )
}

pub struct SessionViewInput {
    pub summary: SessionSummary,
    pub task: Option<TaskSummary>,
    pub timeline: Vec<TimelineItem>,
    pub actions: Vec<ActionView>,
    pub trials: Vec<TrialView>,
    pub diagnosis: Option<DiagnosisView>,
    pub usage: UsageSummary,
    pub approvals: Vec<ApprovalView>,
    pub dependency_graph: DependencyGraphView,
    pub diff: DiffView,
}

pub fn present_session(input: SessionViewInput) -> SessionView {
    SessionView {
        summary: input.summary,
        task: input.task.map(|mut task| {
            task.is_cancellable = task_is_cancellable(task.status);
            task
        }),
        timeline: input.timeline,
        actions: input.actions,
        trials: input.trials,
        diagnosis: input.diagnosis,
        usage: input.usage,
        approvals: input.approvals,
        dependency_graph: input.dependency_graph,
        diff: input.diff,
    }
}

pub trait HumanDisplay {
    fn title(&self) -> String;
    fn short_summary(&self) -> String;
    fn status_label(&self) -> &'static str;
}

impl HumanDisplay for SessionSummary {
    fn title(&self) -> String {
        self.project_name.clone()
    }

    fn short_summary(&self) -> String {
        format!("{} · {}", self.project_name, self.status_label())
    }

    fn status_label(&self) -> &'static str {
        match self.status {
            SessionStatusView::Recording => "Recording",
            SessionStatusView::ReadyForAnalysis => "Ready",
            SessionStatusView::Analyzing => "Analyzing",
            SessionStatusView::Analyzed => "Analyzed",
            SessionStatusView::Cancelled => "Cancelled",
            SessionStatusView::Invalid => "Invalid",
            SessionStatusView::Archived => "Archived",
        }
    }
}

impl HumanDisplay for TrialView {
    fn title(&self) -> String {
        format!("Trial {}", self.id)
    }

    fn short_summary(&self) -> String {
        self.trial_summary.clone()
    }

    fn status_label(&self) -> &'static str {
        classification_label(&self.classification)
    }
}

const fn classification_label(classification: &TrialClassification) -> &'static str {
    match classification {
        TrialClassification::StablePass => "Stable pass",
        TrialClassification::StableFail => "Stable fail",
        TrialClassification::Flaky => "Flaky",
        TrialClassification::Unresolved => "Unresolved",
        TrialClassification::Cancelled => "Cancelled",
    }
}

const fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use fixtrace_protocol::{EvidenceClassificationView, EvidenceView, TrialClassification};

    use super::{
        DiagnosisPresentationInput, TrialPresentationInput, UsagePresentationInput,
        present_diagnosis, present_trial, present_usage,
    };

    #[test]
    fn usage_ratio_uses_the_tighter_budget() {
        let usage = present_usage(UsagePresentationInput {
            input_tokens: 600,
            output_tokens: 200,
            total_cost_usd: 0.25,
            token_limit: 1_000,
            cost_limit_usd: 1.0,
            exact: true,
        });
        assert_eq!(usage.total_tokens, 800);
        assert!((usage.budget_ratio - 0.8).abs() < f64::EPSILON);
    }

    #[test]
    fn diagnosis_confidence_is_derived_once_in_rust() {
        let diagnosis = present_diagnosis(DiagnosisPresentationInput {
            statement: "verified".to_owned(),
            minimal_action_ids: vec![5, 6],
            evidence: vec![EvidenceView {
                claim: "action 5 is necessary".to_owned(),
                classification: EvidenceClassificationView::Necessary,
                action_ids: vec![5],
                trial_ids: Vec::new(),
            }],
            limitations: Vec::new(),
        });
        assert_eq!(diagnosis.confidence, "high");
        assert!(diagnosis.diagnosis_summary.contains("2 minimal actions"));
    }

    #[test]
    fn trial_summary_is_shared_by_all_clients() {
        let trial = present_trial(TrialPresentationInput {
            id: uuid::Uuid::new_v4(),
            action_ids: vec![5, 6],
            classification: TrialClassification::StablePass,
            attempts: Vec::new(),
        });
        assert_eq!(
            trial.trial_summary,
            "Stable pass · 2 actions · 0 repetitions"
        );
    }
}

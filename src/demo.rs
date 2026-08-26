use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    agent::{
        diagnosis::Diagnosis,
        loop_runner::{AgentHistory, AgentRunResult, run_agent},
        tools::AnalysisTools,
    },
    config::FixTraceConfig,
    domain::{action::Action, trial::TrialOutcome},
    error::AppError,
    llm::{
        mock::MockProvider,
        provider::{LlmResponse, ToolCall},
        usage::{Usage, UsageObservation},
    },
    minimize::engine::{AblationEvidence, minimize},
    replay::{oracle::OracleSpec, runner::TrialRunner},
};

#[derive(Debug, Deserialize)]
struct DemoTrace {
    oracle: OracleSpec,
    repetitions: u32,
    actions: Vec<Action>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DemoReport {
    mode: &'static str,
    baseline_hash: String,
    baseline_trial_id: Uuid,
    baseline_outcome: TrialOutcome,
    full_trial_id: Uuid,
    full_outcome: TrialOutcome,
    minimal_action_ids: Vec<u64>,
    final_trial_id: Uuid,
    final_outcome: TrialOutcome,
    ablations: Vec<AblationEvidence>,
    trial_count: usize,
    statement: String,
    diagnosis: Diagnosis,
    agent: Option<AgentRunResult>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub enum DemoOutput {
    Completed(Box<DemoReport>),
    Cancelled { outcome: &'static str },
}

pub async fn run_demo(
    no_llm: bool,
    cancellation: CancellationToken,
) -> Result<DemoOutput, AppError> {
    let trace = load_demo_trace()?;
    let project = demo_project_path();
    let runner = TrialRunner::new(project, trace.oracle, trace.repetitions, false)?;

    let report = match minimize(&runner, &trace.actions, &cancellation).await {
        Ok(report) => report,
        Err(_) if cancellation.is_cancelled() => return Ok(cancelled()),
        Err(error) => return Err(error),
    };
    if report.empty_trial.outcome == TrialOutcome::Cancelled {
        return Ok(cancelled());
    }
    if report.minimal_action_ids != [5, 6] {
        return Err(AppError::DemoVerification(format!(
            "expected minimal actions [5, 6], got {:?}",
            report.minimal_action_ids
        )));
    }
    if report.ablations.len() != 2
        || report
            .ablations
            .iter()
            .any(|ablation| ablation.outcome != TrialOutcome::StableFail)
    {
        return Err(AppError::DemoVerification(
            "both final action ablations must be StableFail".to_owned(),
        ));
    }

    let offline_diagnosis = Diagnosis::offline(&report);
    let (diagnosis, agent) = if no_llm {
        (offline_diagnosis, None)
    } else {
        let provider = MockProvider::new([
            LlmResponse {
                content: None,
                tool_calls: vec![ToolCall {
                    id: "demo-tool-1".to_owned(),
                    name: "run_minimizer".to_owned(),
                    arguments: serde_json::json!({}),
                }],
                usage: mock_usage(),
                request_id: Some("mock-demo-1".to_owned()),
                model: Some("fixtrace-mock".to_owned()),
            },
            LlmResponse {
                content: Some(serde_json::to_string(&offline_diagnosis)?),
                tool_calls: Vec::new(),
                usage: mock_usage(),
                request_id: Some("mock-demo-2".to_owned()),
                model: Some("fixtrace-mock".to_owned()),
            },
        ]);
        let mut tools = AnalysisTools::new(
            &runner,
            &trace.actions,
            &report,
            None,
            None,
            cancellation.clone(),
        );
        let run = run_agent(
            &provider,
            &mut tools,
            &FixTraceConfig::default(),
            cancellation,
            None,
            AgentHistory::none(),
        )
        .await?;
        let diagnosis = run.diagnosis.clone().ok_or_else(|| {
            AppError::DemoVerification(format!(
                "MockProvider agent stopped without diagnosis: {:?}",
                run.stop_reason
            ))
        })?;
        (diagnosis, Some(run))
    };

    Ok(DemoOutput::Completed(Box::new(DemoReport {
        mode: if no_llm {
            "offline-no-llm"
        } else {
            "mock-provider"
        },
        baseline_hash: runner.baseline_hash().to_owned(),
        baseline_trial_id: report.empty_trial.id,
        baseline_outcome: report.empty_trial.outcome,
        full_trial_id: report.full_trial.id,
        full_outcome: report.full_trial.outcome,
        minimal_action_ids: report.minimal_action_ids,
        final_trial_id: report.final_trial.id,
        final_outcome: report.final_trial.outcome,
        ablations: report.ablations,
        trial_count: report.trials.len(),
        statement: report.statement,
        diagnosis,
        agent,
    })))
}

fn mock_usage() -> UsageObservation {
    UsageObservation::Known {
        usage: Usage {
            input_tokens: 20,
            output_tokens: 10,
        },
    }
}

fn load_demo_trace() -> Result<DemoTrace, AppError> {
    Ok(serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/demo/trace.json"
    )))?)
}

fn demo_project_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("demo/broken-project")
}

const fn cancelled() -> DemoOutput {
    DemoOutput::Cancelled {
        outcome: "cancelled",
    }
}

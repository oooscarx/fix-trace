use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    domain::{action::Action, trial::TrialOutcome},
    error::AppError,
    minimize::engine::{AblationEvidence, minimize},
    replay::{oracle::OracleSpec, runner::TrialRunner},
};

#[derive(Debug, Deserialize)]
struct DemoTrace {
    oracle: OracleSpec,
    repetitions: u32,
    actions: Vec<Action>,
}

#[derive(Debug, Serialize)]
struct DemoReport {
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
}

pub async fn run_demo(no_llm: bool, cancellation: CancellationToken) -> Result<(), AppError> {
    let trace = load_demo_trace()?;
    let project = demo_project_path();
    let runner = TrialRunner::new(project, trace.oracle, trace.repetitions, false)?;

    let report = match minimize(&runner, &trace.actions, &cancellation).await {
        Ok(report) => report,
        Err(_) if cancellation.is_cancelled() => return print_cancelled(),
        Err(error) => return Err(error),
    };
    if report.empty_trial.outcome == TrialOutcome::Cancelled {
        return print_cancelled();
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

    let output = DemoReport {
        mode: if no_llm {
            "offline-no-llm"
        } else {
            "deterministic-core-only"
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
    };
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
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

fn print_cancelled() -> Result<(), AppError> {
    println!(r#"{{"outcome":"cancelled"}}"#);
    Ok(())
}

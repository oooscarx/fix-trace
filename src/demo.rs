use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    domain::{action::Action, trial::TrialOutcome},
    error::AppError,
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
    action_ids: Vec<u64>,
    statement: &'static str,
}

pub async fn run_demo(no_llm: bool, cancellation: CancellationToken) -> Result<(), AppError> {
    let trace = load_demo_trace()?;
    let project = demo_project_path();
    let runner = TrialRunner::new(project, trace.oracle, trace.repetitions, false)?;

    let baseline = runner.run(&[], &cancellation).await?;
    if baseline.outcome == TrialOutcome::Cancelled {
        return print_cancelled();
    }
    if baseline.outcome != TrialOutcome::StableFail {
        return Err(AppError::DemoVerification(format!(
            "empty trace must be StableFail, got {:?}",
            baseline.outcome
        )));
    }

    let full = runner.run(&trace.actions, &cancellation).await?;
    if full.outcome == TrialOutcome::Cancelled {
        return print_cancelled();
    }
    if full.outcome != TrialOutcome::StablePass {
        return Err(AppError::DemoVerification(format!(
            "full trace must be StablePass, got {:?}",
            full.outcome
        )));
    }

    let report = DemoReport {
        mode: if no_llm {
            "offline-no-llm"
        } else {
            "deterministic-core-only"
        },
        baseline_hash: runner.baseline_hash().to_owned(),
        baseline_trial_id: baseline.id,
        baseline_outcome: baseline.outcome,
        full_trial_id: full.id,
        full_outcome: full.outcome,
        action_ids: full.action_ids,
        statement: "The complete trace is replay-sufficient for this baseline and Oracle; minimization is added in M2.",
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
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

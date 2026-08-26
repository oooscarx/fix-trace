use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::error::AppError;

use super::executor::{ProcessEvidence, ReplayState, run_shell_command};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OracleSpec {
    pub command: String,
    pub timeout_ms: u64,
}

impl OracleSpec {
    pub fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout_ms)
    }
}

pub async fn run_oracle(
    spec: &OracleSpec,
    state: &ReplayState,
    cancellation: &CancellationToken,
) -> Result<ProcessEvidence, AppError> {
    run_shell_command(
        state.root(),
        state.cwd(),
        state.environment(),
        &spec.command,
        spec.timeout(),
        cancellation,
    )
    .await
}

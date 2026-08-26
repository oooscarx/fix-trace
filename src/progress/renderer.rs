use tokio::{sync::broadcast, task::JoinHandle};

use super::ProgressEvent;

pub fn spawn(mut receiver: broadcast::Receiver<ProgressEvent>) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match receiver.recv().await {
                Ok(event) => render(&event),
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    eprintln!("[fixtrace] progress receiver lagged; skipped {skipped} updates");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

fn render(event: &ProgressEvent) {
    match event {
        ProgressEvent::SessionCreated { session_id } => {
            eprintln!("[fixtrace] session created: {session_id}");
        }
        ProgressEvent::BaselineCopied => eprintln!("[fixtrace] baseline copied"),
        ProgressEvent::OracleAttemptStarted { current, total } => {
            eprintln!("[fixtrace] Oracle attempt {current}/{total}");
        }
        ProgressEvent::ActionReplayStarted { action_id } => {
            eprintln!("[fixtrace] replaying action {action_id}");
        }
        ProgressEvent::TrialStarted {
            trial_id,
            current,
            total,
        } => eprintln!("[fixtrace] trial {trial_id}: repetition {current}/{total}"),
        ProgressEvent::TrialCompleted { trial_id, outcome } => {
            eprintln!("[fixtrace] trial {trial_id}: {outcome:?}");
        }
        ProgressEvent::CandidateReduced { before, after } => {
            eprintln!("[fixtrace] candidate reduced: {before} -> {after}");
        }
        ProgressEvent::AgentStepStarted { step } => {
            eprintln!("[fixtrace] agent step {step}");
        }
        ProgressEvent::AgentMessageStarted { .. } => {
            eprintln!("[fixtrace] agent response started");
        }
        ProgressEvent::AgentTextDelta { .. } => {}
        ProgressEvent::AgentMessageCompleted { .. } => {
            eprintln!("[fixtrace] agent response completed");
        }
        ProgressEvent::ToolCallStarted { name, .. } => {
            eprintln!("[fixtrace] tool call started: {name}");
        }
        ProgressEvent::ToolCallCompleted { name, .. } => {
            eprintln!("[fixtrace] tool call completed: {name}");
        }
        ProgressEvent::UsageUpdated {
            input_tokens,
            output_tokens,
            cost_usd,
        } => eprintln!(
            "[fixtrace] usage: {input_tokens} input + {output_tokens} output tokens, ${cost_usd:.6}"
        ),
        ProgressEvent::Cancelled => eprintln!("[fixtrace] cancelled"),
        ProgressEvent::Finished => eprintln!("[fixtrace] finished"),
    }
}

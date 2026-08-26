use std::{path::Path, sync::Arc, time::Duration};

use chrono::Utc;
use fixtrace::{
    application::{AppServiceOptions, FixTraceAppService},
    config::FixTraceConfig,
    domain::{
        action::Action,
        session::{SessionRecord, SessionStatus},
        snapshot::SnapshotManifest,
    },
    history::database::HistoryDatabase,
    replay::oracle::OracleSpec,
};
use fixtrace_client::{AppClient, InProcessClient};
use fixtrace_protocol::{
    AppEvent, AppRequest, AppResponsePayload, ClientCapabilities, InitializeRequest,
    MessageSendRequest, PROTOCOL_VERSION, SubscribeRequest,
};
use serde::Deserialize;
use tempfile::tempdir;
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Deserialize)]
struct DemoTrace {
    oracle: OracleSpec,
    actions: Vec<Action>,
}

#[tokio::test]
async fn message_send_streams_real_trial_and_agent_timeline_events() {
    let temp = tempdir().unwrap();
    let state = temp.path().join("state");
    std::fs::create_dir_all(state.join("sessions")).unwrap();
    let mut config = FixTraceConfig::default();
    config.replay.repetitions = 1;
    config.model.api_key_env = "FIXTRACE_TEST_INTENTIONALLY_UNSET_KEY".to_owned();
    config.save(&state.join("config.toml")).unwrap();

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let baseline = root.join("demo/broken-project");
    let trace: DemoTrace =
        serde_json::from_str(&std::fs::read_to_string(root.join("demo/trace.json")).unwrap())
            .unwrap();
    let session_id = Uuid::new_v4();
    let now = Utc::now();
    let database = HistoryDatabase::open(state.join("history.sqlite3")).unwrap();
    database
        .save_session(&SessionRecord {
            id: session_id,
            project_name: "tui-vertical".to_owned(),
            original_project: baseline.clone(),
            baseline_path: baseline.clone(),
            worktree_path: baseline.clone(),
            oracle: trace.oracle,
            baseline_manifest: SnapshotManifest::capture(&baseline, false).unwrap(),
            status: SessionStatus::ReadyForAnalysis,
            created_at: now,
            updated_at: now,
        })
        .unwrap();
    for action in trace.actions {
        database.save_action(session_id, &action).unwrap();
    }

    let service = Arc::new(
        FixTraceAppService::start(
            AppServiceOptions {
                state_dir: Some(state),
                config_path: None,
                initialize_event_store: true,
            },
            CancellationToken::new(),
        )
        .unwrap(),
    );
    let client = InProcessClient::new(service);
    client
        .initialize(InitializeRequest {
            protocol_version: PROTOCOL_VERSION.to_owned(),
            client: InProcessClient::client_info(),
            capabilities: ClientCapabilities {
                supports_streaming: true,
                supports_approvals: true,
                supports_diff: true,
                supports_graph: true,
                supports_artifacts: true,
            },
        })
        .await
        .unwrap();
    let mut events = client
        .subscribe(SubscribeRequest {
            session_id,
            after_sequence: Some(0),
        })
        .await
        .unwrap();
    let response = client
        .request(AppRequest::MessageSend(MessageSendRequest {
            session_id,
            text: "Explain the verified minimal repair.".to_owned(),
        }))
        .await
        .unwrap();
    let AppResponsePayload::Task(task) = response else {
        panic!("message/send must start an Agent task")
    };
    assert_eq!(task.session_id, Some(session_id));

    let mut user = false;
    let mut trial = false;
    let mut agent_started = false;
    let mut agent_delta = false;
    let mut agent_completed = false;
    loop {
        let event = timeout(Duration::from_secs(45), events.recv())
            .await
            .expect("Agent turn should finish")
            .expect("event stream should remain contiguous");
        match event.payload {
            AppEvent::ItemCompleted(fixtrace_protocol::TimelineItem::UserMessage(_)) => user = true,
            AppEvent::ItemStarted(fixtrace_protocol::TimelineItem::Trial(_)) => trial = true,
            AppEvent::ItemStarted(fixtrace_protocol::TimelineItem::AgentMessage(_)) => {
                agent_started = true;
            }
            AppEvent::ItemDelta(fixtrace_protocol::ItemDelta::AgentMessage(_)) => {
                agent_delta = true;
            }
            AppEvent::ItemCompleted(fixtrace_protocol::TimelineItem::AgentMessage(_)) => {
                agent_completed = true;
            }
            AppEvent::TaskCompleted(result) if result.task.id == task.id => break,
            AppEvent::TaskFailed(failure) if failure.task.id == task.id => {
                panic!("Agent turn failed: {}", failure.error.message)
            }
            _ => {}
        }
    }
    assert!(user, "user message must be on the shared timeline");
    assert!(trial, "real minimization trials must stream to the UI");
    assert!(agent_started && agent_delta && agent_completed);
}

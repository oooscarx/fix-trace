use std::fs;

use fixtrace::{
    application::{
        AppCommand, AppResponse, AppServiceOptions, FixTraceAppService, FixTraceApplication,
    },
    progress::ProgressEvent,
};
use tempfile::tempdir;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn app_service_is_the_stateful_entry_point_used_without_cli_types() {
    let temp = tempdir().expect("temporary directory should be created");
    let state_dir = temp.path().join("state");
    let project = temp.path().join("project");
    fs::create_dir(&project).expect("project fixture should be created");
    fs::write(project.join("fixture.txt"), "broken").expect("project fixture should be written");

    let service = FixTraceAppService::start(
        AppServiceOptions {
            state_dir: Some(state_dir),
            config_path: None,
        },
        CancellationToken::new(),
    )
    .expect("App Service should start");
    let mut progress = service.subscribe_progress();

    let saved = service
        .execute(AppCommand::SetConfig {
            key: "replay.repetitions".to_owned(),
            value: "1".to_owned(),
        })
        .await
        .expect("configuration should update through App Service");
    assert!(matches!(saved, AppResponse::ConfigSaved { .. }));

    let config = service
        .execute(AppCommand::GetConfig)
        .await
        .expect("configuration should load through App Service");
    let AppResponse::Config { toml } = config else {
        panic!("GetConfig returned the wrong response");
    };
    assert!(toml.contains("repetitions = 1"));

    let initialized = service
        .execute(AppCommand::InitializeSession {
            project,
            oracle: "false".to_owned(),
        })
        .await
        .expect("session should initialize through App Service");
    let AppResponse::SessionInitialized { session } = initialized else {
        panic!("InitializeSession returned the wrong response");
    };
    assert!(session.baseline_path.is_dir());
    assert!(session.worktree_path.is_dir());

    let sessions = service
        .execute(AppCommand::ListSessions)
        .await
        .expect("history should load through App Service");
    let AppResponse::Sessions { sessions } = sessions else {
        panic!("ListSessions returned the wrong response");
    };
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, session.id);

    let mut saw_created = false;
    while let Ok(event) = progress.try_recv() {
        if event
            == (ProgressEvent::SessionCreated {
                session_id: session.id,
            })
        {
            saw_created = true;
        }
    }
    assert!(saw_created, "App Service should publish workflow progress");
}

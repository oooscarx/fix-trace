use chrono::{DateTime, Utc};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use fixtrace_protocol::*;
use fixtrace_tui::{ConnectionMode, Effect, InspectorTab, Model, Theme, TuiEvent, render, update};
use ratatui::{Terminal, backend::TestBackend};
use uuid::Uuid;

#[test]
fn wide_layout_snapshot() {
    insta::assert_snapshot!(rendered(140, 38, InspectorTab::Overview));
}

#[test]
fn medium_layout_snapshot() {
    insta::assert_snapshot!(rendered(100, 32, InspectorTab::Trials));
}

#[test]
fn narrow_layout_snapshot() {
    insta::assert_snapshot!(rendered(72, 26, InspectorTab::Graph));
}

#[test]
fn too_small_terminal_snapshot() {
    let model = fixture_model();
    let backend = TestBackend::new(52, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| render(frame, &model)).unwrap();
    insta::assert_snapshot!(buffer_text(terminal.backend(), 52, 12));
}

#[test]
fn server_delta_updates_the_running_agent_item() {
    let mut model = fixture_model();
    let event = EventEnvelope {
        schema_version: EVENT_SCHEMA_VERSION,
        stream_id: uuid("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),
        sequence: 1,
        event_id: uuid("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"),
        timestamp: timestamp(),
        session_id: model.selected_session_id,
        task_id: model.active_task().map(|task| task.id),
        payload: AppEvent::ItemDelta(ItemDelta::AgentMessage(AgentMessageDelta {
            item_id: uuid("77777777-7777-4777-8777-777777777777"),
            text_delta: " Streaming is live.".to_owned(),
        })),
    };
    assert!(update(&mut model, TuiEvent::Server(Box::new(event))).is_empty());
    let text = model
        .session
        .unwrap()
        .timeline
        .into_iter()
        .find_map(|item| match item {
            TimelineItem::AgentMessage(item) => Some(item.text),
            _ => None,
        })
        .unwrap();
    assert!(text.ends_with("Streaming is live."));
}

#[test]
fn first_ctrl_c_cancels_task_and_second_exits() {
    let mut model = fixture_model();
    let ctrl_c = TuiEvent::Terminal(Event::Key(KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
    )));
    let effects = update(&mut model, ctrl_c.clone());
    assert!(matches!(effects.as_slice(), [Effect::CancelTask(_)]));
    assert!(!model.should_quit);
    assert!(update(&mut model, ctrl_c).is_empty());
    assert!(model.should_quit);
}

#[test]
fn slash_commands_create_record_and_update_configuration() {
    let mut model = fixture_model();
    let effects = submit_text(
        &mut model,
        "/new '/tmp/project with space' --oracle 'cargo test --all' --title repair",
    );
    assert!(matches!(
        effects.as_slice(),
        [Effect::CreateSession { project, oracle, title }]
            if project == std::path::Path::new("/tmp/project with space")
                && oracle == "cargo test --all"
                && title.as_deref() == Some("repair")
    ));

    let effects = submit_text(&mut model, "/record printf fixed '>' fixture.txt");
    assert!(matches!(
        effects.as_slice(),
        [Effect::StartTask {
            input: TaskInput::RecordTrace { line },
            ..
        }] if line == "printf fixed > fixture.txt"
    ));

    let effects = submit_text(&mut model, "/permissions read_only");
    assert!(matches!(
        effects.as_slice(),
        [Effect::UpdateConfig { key, value: ConfigValue::String(value) }]
            if key == "approval.policy" && value == "read_only"
    ));
}

#[test]
fn theme_and_entity_reference_completion_are_ui_state_only() {
    let mut model = fixture_model();
    assert!(submit_text(&mut model, "/theme mono").is_empty());
    assert_eq!(model.theme, Theme::Monochrome);

    model.replace_composer("Please inspect @act");
    let effects = update(
        &mut model,
        TuiEvent::Terminal(Event::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))),
    );
    assert!(effects.is_empty());
    assert_eq!(model.composer_text(), "Please inspect @action:5 ");
}

#[test]
fn natural_message_steers_an_active_agent_task() {
    let mut model = fixture_model();
    let task_id = model.active_task().unwrap().id;
    let effects = submit_text(&mut model, "Focus on the permission change.");
    assert!(matches!(
        effects.as_slice(),
        [Effect::SteerTask { task_id: actual, text }]
            if *actual == task_id && text == "Focus on the permission change."
    ));
}

fn submit_text(model: &mut Model, text: &str) -> Vec<Effect> {
    model.modal = None;
    model.replace_composer(text);
    update(
        model,
        TuiEvent::Terminal(Event::Key(KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::NONE,
        ))),
    )
}

fn rendered(width: u16, height: u16, tab: InspectorTab) -> String {
    let mut model = fixture_model();
    model.inspector_tab = tab;
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| render(frame, &model)).unwrap();
    buffer_text(terminal.backend(), width, height)
}

fn buffer_text(backend: &TestBackend, width: u16, height: u16) -> String {
    let buffer = backend.buffer();
    let mut output = String::new();
    for y in 0..height {
        for x in 0..width {
            output.push_str(buffer[(x, y)].symbol());
        }
        if y + 1 < height {
            output.push('\n');
        }
    }
    output
}

fn fixture_model() -> Model {
    let session_id = uuid("11111111-1111-4111-8111-111111111111");
    let task_id = uuid("22222222-2222-4222-8222-222222222222");
    let now = timestamp();
    let summary = SessionSummary {
        id: session_id,
        project_name: "parser-repair".to_owned(),
        status: SessionStatusView::Analyzing,
        active_task_id: Some(task_id),
        parent_session_id: None,
        archived: false,
        created_at: now,
        updated_at: now,
    };
    let task = TaskSummary {
        id: task_id,
        session_id: Some(session_id),
        operation_id: uuid("33333333-3333-4333-8333-333333333333"),
        kind: TaskKind::AgentTurn,
        status: TaskStatus::Running,
        title: "Agent turn".to_owned(),
        created_at: now,
        started_at: Some(now),
        finished_at: None,
        progress_ratio: Some(0.64),
        is_cancellable: true,
        supports_steer: true,
    };
    let timeline = vec![
        TimelineItem::UserMessage(UserMessageItem {
            header: header(uuid("44444444-4444-4444-8444-444444444444"), ItemStatus::Completed),
            text: "Find the smallest verified repair and explain the evidence.".to_owned(),
        }),
        TimelineItem::ToolCall(ToolCallItem {
            header: header(uuid("55555555-5555-4555-8555-555555555555"), ItemStatus::Completed),
            tool_call_id: "call-1".to_owned(),
            name: "run_candidate".to_owned(),
            arguments_summary: "{\"action_ids\":[5,6]}".to_owned(),
            result_summary: Some("StablePass after 3/3 attempts".to_owned()),
            selection_reason: Some("Verify the current minimal candidate".to_owned()),
        }),
        TimelineItem::Trial(TrialItem {
            header: header(uuid("66666666-6666-4666-8666-666666666666"), ItemStatus::Completed),
            trial_id: uuid("66666666-6666-4666-8666-666666666666"),
            action_ids: vec![5, 6],
            classification: TrialClassification::StablePass,
            repetition_current: None,
            repetition_total: 3,
            summary: "Trial 018 · StablePass 3/3 · 2.41s".to_owned(),
        }),
        TimelineItem::AgentMessage(AgentMessageItem {
            header: header(uuid("77777777-7777-4777-8777-777777777777"), ItemStatus::Running),
            text: "Actions 5 and 6 form the dependency-constrained 1-minimal sufficient repair trace. Removing either action failed all repeated Oracle attempts.".to_owned(),
            public_reasoning_summary: None,
        }),
    ];
    let view = SessionView {
        summary: summary.clone(),
        task: Some(task),
        timeline,
        actions: vec![
            ActionView {
                id: 5,
                original_order: 5,
                kind: "file_patch".to_owned(),
                cwd: ".".to_owned(),
                summary: "edit parser configuration".to_owned(),
                replayable: true,
                can_rerun: true,
                note: None,
            },
            ActionView {
                id: 6,
                original_order: 6,
                kind: "shell".to_owned(),
                cwd: ".".to_owned(),
                summary: "restore executable permission".to_owned(),
                replayable: true,
                can_rerun: true,
                note: None,
            },
        ],
        trials: vec![TrialView {
            id: uuid("66666666-6666-4666-8666-666666666666"),
            action_ids: vec![5, 6],
            classification: TrialClassification::StablePass,
            attempts: (1..=3)
                .map(|index| TrialAttemptView {
                    index,
                    passed: Some(true),
                    exit_code: Some(0),
                    duration_ms: 803,
                    summary: format!("Oracle attempt {index}/3 passed"),
                })
                .collect(),
            trial_summary: "StablePass · 3/3 attempts".to_owned(),
            can_rerun: true,
        }],
        diagnosis: Some(DiagnosisView {
            statement: "Actions 5 and 6 are sufficient under the recorded baseline and Oracle."
                .to_owned(),
            minimal_action_ids: vec![5, 6],
            evidence: Vec::new(),
            limitations: vec!["Scoped to this baseline and environment".to_owned()],
            confidence: "high".to_owned(),
            diagnosis_summary: "Verified 1-minimal repair trace".to_owned(),
        }),
        usage: UsageSummary {
            input_tokens: 1_240,
            output_tokens: 318,
            total_tokens: 1_558,
            total_cost_usd: 0.0032,
            token_limit: 10_000,
            cost_limit_usd: 1.0,
            budget_ratio: 0.156,
            exact: true,
        },
        approvals: Vec::new(),
        dependency_graph: DependencyGraphView {
            nodes: vec![
                DependencyNodeView {
                    action_id: 5,
                    label: "edit parser configuration".to_owned(),
                    in_minimal_set: true,
                },
                DependencyNodeView {
                    action_id: 6,
                    label: "restore executable permission".to_owned(),
                    in_minimal_set: true,
                },
                DependencyNodeView {
                    action_id: 9,
                    label: "run acceptance test".to_owned(),
                    in_minimal_set: false,
                },
            ],
            edges: vec![
                DependencyEdgeView {
                    from_action_id: 5,
                    to_action_id: 9,
                    reason: "writes config read by test".to_owned(),
                },
                DependencyEdgeView {
                    from_action_id: 6,
                    to_action_id: 9,
                    reason: "permission required by test".to_owned(),
                },
            ],
        },
        diff: DiffView {
            files: vec![DiffFileView {
                path: "config/parser.toml".to_owned(),
                change_kind: "modified".to_owned(),
                unified_diff: Some("-mode = \"legacy\"\n+mode = \"strict\"".to_owned()),
                artifact_id: None,
            }],
            truncated: false,
        },
    };
    let mut model = Model::new(initialize(), ConnectionMode::InProcess);
    model.sessions = vec![
        summary.clone(),
        SessionSummary {
            id: uuid("88888888-8888-4888-8888-888888888888"),
            project_name: "cli-timeout".to_owned(),
            status: SessionStatusView::ReadyForAnalysis,
            active_task_id: None,
            parent_session_id: None,
            archived: false,
            created_at: now,
            updated_at: now,
        },
    ];
    model.selected_session_id = Some(session_id);
    model.session = Some(view);
    model.status = "Oracle attempt 2/3".to_owned();
    model
}

fn initialize() -> InitializeResponse {
    InitializeResponse {
        protocol_version: PROTOCOL_VERSION.to_owned(),
        server_version: "0.1.0".to_owned(),
        capabilities: ServerCapabilities {
            supports_streaming: true,
            supports_approvals: true,
            supports_diff: true,
            supports_graph: true,
            supports_artifacts: true,
            supports_event_catch_up: true,
            supports_multiple_clients: true,
            max_page_limit: 500,
            max_artifact_read_bytes: 1_048_576,
        },
        config_summary: PublicConfigSummary {
            provider: "openai-compatible".to_owned(),
            endpoint: "https://example.invalid/v1".to_owned(),
            model: "glm-5".to_owned(),
            api_style: "chat-completions".to_owned(),
            context_length: 32_768,
            reasoning_mode: "medium".to_owned(),
            replay_repetitions: 3,
            oracle_timeout_secs: 120,
            has_api_key: true,
            approval_policy: ApprovalPolicy::AskForOpaque,
        },
        client_id: uuid("99999999-9999-4999-8999-999999999999"),
    }
}

fn header(id: Uuid, status: ItemStatus) -> TimelineItemHeader {
    TimelineItemHeader {
        id,
        status,
        started_at: timestamp(),
        completed_at: (status == ItemStatus::Completed).then(timestamp),
        parent_id: None,
        artifacts: Vec::new(),
        entities: Vec::new(),
    }
}

fn timestamp() -> DateTime<Utc> {
    "2026-08-26T12:00:00Z".parse().unwrap()
}

fn uuid(value: &str) -> Uuid {
    Uuid::parse_str(value).unwrap()
}

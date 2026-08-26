use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::{
    agent::{diagnosis::Diagnosis, loop_runner::AgentRunResult},
    demo::DemoOutput,
    domain::{action::Action, session::SessionRecord, trial::Trial},
    minimize::engine::MinimizationReport,
};

#[derive(Clone, Debug)]
pub struct AppServiceOptions {
    pub state_dir: Option<PathBuf>,
    pub config_path: Option<PathBuf>,
    pub initialize_event_store: bool,
}

impl Default for AppServiceOptions {
    fn default() -> Self {
        Self {
            state_dir: None,
            config_path: None,
            initialize_event_store: true,
        }
    }
}

#[derive(Clone, Debug)]
pub enum AppCommand {
    InitializeSession {
        project: PathBuf,
        oracle: String,
    },
    RunControlledShell {
        session_id: Uuid,
    },
    AnalyzeSession {
        session_id: Uuid,
        no_llm: bool,
        prompt: Option<String>,
    },
    GetSession {
        session_id: Uuid,
    },
    ListSessions,
    ExportSession {
        session_id: Uuid,
        output: PathBuf,
    },
    ImportSession {
        input: PathBuf,
    },
    GetConfig,
    SetConfig {
        key: String,
        value: String,
    },
    RunDemo {
        no_llm: bool,
    },
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppResponse {
    SessionInitialized { session: SessionRecord },
    ControlledShellCompleted { session_id: Uuid },
    SessionAnalyzed { result: Box<AnalysisResult> },
    Session { detail: Box<SessionDetail> },
    Sessions { sessions: Vec<SessionRecord> },
    SessionExported { session_id: Uuid, output: PathBuf },
    SessionImported { session_id: Uuid },
    Config { toml: String },
    ConfigSaved { key: String, path: PathBuf },
    Demo { report: Box<DemoOutput> },
}

#[derive(Clone, Debug, Serialize)]
pub struct AnalysisResult {
    pub session_id: Uuid,
    pub report: MinimizationReport,
    pub diagnosis: Diagnosis,
    pub agent: Option<AgentRunResult>,
    pub llm_mode: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct SessionDetail {
    pub session: SessionRecord,
    pub actions: Vec<Action>,
    pub trials: Vec<Trial>,
    pub messages: Vec<Value>,
    pub tool_calls: Vec<Value>,
    pub api_usage: Vec<Value>,
    pub progress_events: Vec<Value>,
    pub diagnoses: Vec<Value>,
}

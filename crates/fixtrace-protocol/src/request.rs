use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;
use uuid::Uuid;

use crate::{
    ActionListResponse, ActionView, AppErrorView, ApprovalChoice, ArtifactChunk, ArtifactSummary,
    DependencyGraphView, DiagnosisView, EmptyRequest, EventBatch, InitializeRequest,
    InitializeResponse, PageInfo, PageRequest, PublicConfigSummary, SessionIdRequest,
    SessionListResponse, SessionPageRequest, SessionSnapshot, SessionSummary, SubscribeRequest,
    SubscriptionStarted, TaskIdRequest, TaskKind, TaskSummary, TrialListResponse, TrialView,
    UnsubscribeRequest, UsageSummary,
};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct RequestEnvelope {
    pub id: Uuid,
    pub operation_id: Uuid,
    pub method: String,
    pub params: Value,
}

impl RequestEnvelope {
    pub fn decode(&self) -> Result<AppRequest, AppErrorView> {
        serde_json::from_value(serde_json::json!({
            "method": self.method,
            "params": self.params,
        }))
        .map_err(|error| {
            AppErrorView::new(
                crate::ErrorCode::InvalidRequest,
                format!("invalid parameters for `{}`: {error}", self.method),
            )
        })
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct ResponseEnvelope {
    pub id: Uuid,
    pub result: Option<Value>,
    pub error: Option<AppErrorView>,
}

impl ResponseEnvelope {
    pub fn success<T: Serialize>(id: Uuid, result: &T) -> Result<Self, serde_json::Error> {
        Ok(Self {
            id,
            result: Some(serde_json::to_value(result)?),
            error: None,
        })
    }

    pub const fn error(id: Uuid, error: AppErrorView) -> Self {
        Self {
            id,
            result: None,
            error: Some(error),
        }
    }

    pub const fn is_valid(&self) -> bool {
        matches!(
            (&self.result, &self.error),
            (Some(_), None) | (None, Some(_))
        )
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum ClientFrame {
    Request(RequestEnvelope),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum ServerFrame {
    Response(ResponseEnvelope),
    Event(Box<crate::EventEnvelope>),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct SessionListRequest {
    pub page: PageRequest,
    pub include_archived: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct SessionCreateRequest {
    pub project: PathBuf,
    pub oracle: String,
    pub title: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct SessionForkRequest {
    pub session_id: Uuid,
    pub title: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct SessionDeleteRequest {
    pub session_id: Uuid,
    pub confirmation: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct SessionSnapshotRequest {
    pub session_id: Uuid,
    pub timeline_page: PageRequest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum TaskInput {
    AgentTurn { prompt: String },
    RecordTrace { line: String },
    VerifyBaseline,
    ReplayFullTrace,
    AnalyzeMinimalTrace { no_llm: bool },
    RepeatTrial { trial_id: Uuid },
    GenerateDiagnosis { prompt: Option<String> },
    ExportSession { output: PathBuf },
    Demo { no_llm: bool },
}

impl TaskInput {
    pub const fn kind(&self) -> TaskKind {
        match self {
            Self::AgentTurn { .. } => TaskKind::AgentTurn,
            Self::RecordTrace { .. } => TaskKind::RecordTrace,
            Self::VerifyBaseline => TaskKind::VerifyBaseline,
            Self::ReplayFullTrace => TaskKind::ReplayFullTrace,
            Self::AnalyzeMinimalTrace { .. } => TaskKind::AnalyzeMinimalTrace,
            Self::RepeatTrial { .. } => TaskKind::RepeatTrial,
            Self::GenerateDiagnosis { .. } => TaskKind::GenerateDiagnosis,
            Self::ExportSession { .. } => TaskKind::ExportSession,
            Self::Demo { .. } => TaskKind::Demo,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct TaskStartRequest {
    pub session_id: Option<Uuid>,
    pub input: TaskInput,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct TaskSteerRequest {
    pub task_id: Uuid,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct MessageSendRequest {
    pub session_id: Uuid,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct ActionGetRequest {
    pub session_id: Uuid,
    pub action_id: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct TrialGetRequest {
    pub session_id: Uuid,
    pub trial_id: Uuid,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct TrialRunRequest {
    pub session_id: Uuid,
    pub action_ids: Vec<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct TrialRepeatRequest {
    pub session_id: Uuid,
    pub trial_id: Uuid,
    pub repetitions: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct ArtifactReadRequest {
    pub artifact_id: Uuid,
    pub offset: u64,
    pub limit: u32,
}

impl ArtifactReadRequest {
    pub fn validated_limit(&self) -> u32 {
        self.limit.clamp(1, crate::MAX_ARTIFACT_READ_BYTES)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct ApprovalRespondRequest {
    pub approval_id: Uuid,
    pub choice: ApprovalChoice,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ConfigValue {
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct ConfigEntryUpdate {
    pub key: String,
    pub value: ConfigValue,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct ConfigUpdateRequest {
    pub updates: Vec<ConfigEntryUpdate>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct ConnectionTestRequest {
    pub provider: String,
    pub endpoint: String,
    pub model: String,
    pub credential_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct ConnectionTestResponse {
    pub ok: bool,
    pub model: Option<String>,
    pub latency_ms: u64,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct UsageGetRequest {
    pub session_id: Option<Uuid>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct SessionExportRequest {
    pub session_id: Uuid,
    pub output: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct SessionImportRequest {
    pub input: PathBuf,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
#[serde(tag = "method", content = "params")]
pub enum AppRequest {
    #[serde(rename = "initialize")]
    Initialize(InitializeRequest),
    #[serde(rename = "session/list")]
    SessionList(SessionListRequest),
    #[serde(rename = "session/create")]
    SessionCreate(SessionCreateRequest),
    #[serde(rename = "session/open")]
    SessionOpen(SessionIdRequest),
    #[serde(rename = "session/fork")]
    SessionFork(SessionForkRequest),
    #[serde(rename = "session/archive")]
    SessionArchive(SessionIdRequest),
    #[serde(rename = "session/delete")]
    SessionDelete(SessionDeleteRequest),
    #[serde(rename = "session/get_snapshot")]
    SessionGetSnapshot(SessionSnapshotRequest),
    #[serde(rename = "task/start")]
    TaskStart(TaskStartRequest),
    #[serde(rename = "task/steer")]
    TaskSteer(TaskSteerRequest),
    #[serde(rename = "task/cancel")]
    TaskCancel(TaskIdRequest),
    #[serde(rename = "task/get")]
    TaskGet(TaskIdRequest),
    #[serde(rename = "message/send")]
    MessageSend(MessageSendRequest),
    #[serde(rename = "action/list")]
    ActionList(SessionPageRequest),
    #[serde(rename = "action/get")]
    ActionGet(ActionGetRequest),
    #[serde(rename = "trial/list")]
    TrialList(SessionPageRequest),
    #[serde(rename = "trial/get")]
    TrialGet(TrialGetRequest),
    #[serde(rename = "trial/run")]
    TrialRun(TrialRunRequest),
    #[serde(rename = "trial/repeat")]
    TrialRepeat(TrialRepeatRequest),
    #[serde(rename = "dependency/get_graph")]
    DependencyGetGraph(SessionIdRequest),
    #[serde(rename = "diagnosis/get")]
    DiagnosisGet(SessionIdRequest),
    #[serde(rename = "artifact/list")]
    ArtifactList(SessionPageRequest),
    #[serde(rename = "artifact/read")]
    ArtifactRead(ArtifactReadRequest),
    #[serde(rename = "approval/respond")]
    ApprovalRespond(ApprovalRespondRequest),
    #[serde(rename = "config/get")]
    ConfigGet(EmptyRequest),
    #[serde(rename = "config/update")]
    ConfigUpdate(ConfigUpdateRequest),
    #[serde(rename = "config/test_connection")]
    ConfigTestConnection(ConnectionTestRequest),
    #[serde(rename = "usage/get")]
    UsageGet(UsageGetRequest),
    #[serde(rename = "session/export")]
    SessionExport(SessionExportRequest),
    #[serde(rename = "session/import")]
    SessionImport(SessionImportRequest),
    #[serde(rename = "event/subscribe")]
    EventSubscribe(SubscribeRequest),
    #[serde(rename = "event/unsubscribe")]
    EventUnsubscribe(UnsubscribeRequest),
}

impl AppRequest {
    pub const fn method(&self) -> &'static str {
        match self {
            Self::Initialize(_) => "initialize",
            Self::SessionList(_) => "session/list",
            Self::SessionCreate(_) => "session/create",
            Self::SessionOpen(_) => "session/open",
            Self::SessionFork(_) => "session/fork",
            Self::SessionArchive(_) => "session/archive",
            Self::SessionDelete(_) => "session/delete",
            Self::SessionGetSnapshot(_) => "session/get_snapshot",
            Self::TaskStart(_) => "task/start",
            Self::TaskSteer(_) => "task/steer",
            Self::TaskCancel(_) => "task/cancel",
            Self::TaskGet(_) => "task/get",
            Self::MessageSend(_) => "message/send",
            Self::ActionList(_) => "action/list",
            Self::ActionGet(_) => "action/get",
            Self::TrialList(_) => "trial/list",
            Self::TrialGet(_) => "trial/get",
            Self::TrialRun(_) => "trial/run",
            Self::TrialRepeat(_) => "trial/repeat",
            Self::DependencyGetGraph(_) => "dependency/get_graph",
            Self::DiagnosisGet(_) => "diagnosis/get",
            Self::ArtifactList(_) => "artifact/list",
            Self::ArtifactRead(_) => "artifact/read",
            Self::ApprovalRespond(_) => "approval/respond",
            Self::ConfigGet(_) => "config/get",
            Self::ConfigUpdate(_) => "config/update",
            Self::ConfigTestConnection(_) => "config/test_connection",
            Self::UsageGet(_) => "usage/get",
            Self::SessionExport(_) => "session/export",
            Self::SessionImport(_) => "session/import",
            Self::EventSubscribe(_) => "event/subscribe",
            Self::EventUnsubscribe(_) => "event/unsubscribe",
        }
    }

    pub fn into_envelope(
        self,
        id: Uuid,
        operation_id: Uuid,
    ) -> Result<RequestEnvelope, serde_json::Error> {
        let encoded = serde_json::to_value(self)?;
        let method = encoded
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let params = encoded.get("params").cloned().unwrap_or(Value::Null);
        Ok(RequestEnvelope {
            id,
            operation_id,
            method,
            params,
        })
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum AppResponsePayload {
    Initialized(InitializeResponse),
    SessionList(SessionListResponse),
    Session(SessionSummary),
    SessionSnapshot(Box<SessionSnapshot>),
    Task(TaskSummary),
    ActionList(ActionListResponse),
    Action(ActionView),
    TrialList(TrialListResponse),
    Trial(TrialView),
    DependencyGraph(DependencyGraphView),
    Diagnosis(Option<DiagnosisView>),
    ArtifactList {
        artifacts: Vec<ArtifactSummary>,
        page: PageInfo,
    },
    Artifact(ArtifactChunk),
    Config(PublicConfigSummary),
    ConnectionTest(ConnectionTestResponse),
    Usage(UsageSummary),
    Subscription(SubscriptionStarted),
    EventBatch(EventBatch),
    Exported {
        session_id: Uuid,
        output: PathBuf,
    },
    Imported {
        session_id: Uuid,
    },
    Accepted {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use uuid::Uuid;

    use super::{AppRequest, RequestEnvelope, ResponseEnvelope, SessionListRequest};
    use crate::{AppErrorView, ErrorCode, PageRequest};

    #[test]
    fn method_names_use_the_wire_contract() {
        let request = AppRequest::SessionList(SessionListRequest {
            page: PageRequest {
                cursor: None,
                limit: Some(10),
            },
            include_archived: false,
        });
        let encoded = serde_json::to_value(&request).expect("request should serialize");
        assert_eq!(encoded["method"], "session/list");
        assert_eq!(request.method(), "session/list");
        let envelope = request
            .clone()
            .into_envelope(Uuid::new_v4(), Uuid::new_v4())
            .expect("typed request should encode");
        assert_eq!(envelope.decode().unwrap(), request);
    }

    #[test]
    fn envelopes_have_exactly_one_response_branch() {
        let id = Uuid::new_v4();
        assert!(
            ResponseEnvelope::success(id, &json!({"ok": true}))
                .unwrap()
                .is_valid()
        );
        assert!(
            ResponseEnvelope::error(
                id,
                AppErrorView::new(ErrorCode::InvalidRequest, "bad request")
            )
            .is_valid()
        );
        assert!(
            !ResponseEnvelope {
                id,
                result: None,
                error: None
            }
            .is_valid()
        );
    }

    #[test]
    fn request_envelope_does_not_put_credentials_in_the_shape() {
        let envelope = RequestEnvelope {
            id: Uuid::new_v4(),
            operation_id: Uuid::new_v4(),
            method: "config/test_connection".to_owned(),
            params: json!({"credential_id": "keychain:fixtrace"}),
        };
        let encoded = serde_json::to_string(&envelope).unwrap();
        assert!(!encoded.contains("api_key"));
        assert!(!encoded.contains("authorization"));
    }
}

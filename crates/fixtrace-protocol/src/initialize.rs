use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::PublicConfigSummary;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct ClientInfo {
    pub name: String,
    pub title: String,
    pub version: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct ClientCapabilities {
    pub supports_streaming: bool,
    pub supports_approvals: bool,
    pub supports_diff: bool,
    pub supports_graph: bool,
    pub supports_artifacts: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct ServerCapabilities {
    pub supports_streaming: bool,
    pub supports_approvals: bool,
    pub supports_diff: bool,
    pub supports_graph: bool,
    pub supports_artifacts: bool,
    pub supports_event_catch_up: bool,
    pub supports_multiple_clients: bool,
    pub max_page_limit: u32,
    pub max_artifact_read_bytes: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct InitializeRequest {
    pub protocol_version: String,
    pub client: ClientInfo,
    pub capabilities: ClientCapabilities,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct InitializeResponse {
    pub protocol_version: String,
    pub server_version: String,
    pub capabilities: ServerCapabilities,
    pub config_summary: PublicConfigSummary,
    pub client_id: Uuid,
}

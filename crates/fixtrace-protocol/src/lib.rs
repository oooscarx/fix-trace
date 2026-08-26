mod approval;
mod common;
mod error;
mod events;
mod initialize;
mod request;
mod task;
mod timeline;
mod typescript;
mod view;

pub use approval::*;
pub use common::*;
pub use error::*;
pub use events::*;
pub use initialize::*;
pub use request::*;
pub use task::*;
pub use timeline::*;
pub use typescript::export_typescript;
pub use view::*;

pub const PROTOCOL_VERSION: &str = "fixtrace/1";
pub const EVENT_SCHEMA_VERSION: u16 = 1;
pub const DEFAULT_PAGE_LIMIT: u32 = 100;
pub const MAX_PAGE_LIMIT: u32 = 500;
pub const MAX_ARTIFACT_READ_BYTES: u32 = 1024 * 1024;
pub const MAX_SAFE_WIRE_INTEGER: u64 = 9_007_199_254_740_991;

pub fn is_compatible_protocol_version(candidate: &str) -> bool {
    candidate == PROTOCOL_VERSION
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};
    use uuid::Uuid;

    use crate::{
        ActionView, AppEvent, EVENT_SCHEMA_VERSION, EventEnvelope, Notice, NoticeLevel,
        SessionSummary, is_compatible_protocol_version,
    };

    #[test]
    fn protocol_major_is_explicitly_negotiated() {
        assert!(is_compatible_protocol_version("fixtrace/1"));
        assert!(!is_compatible_protocol_version("fixtrace/2"));
        assert!(!is_compatible_protocol_version("1"));
    }

    #[test]
    fn event_wire_shape_is_snapshotted() {
        let event = EventEnvelope {
            schema_version: EVENT_SCHEMA_VERSION,
            stream_id: uuid("11111111-1111-4111-8111-111111111111"),
            sequence: 7,
            event_id: uuid("22222222-2222-4222-8222-222222222222"),
            timestamp: "2026-08-26T12:00:00Z"
                .parse::<DateTime<Utc>>()
                .expect("timestamp should parse"),
            session_id: Some(uuid("33333333-3333-4333-8333-333333333333")),
            task_id: None,
            payload: AppEvent::Notice(Notice {
                code: "ready".to_owned(),
                level: NoticeLevel::Success,
                title: "Ready".to_owned(),
                message: "Event stream initialized".to_owned(),
            }),
        };
        insta::assert_json_snapshot!("event_envelope_notice", event);
    }

    #[test]
    fn unknown_future_events_are_ignored_without_losing_the_envelope() {
        let event: EventEnvelope = serde_json::from_value(serde_json::json!({
            "schema_version": EVENT_SCHEMA_VERSION,
            "stream_id": "11111111-1111-4111-8111-111111111111",
            "sequence": 8,
            "event_id": "22222222-2222-4222-8222-222222222222",
            "timestamp": "2026-08-26T12:00:00Z",
            "session_id": null,
            "task_id": null,
            "payload": { "type": "future_event", "data": { "new": true } }
        }))
        .expect("unknown events should decode to the forward-compatible sentinel");
        assert_eq!(event.payload, AppEvent::Unknown);
        event
            .validate()
            .expect("the containing envelope remains valid");
    }

    #[test]
    fn old_optional_view_fields_receive_safe_defaults() {
        let session: SessionSummary = serde_json::from_value(serde_json::json!({
            "id": "33333333-3333-4333-8333-333333333333",
            "project_name": "legacy",
            "status": "recording",
            "active_task_id": null,
            "parent_session_id": null,
            "archived": false,
            "created_at": "2026-08-26T12:00:00Z",
            "updated_at": "2026-08-26T12:00:00Z"
        }))
        .expect("legacy session summary should remain readable");
        assert!(session.project_path.is_empty());

        let action: ActionView = serde_json::from_value(serde_json::json!({
            "id": 1,
            "original_order": 1,
            "kind": "shell_command",
            "cwd": ".",
            "summary": "legacy action",
            "replayable": true,
            "can_rerun": true,
            "note": null
        }))
        .expect("legacy action view should remain readable");
        assert!(action.reads.is_empty());
        assert!(action.writes.is_empty());
        assert!(!action.resource_access_opaque);
    }

    #[test]
    fn unsupported_event_schema_is_rejected_explicitly() {
        let event = EventEnvelope {
            schema_version: 0,
            stream_id: uuid("11111111-1111-4111-8111-111111111111"),
            sequence: 1,
            event_id: uuid("22222222-2222-4222-8222-222222222222"),
            timestamp: "2026-08-26T12:00:00Z".parse().unwrap(),
            session_id: None,
            task_id: None,
            payload: AppEvent::Unknown,
        };
        let error = event.validate().expect_err("old schema requires recovery");
        assert!(error.message.contains("unsupported event schema version 0"));
    }

    fn uuid(value: &str) -> Uuid {
        Uuid::parse_str(value).expect("UUID fixture should parse")
    }
}

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
        AppEvent, EVENT_SCHEMA_VERSION, EventEnvelope, Notice, NoticeLevel,
        is_compatible_protocol_version,
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

    fn uuid(value: &str) -> Uuid {
        Uuid::parse_str(value).expect("UUID fixture should parse")
    }
}

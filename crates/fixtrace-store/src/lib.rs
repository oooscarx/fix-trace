mod migrations;
mod store;

pub use store::{EventStore, StoreError};

pub const CURRENT_SCHEMA_VERSION: i64 = 2;

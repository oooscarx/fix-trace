mod connection;
mod lock;
mod stdio;
mod token;
mod websocket;

pub use connection::{ConnectionAction, ConnectionReply, ConnectionState};
pub use lock::{LockError, WriterLock};
pub use stdio::{StdioError, serve_stdio};
pub use token::{TokenError, load_or_create_token};
pub use websocket::{WebSocketServerError, parse_ws_bind, serve_websocket};

pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

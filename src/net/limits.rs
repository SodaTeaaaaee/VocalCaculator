//! Central resource limits for the LAN stack.
//!
//! Wire constants (`PROTOCOL_MAGIC`, `LAN_FIXED_PORT`, `NetworkMessage`
//! layout) stay in [`crate::net::protocol`]. This module is the single
//! place for queue depths, peer caps, and rate bounds so Router, runtime,
//! and UI do not drift.

use std::time::Duration;

/// Hard cap between the network runtime and the UI event loop.
pub const UI_EVENT_CAPACITY: usize = 256;
/// Runtime command channel (session register/unregister, connect, scan).
pub const RUNTIME_COMMAND_CAPACITY: usize = 256;
/// Outbound `(NodeId, NetworkMessage)` fan-out queue.
pub const OUTGOING_MESSAGE_CAPACITY: usize = 256;
/// Per-session inbound command queue.
pub const SESSION_COMMAND_CAPACITY: usize = 256;
/// Merged command queue feeding the runtime command loop.
pub const MERGED_COMMAND_CAPACITY: usize = 512;
/// Scan trigger channel. Capacity 1 coalesces repeated F5 presses.
pub const SCAN_COMMAND_CAPACITY: usize = 1;

/// Authenticated inbound TCP sessions accepted at once.
pub const MAX_INBOUND_SESSIONS: usize = 16;
/// Concurrent outbound connect attempts.
pub const MAX_IN_FLIGHT_CONNECTS: usize = 32;
/// Discovery endpoint retry budget.
pub const MAX_DISCOVERY_ENDPOINT_ATTEMPTS: usize = 256;
/// Seconds before a failed discovery endpoint may be retried.
pub const DISCOVERY_ENDPOINT_RETRY_SECS: u64 = 30;
/// Product-level peer map (Router / UI). Transport PeerTable is also 64.
pub const MAX_TRACKED_REMOTE_PEERS: usize = 64;
/// PeerTable / known-peer cap used by discovery.
pub const MAX_KNOWN_PEERS: usize = 64;

/// Remote actions accepted per peer per second (burst window).
pub const MAX_REMOTE_ACTIONS_PER_SECOND: u32 = 120;
/// Reject actions older than this.
pub const MAX_ACTION_AGE: Duration = Duration::from_secs(5 * 60);
/// Reject actions stamped too far in the future.
pub const MAX_ACTION_FUTURE_SKEW: Duration = Duration::from_secs(30);

/// StateSnapshot display field.
pub const MAX_DISPLAY_BYTES: usize = 64;
/// StateSnapshot history field.
pub const MAX_HISTORY_BYTES: usize = 256;
/// StateSnapshot memory indicator field.
pub const MAX_MEMORY_INDICATOR_BYTES: usize = 8;

/// Length-delimited TCP frame cap (bytes).
pub const MAX_FRAME_LENGTH: usize = 4 * 1024;

/// Network OS thread startup wait.
pub const NETWORK_THREAD_START_TIMEOUT: Duration = Duration::from_secs(5);
/// Network OS thread shutdown wait.
pub const NETWORK_THREAD_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

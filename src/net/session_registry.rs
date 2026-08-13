//! Active TCP session registry.
//!
//! This is the only owner of `HashMap<NodeId, ActiveSession>`. Session
//! generation IDs are assigned here so compare-and-remove teardown cannot
//! drop a replacement connection.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LockResult, Mutex, MutexGuard};

use crate::net::protocol::{ConnectionDirection, NodeId, SessionId};
use crate::net::session::ActiveSession;

/// Outcome of [`SessionRegistry::insert`].
pub(crate) struct SessionInsertResult {
    pub accepted: bool,
    pub session_id: SessionId,
    pub replaced: Option<ActiveSession>,
}

impl std::fmt::Debug for SessionInsertResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionInsertResult")
            .field("accepted", &self.accepted)
            .field("session_id", &self.session_id)
            .field(
                "replaced.session_id",
                &self.replaced.as_ref().map(|session| session.session_id),
            )
            .finish()
    }
}

/// Shared map of the current session generation for each peer.
#[derive(Clone)]
pub(crate) struct SessionRegistry {
    inner: Arc<Mutex<HashMap<NodeId, ActiveSession>>>,
}

impl SessionRegistry {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Direct map lock for `NetworkManager` helpers and existing tests.
    pub(crate) fn lock(&self) -> LockResult<MutexGuard<'_, HashMap<NodeId, ActiveSession>>> {
        self.inner.lock()
    }

    fn lock_map(&self) -> MutexGuard<'_, HashMap<NodeId, ActiveSession>> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Insert `session` as the current generation for `node_id`.
    ///
    /// A nil [`SessionId`] is replaced with a fresh UUID. NodeId-ordering
    /// dedup ([`should_replace_session`]) decides whether an existing
    /// generation is kept.
    pub(crate) fn insert(
        &self,
        local_id: NodeId,
        node_id: NodeId,
        mut session: ActiveSession,
    ) -> SessionInsertResult {
        if session.session_id == SessionId::nil() {
            session.session_id = SessionId::new_v4();
        }
        let session_id = session.session_id;
        let new_direction = session.direction;
        let mut sessions = self.lock_map();
        let keep_new = sessions
            .get(&node_id)
            .map(|existing| {
                should_replace_session(local_id, node_id, existing.direction, new_direction)
            })
            .unwrap_or(true);
        if keep_new {
            let replaced = sessions.insert(node_id, session);
            SessionInsertResult {
                accepted: true,
                session_id,
                replaced,
            }
        } else {
            SessionInsertResult {
                accepted: false,
                session_id,
                replaced: None,
            }
        }
    }

    pub(crate) fn remove_if_current(&self, node_id: NodeId, session_id: SessionId) -> bool {
        let mut sessions = self.lock_map();
        remove_session_if_current(&mut sessions, node_id, session_id)
    }

    pub(crate) fn get(&self, node_id: NodeId) -> Option<ActiveSession> {
        self.lock_map().get(&node_id).cloned()
    }

    pub(crate) fn ids(&self) -> HashSet<NodeId> {
        self.lock_map().keys().copied().collect()
    }

    /// Cancel one session generation without removing a newer replacement.
    pub(crate) fn cancel_generation(&self, node_id: NodeId, session_id: SessionId) -> bool {
        if let Some(session) = self.lock_map().get(&node_id)
            && session.session_id == session_id
        {
            let _ = session.cancel_tx.send(true);
            return true;
        }
        false
    }

    pub(crate) fn contains(&self, node_id: NodeId) -> bool {
        self.lock_map().contains_key(&node_id)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.lock_map().is_empty()
    }

    pub(crate) fn clear(&self) {
        self.lock_map().clear();
    }

    /// Put `session` back after a failed registration decision.
    pub(crate) fn restore(&self, node_id: NodeId, session: ActiveSession) {
        self.lock_map().insert(node_id, session);
    }

    pub(crate) fn snapshot(&self) -> Vec<ActiveSession> {
        self.lock_map().values().cloned().collect()
    }
}

/// Prefer the connection direction owned by the lower [`NodeId`].
///
/// When `local_id < remote_id` this node keeps outbound; otherwise it keeps
/// inbound. A new generation replaces the current one only when it is the
/// preferred direction and the existing one is not.
pub(crate) fn should_replace_session(
    local_id: NodeId,
    remote_id: NodeId,
    existing_direction: ConnectionDirection,
    new_direction: ConnectionDirection,
) -> bool {
    let preferred = if local_id < remote_id {
        ConnectionDirection::Outbound
    } else {
        ConnectionDirection::Inbound
    };
    new_direction == preferred && existing_direction != preferred
}

/// Remove `node_id` only when `session_id` is still the current generation.
pub(crate) fn remove_session_if_current(
    sessions: &mut HashMap<NodeId, ActiveSession>,
    node_id: NodeId,
    session_id: SessionId,
) -> bool {
    if sessions
        .get(&node_id)
        .is_some_and(|session| session.session_id == session_id)
    {
        sessions.remove(&node_id);
        true
    } else {
        false
    }
}

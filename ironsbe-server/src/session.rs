//! Session management.

use parking_lot::RwLock;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};

/// Session information.
#[derive(Debug, Clone)]
pub struct Session {
    /// Session ID.
    pub id: u64,
    /// Peer address.
    pub peer_addr: SocketAddr,
    /// Creation timestamp (nanos since epoch).
    pub created_at: u64,
    /// Last activity timestamp.
    pub last_activity: u64,
}

/// Manages active sessions.
pub struct SessionManager {
    sessions: RwLock<HashMap<u64, Session>>,
    next_id: AtomicU64,
}

impl SessionManager {
    /// Creates a new session manager.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        }
    }

    /// Creates a new session and returns its ID.
    pub fn create_session(&self, peer_addr: SocketAddr) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;

        let session = Session {
            id,
            peer_addr,
            created_at: now,
            last_activity: now,
        };

        self.sessions.write().insert(id, session);
        id
    }

    /// Closes a session.
    pub fn close_session(&self, session_id: u64) -> Option<Session> {
        self.sessions.write().remove(&session_id)
    }

    /// Gets a session by ID.
    #[must_use]
    pub fn get_session(&self, session_id: u64) -> Option<Session> {
        self.sessions.read().get(&session_id).cloned()
    }

    /// Updates the last activity timestamp for a session.
    pub fn touch_session(&self, session_id: u64) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;

        if let Some(session) = self.sessions.write().get_mut(&session_id) {
            session.last_activity = now;
        }
    }

    /// Returns the number of active sessions.
    #[must_use]
    pub fn count(&self) -> usize {
        self.sessions.read().len()
    }

    /// Returns all session IDs.
    #[must_use]
    pub fn session_ids(&self) -> Vec<u64> {
        self.sessions.read().keys().copied().collect()
    }

    /// Iterates over all sessions.
    pub fn for_each<F>(&self, mut f: F)
    where
        F: FnMut(&Session),
    {
        for session in self.sessions.read().values() {
            f(session);
        }
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_session() {
        let manager = SessionManager::new();
        let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();

        let id1 = manager.create_session(addr);
        let id2 = manager.create_session(addr);

        assert_ne!(id1, id2);
        assert_eq!(manager.count(), 2);
    }

    #[test]
    fn test_close_session() {
        let manager = SessionManager::new();
        let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();

        let id = manager.create_session(addr);
        assert_eq!(manager.count(), 1);

        let session = manager.close_session(id);
        assert!(session.is_some());
        assert_eq!(manager.count(), 0);
    }

    #[test]
    fn test_get_session() {
        let manager = SessionManager::new();
        let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();

        let id = manager.create_session(addr);
        let session = manager.get_session(id).unwrap();

        assert_eq!(session.id, id);
        assert_eq!(session.peer_addr, addr);
    }
}

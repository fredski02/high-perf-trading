//! Session management for authenticated connections
//!
//! Tracks which connections are authenticated and their associated account IDs.
//! Enforces that clients must authenticate before placing orders.

use common::AccountId;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Session manager tracks authenticated connections
pub struct SessionManager {
    /// Map of conn_id -> account_id for authenticated sessions
    sessions: Arc<RwLock<HashMap<u64, AccountId>>>,
}

impl SessionManager {
    /// Create a new session manager
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register an authenticated session
    pub async fn register(&self, conn_id: u64, account_id: AccountId) {
        let mut sessions = self.sessions.write().await;
        sessions.insert(conn_id, account_id);
    }

    /// Get the account ID for a connection (if authenticated)
    #[allow(dead_code)]
    pub async fn get_account_id(&self, conn_id: u64) -> Option<AccountId> {
        let sessions = self.sessions.read().await;
        sessions.get(&conn_id).copied()
    }

    /// Check if a connection is authenticated
    pub async fn is_authenticated(&self, conn_id: u64) -> bool {
        let sessions = self.sessions.read().await;
        sessions.contains_key(&conn_id)
    }

    /// Unregister a session (on disconnect)
    pub async fn unregister(&self, conn_id: u64) -> Option<AccountId> {
        let mut sessions = self.sessions.write().await;
        sessions.remove(&conn_id)
    }

    /// Get total number of active sessions
    #[allow(dead_code)]
    pub async fn session_count(&self) -> usize {
        let sessions = self.sessions.read().await;
        sessions.len()
    }

    /// Get all active sessions
    #[allow(dead_code)]
    pub async fn get_all_sessions(&self) -> HashMap<u64, AccountId> {
        let sessions = self.sessions.read().await;
        sessions.clone()
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

    #[tokio::test]
    async fn test_register_and_get() {
        let sm = SessionManager::new();

        // Not authenticated initially
        assert!(!sm.is_authenticated(1).await);
        assert_eq!(sm.get_account_id(1).await, None);

        // Register session
        sm.register(1, 100).await;

        // Now authenticated
        assert!(sm.is_authenticated(1).await);
        assert_eq!(sm.get_account_id(1).await, Some(100));
    }

    #[tokio::test]
    async fn test_unregister() {
        let sm = SessionManager::new();

        sm.register(1, 100).await;
        assert!(sm.is_authenticated(1).await);

        // Unregister returns the account_id
        let account_id = sm.unregister(1).await;
        assert_eq!(account_id, Some(100));

        // No longer authenticated
        assert!(!sm.is_authenticated(1).await);
        assert_eq!(sm.get_account_id(1).await, None);

        // Unregistering again returns None
        assert_eq!(sm.unregister(1).await, None);
    }

    #[tokio::test]
    async fn test_session_count() {
        let sm = SessionManager::new();

        assert_eq!(sm.session_count().await, 0);

        sm.register(1, 100).await;
        sm.register(2, 200).await;
        sm.register(3, 300).await;

        assert_eq!(sm.session_count().await, 3);

        sm.unregister(2).await;
        assert_eq!(sm.session_count().await, 2);
    }

    #[tokio::test]
    async fn test_get_all_sessions() {
        let sm = SessionManager::new();

        sm.register(1, 100).await;
        sm.register(2, 200).await;
        sm.register(3, 300).await;

        let all = sm.get_all_sessions().await;
        assert_eq!(all.len(), 3);
        assert_eq!(all.get(&1), Some(&100));
        assert_eq!(all.get(&2), Some(&200));
        assert_eq!(all.get(&3), Some(&300));
    }

    #[tokio::test]
    async fn test_multiple_sessions_same_account() {
        let sm = SessionManager::new();

        // Same account can have multiple sessions (multiple connections)
        sm.register(1, 100).await;
        sm.register(2, 100).await;
        sm.register(3, 100).await;

        assert_eq!(sm.get_account_id(1).await, Some(100));
        assert_eq!(sm.get_account_id(2).await, Some(100));
        assert_eq!(sm.get_account_id(3).await, Some(100));
        assert_eq!(sm.session_count().await, 3);
    }
}

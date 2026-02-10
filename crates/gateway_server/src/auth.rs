//! Authentication module for gateway server
//!
//! Handles API key verification and account assignment.
//! For production, this would integrate with a database or external auth service.

use anyhow::{anyhow, Result};
use common::AccountId;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Authentication service for verifying API keys and assigning accounts
pub struct AuthService {
    /// Map of API key -> Account ID
    /// In production, this would be a database lookup
    api_keys: Arc<RwLock<HashMap<String, AccountId>>>,
}

impl AuthService {
    /// Create a new authentication service
    pub fn new() -> Self {
        Self {
            api_keys: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register a new API key for an account (admin operation)
    pub async fn register_api_key(&self, api_key: String, account_id: AccountId) {
        let mut keys = self.api_keys.write().await;
        keys.insert(api_key, account_id);
    }

    /// Verify an API key and return the associated account ID
    pub async fn authenticate(&self, api_key: &str) -> Result<AccountId> {
        let keys = self.api_keys.read().await;
        
        keys.get(api_key)
            .copied()
            .ok_or_else(|| anyhow!("Invalid API key"))
    }

    /// Check if an API key exists
    #[allow(dead_code)]
    pub async fn has_api_key(&self, api_key: &str) -> bool {
        let keys = self.api_keys.read().await;
        keys.contains_key(api_key)
    }

    /// Revoke an API key (admin operation)
    #[allow(dead_code)]
    pub async fn revoke_api_key(&self, api_key: &str) -> bool {
        let mut keys = self.api_keys.write().await;
        keys.remove(api_key).is_some()
    }

    /// Get total number of registered API keys
    #[allow(dead_code)]
    pub async fn key_count(&self) -> usize {
        let keys = self.api_keys.read().await;
        keys.len()
    }
}

impl Default for AuthService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_register_and_authenticate() {
        let auth = AuthService::new();
        
        // Register an API key
        auth.register_api_key("test-key-123".to_string(), 42).await;
        
        // Authenticate with valid key
        let account_id = auth.authenticate("test-key-123").await.unwrap();
        assert_eq!(account_id, 42);
        
        // Authenticate with invalid key
        assert!(auth.authenticate("invalid-key").await.is_err());
    }

    #[tokio::test]
    async fn test_has_api_key() {
        let auth = AuthService::new();
        
        auth.register_api_key("key1".to_string(), 1).await;
        
        assert!(auth.has_api_key("key1").await);
        assert!(!auth.has_api_key("key2").await);
    }

    #[tokio::test]
    async fn test_revoke_api_key() {
        let auth = AuthService::new();
        
        auth.register_api_key("key1".to_string(), 1).await;
        assert!(auth.has_api_key("key1").await);
        
        // Revoke the key
        assert!(auth.revoke_api_key("key1").await);
        assert!(!auth.has_api_key("key1").await);
        
        // Revoking again returns false
        assert!(!auth.revoke_api_key("key1").await);
    }

    #[tokio::test]
    async fn test_key_count() {
        let auth = AuthService::new();
        
        assert_eq!(auth.key_count().await, 0);
        
        auth.register_api_key("key1".to_string(), 1).await;
        auth.register_api_key("key2".to_string(), 2).await;
        
        assert_eq!(auth.key_count().await, 2);
        
        auth.revoke_api_key("key1").await;
        assert_eq!(auth.key_count().await, 1);
    }

    #[tokio::test]
    async fn test_multiple_accounts() {
        let auth = AuthService::new();
        
        // Register multiple keys for different accounts
        auth.register_api_key("alice-key".to_string(), 100).await;
        auth.register_api_key("bob-key".to_string(), 200).await;
        auth.register_api_key("charlie-key".to_string(), 300).await;
        
        // Verify each returns correct account
        assert_eq!(auth.authenticate("alice-key").await.unwrap(), 100);
        assert_eq!(auth.authenticate("bob-key").await.unwrap(), 200);
        assert_eq!(auth.authenticate("charlie-key").await.unwrap(), 300);
    }
}
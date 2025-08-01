//! This module defines the `RelayerRepository` trait and its in-memory implementation,
//! `InMemoryRelayerRepository`. It provides functionality for managing relayers, including
//! creating, updating, enabling, disabling, and listing relayers. The module also includes
//! conversion logic for transforming configuration file data into repository models and
//! implements pagination for listing relayers.
//!
//! The `RelayerRepository` trait is designed to be implemented by any storage backend,
//! allowing for flexibility in how relayers are stored and managed. The in-memory
//! implementation is useful for testing and development purposes.
use crate::models::PaginationQuery;
use crate::{
    domain::RelayerUpdateRequest,
    models::{RelayerNetworkPolicy, RelayerRepoModel, RepositoryError},
};
use async_trait::async_trait;
use eyre::Result;
use std::collections::HashMap;
use tokio::sync::{Mutex, MutexGuard};

use crate::repositories::{PaginatedResult, RelayerRepository, Repository};

#[derive(Debug)]
pub struct InMemoryRelayerRepository {
    store: Mutex<HashMap<String, RelayerRepoModel>>,
}

impl InMemoryRelayerRepository {
    pub fn new() -> Self {
        Self {
            store: Mutex::new(HashMap::new()),
        }
    }
    async fn acquire_lock<T>(lock: &Mutex<T>) -> Result<MutexGuard<T>, RepositoryError> {
        Ok(lock.lock().await)
    }
}

impl Default for InMemoryRelayerRepository {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for InMemoryRelayerRepository {
    fn clone(&self) -> Self {
        // Try to get the current data, or use empty HashMap if lock fails
        let data = self
            .store
            .try_lock()
            .map(|guard| guard.clone())
            .unwrap_or_else(|_| HashMap::new());

        Self {
            store: Mutex::new(data),
        }
    }
}

#[async_trait]
impl RelayerRepository for InMemoryRelayerRepository {
    async fn list_active(&self) -> Result<Vec<RelayerRepoModel>, RepositoryError> {
        let store = Self::acquire_lock(&self.store).await?;
        let active_relayers: Vec<RelayerRepoModel> = store
            .values()
            .filter(|&relayer| !relayer.paused)
            .cloned()
            .collect();
        Ok(active_relayers)
    }

    async fn partial_update(
        &self,
        id: String,
        update: RelayerUpdateRequest,
    ) -> Result<RelayerRepoModel, RepositoryError> {
        let mut store = Self::acquire_lock(&self.store).await?;
        if let Some(relayer) = store.get_mut(&id) {
            if let Some(paused) = update.paused {
                relayer.paused = paused;
            }
            Ok(relayer.clone())
        } else {
            Err(RepositoryError::NotFound(format!(
                "Relayer with ID {} not found",
                id
            )))
        }
    }

    async fn update_policy(
        &self,
        id: String,
        policy: RelayerNetworkPolicy,
    ) -> Result<RelayerRepoModel, RepositoryError> {
        let mut store = Self::acquire_lock(&self.store).await?;
        let relayer = store.get_mut(&id).ok_or_else(|| {
            RepositoryError::NotFound(format!("Relayer with ID {} not found", id))
        })?;
        relayer.policies = policy;
        Ok(relayer.clone())
    }

    async fn disable_relayer(
        &self,
        relayer_id: String,
    ) -> Result<RelayerRepoModel, RepositoryError> {
        let mut store = self.store.lock().await;
        if let Some(relayer) = store.get_mut(&relayer_id) {
            relayer.system_disabled = true;
            Ok(relayer.clone())
        } else {
            Err(RepositoryError::NotFound(format!(
                "Relayer with ID {} not found",
                relayer_id
            )))
        }
    }

    async fn enable_relayer(
        &self,
        relayer_id: String,
    ) -> Result<RelayerRepoModel, RepositoryError> {
        let mut store = self.store.lock().await;
        if let Some(relayer) = store.get_mut(&relayer_id) {
            relayer.system_disabled = false;
            Ok(relayer.clone())
        } else {
            Err(RepositoryError::NotFound(format!(
                "Relayer with ID {} not found",
                relayer_id
            )))
        }
    }
}

#[async_trait]
impl Repository<RelayerRepoModel, String> for InMemoryRelayerRepository {
    async fn create(&self, relayer: RelayerRepoModel) -> Result<RelayerRepoModel, RepositoryError> {
        let mut store = Self::acquire_lock(&self.store).await?;
        if store.contains_key(&relayer.id) {
            return Err(RepositoryError::ConstraintViolation(format!(
                "Relayer with ID {} already exists",
                relayer.id
            )));
        }
        store.insert(relayer.id.clone(), relayer.clone());
        Ok(relayer)
    }

    async fn get_by_id(&self, id: String) -> Result<RelayerRepoModel, RepositoryError> {
        let store = Self::acquire_lock(&self.store).await?;
        match store.get(&id) {
            Some(relayer) => Ok(relayer.clone()),
            None => Err(RepositoryError::NotFound(format!(
                "Relayer with ID {} not found",
                id
            ))),
        }
    }
    #[allow(clippy::map_entry)]
    async fn update(
        &self,
        id: String,
        relayer: RelayerRepoModel,
    ) -> Result<RelayerRepoModel, RepositoryError> {
        let mut store = Self::acquire_lock(&self.store).await?;
        if store.contains_key(&id) {
            // Ensure we update the existing entry
            let mut updated_relayer = relayer;
            updated_relayer.id = id.clone(); // Preserve original ID
            store.insert(id, updated_relayer.clone());
            Ok(updated_relayer)
        } else {
            Err(RepositoryError::NotFound(format!(
                "Relayer with ID {} not found",
                id
            )))
        }
    }

    async fn delete_by_id(&self, id: String) -> Result<(), RepositoryError> {
        let mut store = Self::acquire_lock(&self.store).await?;
        if store.remove(&id).is_some() {
            Ok(())
        } else {
            Err(RepositoryError::NotFound(format!(
                "Relayer with ID {} not found",
                id
            )))
        }
    }

    async fn list_all(&self) -> Result<Vec<RelayerRepoModel>, RepositoryError> {
        let store = Self::acquire_lock(&self.store).await?;
        Ok(store.values().cloned().collect())
    }

    async fn list_paginated(
        &self,
        query: PaginationQuery,
    ) -> Result<PaginatedResult<RelayerRepoModel>, RepositoryError> {
        let total = self.count().await?;
        let start = ((query.page - 1) * query.per_page) as usize;
        let items = self
            .store
            .lock()
            .await
            .values()
            .skip(start)
            .take(query.per_page as usize)
            .cloned()
            .collect();
        Ok(PaginatedResult {
            items,
            total: total as u64,
            page: query.page,
            per_page: query.per_page,
        })
    }

    async fn count(&self) -> Result<usize, RepositoryError> {
        Ok(self.store.lock().await.len())
    }

    async fn has_entries(&self) -> Result<bool, RepositoryError> {
        let store = Self::acquire_lock(&self.store).await?;
        Ok(!store.is_empty())
    }

    async fn drop_all_entries(&self) -> Result<(), RepositoryError> {
        let mut store = Self::acquire_lock(&self.store).await?;
        store.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::models::{NetworkType, RelayerEvmPolicy};

    use super::*;

    fn create_test_relayer(id: String) -> RelayerRepoModel {
        RelayerRepoModel {
            id: id.clone(),
            name: format!("Relayer {}", id.clone()),
            network: "TestNet".to_string(),
            paused: false,
            network_type: NetworkType::Evm,
            policies: RelayerNetworkPolicy::Evm(RelayerEvmPolicy {
                gas_price_cap: None,
                whitelist_receivers: None,
                eip1559_pricing: Some(false),
                private_transactions: false,
                min_balance: 0,
                gas_limit_estimation: Some(true),
            }),
            signer_id: "test".to_string(),
            address: "0x".to_string(),
            notification_id: None,
            system_disabled: false,
            custom_rpc_urls: None,
        }
    }

    #[actix_web::test]
    async fn test_new_repository_is_empty() {
        let repo = InMemoryRelayerRepository::new();
        assert_eq!(repo.count().await.unwrap(), 0);
    }

    #[actix_web::test]
    async fn test_add_relayer() {
        let repo = InMemoryRelayerRepository::new();
        let relayer = create_test_relayer("test".to_string());

        repo.create(relayer.clone()).await.unwrap();
        assert_eq!(repo.count().await.unwrap(), 1);

        let stored = repo.get_by_id("test".to_string()).await.unwrap();
        assert_eq!(stored.id, relayer.id);
        assert_eq!(stored.name, relayer.name);
    }

    #[actix_web::test]
    async fn test_update_relayer() {
        let repo = InMemoryRelayerRepository::new();
        let mut relayer = create_test_relayer("test".to_string());

        repo.create(relayer.clone()).await.unwrap();

        relayer.name = "Updated Name".to_string();
        repo.update("test".to_string(), relayer.clone())
            .await
            .unwrap();

        let updated = repo.get_by_id("test".to_string()).await.unwrap();
        assert_eq!(updated.name, "Updated Name");
    }

    #[actix_web::test]
    async fn test_list_relayers() {
        let repo = InMemoryRelayerRepository::new();
        let relayer1 = create_test_relayer("test".to_string());
        let relayer2 = create_test_relayer("test2".to_string());

        repo.create(relayer1.clone()).await.unwrap();
        repo.create(relayer2).await.unwrap();

        let relayers = repo.list_all().await.unwrap();
        assert_eq!(relayers.len(), 2);
    }

    #[actix_web::test]
    async fn test_list_active_relayers() {
        let repo = InMemoryRelayerRepository::new();
        let relayer1 = create_test_relayer("test".to_string());
        let mut relayer2 = create_test_relayer("test2".to_string());

        relayer2.paused = true;

        repo.create(relayer1.clone()).await.unwrap();
        repo.create(relayer2).await.unwrap();

        let active_relayers = repo.list_active().await.unwrap();
        assert_eq!(active_relayers.len(), 1);
        assert_eq!(active_relayers[0].id, "test".to_string());
    }

    #[actix_web::test]
    async fn test_update_nonexistent_relayer() {
        let repo = InMemoryRelayerRepository::new();
        let relayer = create_test_relayer("test".to_string());

        let result = repo.update("test".to_string(), relayer).await;
        assert!(matches!(result, Err(RepositoryError::NotFound(_))));
    }

    #[actix_web::test]
    async fn test_get_nonexistent_relayer() {
        let repo = InMemoryRelayerRepository::new();

        let result = repo.get_by_id("test".to_string()).await;
        assert!(matches!(result, Err(RepositoryError::NotFound(_))));
    }

    #[actix_web::test]
    async fn test_partial_update_relayer() {
        let repo = InMemoryRelayerRepository::new();

        // Add a relayer to the repository
        let relayer_id = "test_relayer".to_string();
        let initial_relayer = create_test_relayer(relayer_id.clone());

        repo.create(initial_relayer.clone()).await.unwrap();

        // Perform a partial update on the relayer
        let update_req = RelayerUpdateRequest { paused: Some(true) };

        let updated_relayer = repo
            .partial_update(relayer_id.clone(), update_req)
            .await
            .unwrap();

        assert_eq!(updated_relayer.id, initial_relayer.id);
        assert!(updated_relayer.paused);
    }

    #[actix_web::test]
    async fn test_disable_relayer() {
        let repo = InMemoryRelayerRepository::new();

        // Add a relayer to the repository
        let relayer_id = "test_relayer".to_string();
        let initial_relayer = create_test_relayer(relayer_id.clone());

        repo.create(initial_relayer.clone()).await.unwrap();

        // Disable the relayer
        let disabled_relayer = repo.disable_relayer(relayer_id.clone()).await.unwrap();

        assert_eq!(disabled_relayer.id, initial_relayer.id);
        assert!(disabled_relayer.system_disabled);
    }

    #[actix_web::test]
    async fn test_enable_relayer() {
        let repo = InMemoryRelayerRepository::new();

        // Add a relayer to the repository
        let relayer_id = "test_relayer".to_string();
        let mut initial_relayer = create_test_relayer(relayer_id.clone());

        initial_relayer.system_disabled = true;

        repo.create(initial_relayer.clone()).await.unwrap();

        // Enable the relayer
        let enabled_relayer = repo.enable_relayer(relayer_id.clone()).await.unwrap();

        assert_eq!(enabled_relayer.id, initial_relayer.id);
        assert!(!enabled_relayer.system_disabled);
    }

    #[actix_web::test]
    async fn test_update_policy() {
        let repo = InMemoryRelayerRepository::new();
        let relayer = create_test_relayer("test".to_string());

        repo.create(relayer.clone()).await.unwrap();

        // Create a new policy to update
        let new_policy = RelayerNetworkPolicy::Evm(RelayerEvmPolicy {
            gas_price_cap: Some(50000000000),
            whitelist_receivers: Some(vec!["0x1234".to_string()]),
            eip1559_pricing: Some(true),
            private_transactions: true,
            min_balance: 1000000,
            gas_limit_estimation: Some(true),
        });

        // Update the policy
        let updated_relayer = repo
            .update_policy("test".to_string(), new_policy.clone())
            .await
            .unwrap();

        // Verify the policy was updated
        match updated_relayer.policies {
            RelayerNetworkPolicy::Evm(policy) => {
                assert_eq!(policy.gas_price_cap, Some(50000000000));
                assert_eq!(policy.whitelist_receivers, Some(vec!["0x1234".to_string()]));
                assert_eq!(policy.eip1559_pricing, Some(true));
                assert!(policy.private_transactions);
                assert_eq!(policy.min_balance, 1000000);
            }
            _ => panic!("Unexpected policy type"),
        }
    }

    // test has_entries
    #[actix_web::test]
    async fn test_has_entries() {
        let repo = InMemoryRelayerRepository::new();
        assert!(!repo.has_entries().await.unwrap());

        let relayer = create_test_relayer("test".to_string());

        repo.create(relayer.clone()).await.unwrap();
        assert!(repo.has_entries().await.unwrap());
    }

    #[actix_web::test]
    async fn test_drop_all_entries() {
        let repo = InMemoryRelayerRepository::new();
        let relayer = create_test_relayer("test".to_string());

        repo.create(relayer.clone()).await.unwrap();

        assert!(repo.has_entries().await.unwrap());

        repo.drop_all_entries().await.unwrap();
        assert!(!repo.has_entries().await.unwrap());
    }
}

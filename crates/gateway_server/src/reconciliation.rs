//! Reconciliation between gateway and engines
//!
//! The gateway maintains in-memory reservation state. When the gateway or an engine crashes,
//! we need to reconcile state to ensure consistency and prevent double-spend.
//!
//! Two reconciliation scenarios:
//! 1. Gateway startup: Query all engines and rebuild reservation state
//! 2. Engine restart: Detect "ghost orders" (orders gateway thinks exist but engine doesn't have)

use crate::account_manager::AccountManager;
use crate::engine_router::EngineRouter;
use common::{AccountId, OrderSnapshot, SymbolId};
use std::collections::HashMap;
use std::sync::Arc;

pub struct Reconciliation {
    account_manager: Arc<AccountManager>,
    engine_router: Arc<EngineRouter>,
}

impl Reconciliation {
    pub fn new(account_manager: Arc<AccountManager>, engine_router: Arc<EngineRouter>) -> Self {
        Self {
            account_manager,
            engine_router,
        }
    }

    /// Query all engines and rebuild reservation state
    ///
    /// This is called on gateway startup after loading accounts from journal.
    /// It rebuilds the in-memory tentative_reserved values by querying all engines
    /// for their active orders.
    pub async fn rebuild_reservations(&self) -> anyhow::Result<()> {
        tracing::info!("Starting reservation reconciliation with engines");

        // Query all engines for their orders
        let all_orders = self.engine_router.query_all_engines_for_orders().await?;

        tracing::info!("Received {} orders from all engines", all_orders.len());

        // Group by account_id and sum reserved amounts
        let mut reservations_by_account: HashMap<AccountId, i64> = HashMap::new();

        for order in &all_orders {
            *reservations_by_account.entry(order.account_id).or_insert(0) += order.reserved_amount;
        }

        // Update tentative_reserved for each account
        for (account_id, total_reserved) in &reservations_by_account {
            self.account_manager
                .set_tentative_reserved(*account_id, *total_reserved);

            let order_count = all_orders
                .iter()
                .filter(|o| o.account_id == *account_id)
                .count();

            tracing::debug!(
                "Account {} has {} reserved from {} orders",
                account_id,
                total_reserved,
                order_count
            );
        }

        tracing::info!(
            "Reconciliation complete: {} accounts with active orders",
            reservations_by_account.len()
        );

        Ok(())
    }

    /// Handle engine restart - detect and release ghost reservations
    ///
    /// When an engine crashes and restarts, some orders may have been lost (commands
    /// in the batch buffer that didn't get fsynced). The gateway may still think these
    /// orders exist. We need to detect these "ghost orders" and release their reservations.
    ///
    /// This is called when we receive an EngineReady message from an engine.
    #[allow(dead_code)]
    pub async fn reconcile_engine_restart(
        &self,
        _symbol_id: SymbolId,
        _engine_orders: Vec<OrderSnapshot>,
    ) -> anyhow::Result<()> {
        // TODO: This requires access to pending_orders from client_handler
        // For now, we'll implement this in Phase 4 when we integrate everything
        tracing::warn!("Engine restart reconciliation not yet fully implemented");
        Ok(())
    }
}

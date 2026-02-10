//! Account state management with tentative reservations for risk checks
//!
//! This module manages all account state in-memory including:
//! - Buying power and balance
//! - Positions across all symbols
//! - Risk limits
//! - Tentative reservations to prevent race conditions

use common::{
    AccountId, Command, Event, NewOrder, Position as CommonPosition,
    RiskLimits as CommonRiskLimits, Side, SymbolId,
};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Events that modify account state (for journaling)
///
/// NOTE: Only account creation is journaled. Fills, risk limits, and balance updates
/// are NOT journaled - engines are the source of truth for these.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AccountUpdate {
    /// Account created with initial buying power
    CreateAccount {
        account_id: AccountId,
        buying_power: i64,
    },
    // REMOVED: Fill, SetRiskLimits, AdjustBuyingPower
    // These are no longer journaled - engines are source of truth
}

/// Account state for a single account
#[derive(Debug, Clone)]
pub struct AccountState {
    #[allow(dead_code)]
    pub account_id: AccountId,
    pub buying_power: i64,
    /// Money locked for pending orders (prevents double-spend)
    pub tentative_reserved: i64,
    /// Positions by symbol_id
    pub positions: HashMap<SymbolId, CommonPosition>,
    /// Risk limits by symbol_id
    pub risk_limits: HashMap<SymbolId, CommonRiskLimits>,
}

impl AccountState {
    pub fn new(account_id: AccountId, initial_buying_power: i64) -> Self {
        Self {
            account_id,
            buying_power: initial_buying_power,
            tentative_reserved: 0,
            positions: HashMap::new(),
            risk_limits: HashMap::new(),
        }
    }

    /// Get or create default risk limits for a symbol
    fn get_risk_limits(&self, symbol_id: SymbolId) -> CommonRiskLimits {
        self.risk_limits
            .get(&symbol_id)
            .copied()
            .unwrap_or_default()
    }

    /// Get current position for a symbol (or default)
    fn get_position(&self, symbol_id: SymbolId) -> CommonPosition {
        self.positions.get(&symbol_id).copied().unwrap_or_default()
    }
}

/// Token representing a tentative reservation of buying power
#[derive(Debug, Clone)]
pub struct ReservationToken {
    pub account_id: AccountId,
    #[allow(dead_code)]
    pub symbol_id: SymbolId,
    pub amount: i64,
    #[allow(dead_code)]
    pub order_id: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskViolation {
    InsufficientFunds,
    PositionLimitExceeded,
    OrderSizeTooLarge,
    AccountNotFound,
}

/// Manages all account state with per-account locking for race-condition-free risk checks
pub struct AccountManager {
    /// Concurrent hashmap with per-account locks
    accounts: DashMap<AccountId, AccountState>,
    /// Global sequence number for idempotency
    next_gateway_seq: std::sync::atomic::AtomicU64,
    /// Journal for persistence (optional, None = no persistence)
    journal: Option<std::sync::Mutex<crate::persistence::AccountJournal>>,
}

impl AccountManager {
    pub fn new() -> Self {
        Self {
            accounts: DashMap::new(),
            next_gateway_seq: std::sync::atomic::AtomicU64::new(1),
            journal: None,
        }
    }

    /// Create AccountManager with persistence enabled
    pub fn with_journal(journal: crate::persistence::AccountJournal) -> Self {
        Self {
            accounts: DashMap::new(),
            next_gateway_seq: std::sync::atomic::AtomicU64::new(1),
            journal: Some(std::sync::Mutex::new(journal)),
        }
    }

    /// Log an account update to the journal (if enabled)
    fn log_update(&self, update: AccountUpdate) {
        if let Some(journal) = &self.journal {
            let mut j = journal.lock().unwrap();
            if let Err(e) = j.append(&update) {
                tracing::error!("Failed to append to journal: {}", e);
            }
        }
    }

    /// Flush journal to disk
    pub fn flush_journal(&self) -> anyhow::Result<()> {
        if let Some(journal) = &self.journal {
            let mut j = journal.lock().unwrap();
            j.flush()?;
        }
        Ok(())
    }

    /// Create or fund an account
    pub fn create_account(&self, account_id: AccountId, buying_power: i64) {
        self.accounts
            .insert(account_id, AccountState::new(account_id, buying_power));

        // Log to journal
        self.log_update(AccountUpdate::CreateAccount {
            account_id,
            buying_power,
        });
    }

    /// Get next gateway sequence number (for idempotency)
    pub fn next_seq(&self) -> u64 {
        self.next_gateway_seq
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }

    /// Check risk and reserve buying power for a new order
    pub fn check_and_reserve(&self, cmd: &Command) -> Result<ReservationToken, RiskViolation> {
        match cmd {
            Command::NewOrder(order) => self.check_and_reserve_order(order),
            Command::Replace(replace) => self.check_and_reserve_replace(replace),
            // Other commands don't need reservations (Cancel is handled separately)
            _ => Ok(ReservationToken {
                account_id: Self::extract_account_id(cmd),
                symbol_id: Self::extract_symbol_id(cmd),
                amount: 0,
                order_id: 0,
            }),
        }
    }

    fn check_and_reserve_order(&self, order: &NewOrder) -> Result<ReservationToken, RiskViolation> {
        // Get account with lock
        let mut account = self
            .accounts
            .get_mut(&order.account_id)
            .ok_or(RiskViolation::AccountNotFound)?;

        let state = account.value_mut();

        // Get risk limits
        let limits = state.get_risk_limits(order.symbol_id);

        // Check order size
        if order.qty.abs() > limits.max_order_size {
            return Err(RiskViolation::OrderSizeTooLarge);
        }

        // Get current position
        let position = state.get_position(order.symbol_id);

        // Calculate new position if this order fills
        let new_position = match order.side {
            Side::Buy => position.net_position + order.qty,
            Side::Sell => position.net_position - order.qty,
        };

        // Check position limits
        if new_position > limits.max_long_position {
            return Err(RiskViolation::PositionLimitExceeded);
        }
        if new_position < -(limits.max_short_position) {
            return Err(RiskViolation::PositionLimitExceeded);
        }

        // Calculate buying power required (for buys, reserve full notional)
        let required = match order.side {
            Side::Buy => order.price * order.qty,
            Side::Sell => 0, // Selling doesn't require buying power
        };

        // Check available funds
        let available = state.buying_power - state.tentative_reserved;
        if required > available {
            return Err(RiskViolation::InsufficientFunds);
        }

        // Reserve the buying power
        state.tentative_reserved += required;

        Ok(ReservationToken {
            account_id: order.account_id,
            symbol_id: order.symbol_id,
            amount: required,
            order_id: order.order_id,
        })
    }

    /// Apply a fill from the engine, releasing tentative reservation and updating actual position
    ///
    /// Note: The gateway must track which account_id corresponds to which order_id,
    /// since Fill events don't include account_id (only order_ids)
    pub fn apply_fill(&self, event: &Event, token: &ReservationToken, is_buy: bool) {
        if let Event::Fill(fill) = event {
            if let Some(mut account) = self.accounts.get_mut(&token.account_id) {
                let state = account.value_mut();

                // Release tentative reservation
                state.tentative_reserved -= token.amount;

                // Calculate actual cost/proceeds
                let actual_cost = fill.price * fill.qty;

                // Update buying power
                // For buys: reduce buying power (spent money)
                // For sells: increase buying power (received money)
                if is_buy {
                    state.buying_power -= actual_cost;
                } else {
                    state.buying_power += actual_cost;
                }

                // Update position
                let position = state
                    .positions
                    .entry(fill.symbol_id)
                    .or_insert(CommonPosition::default());

                // Update net position
                if is_buy {
                    position.net_position += fill.qty;
                } else {
                    position.net_position -= fill.qty;
                }

                // Update average price (simplified - should be volume-weighted)
                if position.net_position != 0 {
                    position.avg_price = fill.price;
                }

                // Realized PnL calculation would go here
                // For now, simplified

                // NOTE: Fills are NOT journaled - engines are the source of truth
            }
        }
    }

    /// Release a tentative reservation (order cancelled or rejected by engine)
    pub fn release_reservation(&self, token: &ReservationToken) {
        if let Some(mut account) = self.accounts.get_mut(&token.account_id) {
            let state = account.value_mut();
            state.tentative_reserved -= token.amount;
        }
    }

    /// Check risk and reserve for Replace command
    /// This adjusts the existing reservation (release old, reserve new)
    fn check_and_reserve_replace(
        &self,
        replace: &common::Replace,
    ) -> Result<ReservationToken, RiskViolation> {
        // Get account with lock
        let mut account = self
            .accounts
            .get_mut(&replace.account_id)
            .ok_or(RiskViolation::AccountNotFound)?;

        let state = account.value_mut();

        // Get risk limits
        let limits = state.get_risk_limits(replace.symbol_id);

        // Check order size
        if replace.new_qty.abs() > limits.max_order_size {
            return Err(RiskViolation::OrderSizeTooLarge);
        }

        // Note: We don't know the side yet (engine will infer it from existing order)
        // For now, we'll assume worst case: it's a buy order
        // The engine will validate the actual side when it processes the replace

        // Calculate new reservation amount (assuming buy side, worst case)
        let new_amount = replace.new_price * replace.new_qty;

        // For replace, we assume the old reservation exists and will be released
        // The caller (client_handler) should track the old reservation and pass it
        // For now, we just check if the NEW amount is affordable with current state

        let available = state.buying_power - state.tentative_reserved;
        if new_amount > available {
            return Err(RiskViolation::InsufficientFunds);
        }

        // Reserve the new amount
        // Note: The caller must release the old reservation separately
        state.tentative_reserved += new_amount;

        Ok(ReservationToken {
            account_id: replace.account_id,
            symbol_id: replace.symbol_id,
            amount: new_amount,
            order_id: replace.order_id,
        })
    }

    /// Adjust reservation for Replace (atomic: release old, reserve new)
    /// This is the preferred method for handling Replace commands
    pub fn adjust_reservation(
        &self,
        old_token: &ReservationToken,
        replace: &common::Replace,
    ) -> Result<ReservationToken, RiskViolation> {
        // Get account with lock
        let mut account = self
            .accounts
            .get_mut(&replace.account_id)
            .ok_or(RiskViolation::AccountNotFound)?;

        let state = account.value_mut();

        // Get risk limits
        let limits = state.get_risk_limits(replace.symbol_id);

        // Check order size
        if replace.new_qty.abs() > limits.max_order_size {
            return Err(RiskViolation::OrderSizeTooLarge);
        }

        // Calculate new reservation amount
        let new_amount = replace.new_price * replace.new_qty;

        // Calculate available including the old reservation (which we'll release)
        let available = state.buying_power - state.tentative_reserved + old_token.amount;

        if new_amount > available {
            return Err(RiskViolation::InsufficientFunds);
        }

        // Atomically adjust: release old, reserve new
        state.tentative_reserved = state.tentative_reserved - old_token.amount + new_amount;

        Ok(ReservationToken {
            account_id: replace.account_id,
            symbol_id: replace.symbol_id,
            amount: new_amount,
            order_id: replace.order_id,
        })
    }

    /// Get account state for querying
    pub fn get_account(&self, account_id: AccountId) -> Option<AccountState> {
        self.accounts.get(&account_id).map(|r| r.value().clone())
    }

    /// Set tentative_reserved directly (used during reconciliation)
    pub fn set_tentative_reserved(&self, account_id: AccountId, amount: i64) {
        if let Some(mut account) = self.accounts.get_mut(&account_id) {
            account.tentative_reserved = amount;
        }
    }

    /// Get current tentative_reserved (for debugging/monitoring)
    #[allow(dead_code)]
    pub fn get_tentative_reserved(&self, account_id: AccountId) -> i64 {
        self.accounts
            .get(&account_id)
            .map(|a| a.tentative_reserved)
            .unwrap_or(0)
    }

    /// Create a snapshot of all account state
    pub fn create_snapshot(&self, sequence: u64) -> crate::persistence::AccountSnapshot {
        let accounts: Vec<crate::persistence::AccountStateSnapshot> = self
            .accounts
            .iter()
            .map(|entry| {
                let state = entry.value();
                crate::persistence::AccountStateSnapshot {
                    account_id: state.account_id,
                    buying_power: state.buying_power,
                    positions: state.positions.iter().map(|(k, v)| (*k, *v)).collect(),
                    risk_limits: state.risk_limits.iter().map(|(k, v)| (*k, *v)).collect(),
                }
            })
            .collect();

        crate::persistence::AccountSnapshot { sequence, accounts }
    }

    /// Restore account state from snapshot
    pub fn restore_from_snapshot(&self, snapshot: &crate::persistence::AccountSnapshot) {
        // Clear existing accounts
        self.accounts.clear();

        // Restore each account
        for acc in &snapshot.accounts {
            let mut state = AccountState::new(acc.account_id, acc.buying_power);

            // Restore positions
            for (symbol_id, position) in &acc.positions {
                state.positions.insert(*symbol_id, *position);
            }

            // Restore risk limits
            for (symbol_id, limits) in &acc.risk_limits {
                state.risk_limits.insert(*symbol_id, *limits);
            }

            self.accounts.insert(acc.account_id, state);
        }

        tracing::info!(
            "Restored {} accounts from snapshot (seq={})",
            snapshot.accounts.len(),
            snapshot.sequence
        );
    }

    /// Replay journal updates to rebuild state
    ///
    /// Since we only journal account creation now, this simply recreates accounts.
    /// Fills, risk limits, and balance updates are handled via engine reconciliation.
    pub fn replay_journal(&self, updates: Vec<AccountUpdate>) {
        let update_count = updates.len();
        for update in updates {
            match update {
                AccountUpdate::CreateAccount {
                    account_id,
                    buying_power,
                } => {
                    // Don't log during replay
                    self.accounts
                        .insert(account_id, AccountState::new(account_id, buying_power));
                } // No other variants exist after simplification
            }
        }

        tracing::info!("Replayed {} account creations from journal", update_count);
    }

    /// Helper to extract account_id from any command
    fn extract_account_id(cmd: &Command) -> AccountId {
        match cmd {
            Command::NewOrder(o) => o.account_id,
            Command::Cancel(c) => c.account_id,
            Command::Replace(r) => r.account_id,
            Command::SetRiskLimits(s) => s.account_id,
            Command::QueryAccount(q) => q.account_id,
            Command::Authenticate(_) => 0, // Auth doesn't have account_id yet
        }
    }

    /// Helper to extract symbol_id from any command
    fn extract_symbol_id(cmd: &Command) -> SymbolId {
        match cmd {
            Command::NewOrder(o) => o.symbol_id,
            Command::Cancel(c) => c.symbol_id,
            Command::Replace(r) => r.symbol_id,
            Command::SetRiskLimits(s) => s.symbol_id,
            Command::QueryAccount(q) => q.symbol_id,
            Command::Authenticate(_) => 0, // Auth doesn't have symbol_id
        }
    }
}

impl Default for AccountManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::{OrderFlags, TimeInForce};

    fn make_test_order(
        account_id: AccountId,
        symbol_id: SymbolId,
        side: Side,
        price: i64,
        qty: i64,
    ) -> NewOrder {
        NewOrder {
            client_seq: 1,
            order_id: 1,
            account_id,
            symbol_id,
            side,
            price,
            qty,
            tif: TimeInForce::Gtc,
            flags: OrderFlags::default(),
        }
    }

    fn make_test_replace(
        account_id: AccountId,
        symbol_id: SymbolId,
        order_id: u64,
        new_price: i64,
        new_qty: i64,
    ) -> common::Replace {
        common::Replace {
            client_seq: 2,
            order_id,
            account_id,
            symbol_id,
            new_price,
            new_qty,
        }
    }

    #[test]
    fn test_create_account() {
        let mgr = AccountManager::new();
        mgr.create_account(100, 50000);

        let account = mgr.get_account(100).unwrap();
        assert_eq!(account.buying_power, 50000);
        assert_eq!(account.tentative_reserved, 0);
    }

    #[test]
    fn test_check_and_reserve_sufficient_funds() {
        let mgr = AccountManager::new();
        mgr.create_account(100, 100000);

        let order = make_test_order(100, 1, Side::Buy, 50000, 1);
        let result = mgr.check_and_reserve(&Command::NewOrder(order));

        assert!(result.is_ok());
        let token = result.unwrap();
        assert_eq!(token.amount, 50000);

        // Check reservation was made
        let account = mgr.get_account(100).unwrap();
        assert_eq!(account.tentative_reserved, 50000);
    }

    #[test]
    fn test_check_and_reserve_insufficient_funds() {
        let mgr = AccountManager::new();
        mgr.create_account(100, 30000);

        let order = make_test_order(100, 1, Side::Buy, 50000, 1);
        let result = mgr.check_and_reserve(&Command::NewOrder(order));

        assert_eq!(result.unwrap_err(), RiskViolation::InsufficientFunds);
    }

    #[test]
    fn test_double_spend_prevention() {
        let mgr = AccountManager::new();
        mgr.create_account(100, 100000);

        let order1 = make_test_order(100, 1, Side::Buy, 60000, 1);
        let order2 = make_test_order(100, 1, Side::Buy, 60000, 1);

        // First order should succeed
        let result1 = mgr.check_and_reserve(&Command::NewOrder(order1));
        assert!(result1.is_ok());

        // Second order should fail (would exceed buying power)
        let result2 = mgr.check_and_reserve(&Command::NewOrder(order2));
        assert_eq!(result2.unwrap_err(), RiskViolation::InsufficientFunds);
    }

    #[test]
    fn test_release_reservation() {
        let mgr = AccountManager::new();
        mgr.create_account(100, 100000);

        let order = make_test_order(100, 1, Side::Buy, 50000, 1);
        let token = mgr.check_and_reserve(&Command::NewOrder(order)).unwrap();

        // Release the reservation
        mgr.release_reservation(&token);

        // Should be able to reserve again
        let order2 = make_test_order(100, 1, Side::Buy, 50000, 1);
        let result = mgr.check_and_reserve(&Command::NewOrder(order2));
        assert!(result.is_ok());
    }

    #[test]
    fn test_sell_order_no_reservation() {
        let mgr = AccountManager::new();
        mgr.create_account(100, 0); // No buying power

        let order = make_test_order(100, 1, Side::Sell, 50000, 1);
        let result = mgr.check_and_reserve(&Command::NewOrder(order));

        // Should succeed - selling doesn't require buying power
        assert!(result.is_ok());
        assert_eq!(result.unwrap().amount, 0);
    }

    #[test]
    fn test_account_not_found() {
        let mgr = AccountManager::new();

        let order = make_test_order(999, 1, Side::Buy, 50000, 1);
        let result = mgr.check_and_reserve(&Command::NewOrder(order));

        assert_eq!(result.unwrap_err(), RiskViolation::AccountNotFound);
    }

    #[test]
    fn test_cancel_releases_reservation() {
        let mgr = AccountManager::new();
        mgr.create_account(100, 100000);

        // Create an order
        let order = make_test_order(100, 1, Side::Buy, 50000, 1);
        let token = mgr.check_and_reserve(&Command::NewOrder(order)).unwrap();

        // Verify reservation was made
        let account = mgr.get_account(100).unwrap();
        assert_eq!(account.tentative_reserved, 50000);

        // Release the reservation (simulating cancel)
        mgr.release_reservation(&token);

        // Verify reservation was released
        let account = mgr.get_account(100).unwrap();
        assert_eq!(account.tentative_reserved, 0);
    }

    #[test]
    fn test_replace_adjust_reservation() {
        let mgr = AccountManager::new();
        mgr.create_account(100, 100000);

        // Create initial order
        let order = make_test_order(100, 1, Side::Buy, 50000, 1);
        let old_token = mgr.check_and_reserve(&Command::NewOrder(order)).unwrap();

        // Verify initial reservation
        let account = mgr.get_account(100).unwrap();
        assert_eq!(account.tentative_reserved, 50000);
        assert_eq!(account.buying_power, 100000);

        // Replace with higher price (need more funds)
        let replace = make_test_replace(100, 1, 1, 60000, 1);
        let new_token = mgr.adjust_reservation(&old_token, &replace).unwrap();

        // Verify adjusted reservation
        let account = mgr.get_account(100).unwrap();
        assert_eq!(account.tentative_reserved, 60000);
        assert_eq!(new_token.amount, 60000);
    }

    #[test]
    fn test_replace_insufficient_funds() {
        let mgr = AccountManager::new();
        mgr.create_account(100, 60000);

        // Create initial order (uses 50k)
        let order = make_test_order(100, 1, Side::Buy, 50000, 1);
        let old_token = mgr.check_and_reserve(&Command::NewOrder(order)).unwrap();

        // Try to replace with much higher price (need 100k total, only have 60k)
        let replace = make_test_replace(100, 1, 1, 100000, 1);
        let result = mgr.adjust_reservation(&old_token, &replace);

        // Should fail - insufficient funds
        assert_eq!(result.unwrap_err(), RiskViolation::InsufficientFunds);

        // Original reservation should remain unchanged
        let account = mgr.get_account(100).unwrap();
        assert_eq!(account.tentative_reserved, 50000);
    }

    #[test]
    fn test_replace_decrease_price() {
        let mgr = AccountManager::new();
        mgr.create_account(100, 100000);

        // Create initial order
        let order = make_test_order(100, 1, Side::Buy, 60000, 1);
        let old_token = mgr.check_and_reserve(&Command::NewOrder(order)).unwrap();

        assert_eq!(mgr.get_account(100).unwrap().tentative_reserved, 60000);

        // Replace with lower price (frees up funds)
        let replace = make_test_replace(100, 1, 1, 40000, 1);
        let new_token = mgr.adjust_reservation(&old_token, &replace).unwrap();

        // Verify adjusted reservation (should be lower)
        let account = mgr.get_account(100).unwrap();
        assert_eq!(account.tentative_reserved, 40000);
        assert_eq!(new_token.amount, 40000);
    }

    #[test]
    fn test_persistence_journal_roundtrip() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let journal_path = dir.path().join("test_journal.bin");

        // Create manager with journal
        let journal = crate::persistence::AccountJournal::open(&journal_path).unwrap();
        let mgr = AccountManager::with_journal(journal);

        // Create some accounts
        mgr.create_account(1, 100000);
        mgr.create_account(2, 200000);

        // Flush to disk
        mgr.flush_journal().unwrap();

        // Create new manager and replay
        let mut journal2 = crate::persistence::AccountJournal::open(&journal_path).unwrap();
        let updates = journal2.read_all().unwrap();

        assert_eq!(updates.len(), 2);

        let mgr2 = AccountManager::new();
        mgr2.replay_journal(updates);

        // Verify accounts were restored
        let acc1 = mgr2.get_account(1).unwrap();
        assert_eq!(acc1.buying_power, 100000);

        let acc2 = mgr2.get_account(2).unwrap();
        assert_eq!(acc2.buying_power, 200000);
    }

    #[test]
    fn test_persistence_snapshot_roundtrip() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let snapshot_dir = dir.path().join("snapshots");

        let mgr = AccountManager::new();

        // Create some accounts with positions
        mgr.create_account(1, 100000);
        mgr.create_account(2, 200000);

        // Create snapshot
        let snapshot = mgr.create_snapshot(42);
        assert_eq!(snapshot.sequence, 42);
        assert_eq!(snapshot.accounts.len(), 2);

        // Save and load
        snapshot.save(&snapshot_dir).unwrap();
        let loaded = crate::persistence::AccountSnapshot::load_latest(&snapshot_dir)
            .unwrap()
            .unwrap();

        assert_eq!(loaded.sequence, 42);
        assert_eq!(loaded.accounts.len(), 2);

        // Restore to new manager
        let mgr2 = AccountManager::new();
        mgr2.restore_from_snapshot(&loaded);

        let acc1 = mgr2.get_account(1).unwrap();
        assert_eq!(acc1.buying_power, 100000);

        let acc2 = mgr2.get_account(2).unwrap();
        assert_eq!(acc2.buying_power, 200000);
    }

    #[test]
    fn test_persistence_fill_replay() {
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let journal_path = dir.path().join("test_journal.bin");

        let journal = crate::persistence::AccountJournal::open(&journal_path).unwrap();
        let mgr = AccountManager::with_journal(journal);

        // Create account
        mgr.create_account(1, 100000);

        // NOTE: Fills are no longer journaled - engines are source of truth
        // This test now only verifies account creation persistence

        mgr.flush_journal().unwrap();

        // Replay in new manager
        let mut journal2 = crate::persistence::AccountJournal::open(&journal_path).unwrap();
        let updates = journal2.read_all().unwrap();

        let mgr2 = AccountManager::new();
        mgr2.replay_journal(updates);

        // Verify account was persisted and replayed
        let acc = mgr2.get_account(1).unwrap();
        assert_eq!(acc.buying_power, 100000);
    }

    #[test]
    fn test_multiple_orders_with_cancel() {
        let mgr = AccountManager::new();
        mgr.create_account(100, 100000);

        // Place two orders
        let order1 = make_test_order(100, 1, Side::Buy, 30000, 1);
        let token1 = mgr.check_and_reserve(&Command::NewOrder(order1)).unwrap();

        let order2 = NewOrder {
            order_id: 2,
            ..make_test_order(100, 1, Side::Buy, 30000, 1)
        };
        let token2 = mgr.check_and_reserve(&Command::NewOrder(order2)).unwrap();

        // Both reservations should exist
        let account = mgr.get_account(100).unwrap();
        assert_eq!(account.tentative_reserved, 60000);

        // Cancel first order
        mgr.release_reservation(&token1);

        // Only second reservation remains
        let account = mgr.get_account(100).unwrap();
        assert_eq!(account.tentative_reserved, 30000);

        // Should be able to place a new 30k order now
        let order3 = NewOrder {
            order_id: 3,
            ..make_test_order(100, 1, Side::Buy, 30000, 1)
        };
        let result = mgr.check_and_reserve(&Command::NewOrder(order3));
        assert!(result.is_ok());

        let account = mgr.get_account(100).unwrap();
        assert_eq!(account.tentative_reserved, 60000);

        // Cancel both remaining orders
        mgr.release_reservation(&token2);
        mgr.release_reservation(&result.unwrap());

        let account = mgr.get_account(100).unwrap();
        assert_eq!(account.tentative_reserved, 0);
    }
}

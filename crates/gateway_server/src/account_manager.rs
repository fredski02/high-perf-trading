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
use std::collections::HashMap;

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
}

impl AccountManager {
    pub fn new() -> Self {
        Self {
            accounts: DashMap::new(),
            next_gateway_seq: std::sync::atomic::AtomicU64::new(1),
        }
    }

    /// Create or fund an account
    pub fn create_account(&self, account_id: AccountId, buying_power: i64) {
        self.accounts
            .insert(account_id, AccountState::new(account_id, buying_power));
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
            // Other commands don't need reservations
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

    /// Get account state for querying
    pub fn get_account(&self, account_id: AccountId) -> Option<AccountState> {
        self.accounts.get(&account_id).map(|r| r.value().clone())
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
}
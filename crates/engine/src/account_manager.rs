use std::collections::HashMap;

use common::{AccountId, Position, RejectReason, RiskLimits, Side};
use serde::{Deserialize, Serialize};

/// Manages account positions and risk limits
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountManager {
    /// Current positions per account
    positions: HashMap<AccountId, Position>,
    
    /// Risk limits per account (defaults used if not set)
    risk_limits: HashMap<AccountId, RiskLimits>,
    
    /// Default limits for accounts without explicit limits
    default_limits: RiskLimits,
}

impl AccountManager {
    /// Create a new AccountManager with default risk limits
    pub fn new(default_limits: RiskLimits) -> Self {
        Self {
            positions: HashMap::new(),
            risk_limits: HashMap::new(),
            default_limits,
        }
    }

    /// Check if an order would violate risk limits
    /// Returns Ok(()) if order is allowed, Err(RejectReason::Risk) if not
    pub fn check_risk(
        &self,
        account_id: AccountId,
        side: Side,
        qty: i64,
    ) -> Result<(), RejectReason> {
        let limits = self.get_limits(account_id);
        
        // Check single order size
        if qty > limits.max_order_size {
            return Err(RejectReason::Risk);
        }
        
        // Get current position
        let current_pos = self.get_position(account_id).net_position;
        
        // Calculate hypothetical new position if order fully fills
        let new_position = match side {
            Side::Buy => current_pos + qty,
            Side::Sell => current_pos - qty,
        };
        
        // Check position limits
        if new_position > limits.max_long_position {
            return Err(RejectReason::Risk);
        }
        
        if new_position < -limits.max_short_position {
            return Err(RejectReason::Risk);
        }
        
        Ok(())
    }

    /// Update position after a fill
    /// This handles both position accumulation and P&L calculation
    pub fn apply_fill(
        &mut self,
        account_id: AccountId,
        side: Side,
        price: i64,
        qty: i64,
    ) {
        let pos = self.positions.entry(account_id).or_default();
        
        match side {
            Side::Buy => {
                // Buying: increases position
                if pos.net_position < 0 {
                    // Closing short: realize P&L
                    let closing_qty = qty.min(-pos.net_position);
                    let pnl_per_lot = pos.avg_price - price;
                    pos.realized_pnl += closing_qty * pnl_per_lot;
                    
                    // Remaining qty opens new long
                    let opening_qty = qty - closing_qty;
                    if opening_qty > 0 {
                        pos.avg_price = price;
                    }
                } else {
                    // Opening or adding to long: update weighted avg
                    if pos.net_position == 0 {
                        pos.avg_price = price;
                    } else {
                        let total_value = pos.net_position * pos.avg_price + qty * price;
                        pos.avg_price = total_value / (pos.net_position + qty);
                    }
                }
                pos.net_position += qty;
            }
            Side::Sell => {
                // Selling: decreases position
                if pos.net_position > 0 {
                    // Closing long: realize P&L
                    let closing_qty = qty.min(pos.net_position);
                    let pnl_per_lot = price - pos.avg_price;
                    pos.realized_pnl += closing_qty * pnl_per_lot;
                    
                    // Remaining qty opens new short
                    let opening_qty = qty - closing_qty;
                    if opening_qty > 0 {
                        pos.avg_price = price;
                    }
                } else {
                    // Opening or adding to short: update weighted avg
                    if pos.net_position == 0 {
                        pos.avg_price = price;
                    } else {
                        let total_value = (-pos.net_position) * pos.avg_price + qty * price;
                        pos.avg_price = total_value / (-pos.net_position + qty);
                    }
                }
                pos.net_position -= qty;
            }
        }
    }

    /// Get position for an account (returns default if not found)
    pub fn get_position(&self, account_id: AccountId) -> Position {
        self.positions.get(&account_id).copied().unwrap_or_default()
    }

    /// Get risk limits for an account (returns default if not set)
    pub fn get_limits(&self, account_id: AccountId) -> RiskLimits {
        self.risk_limits
            .get(&account_id)
            .copied()
            .unwrap_or(self.default_limits)
    }

    /// Set custom risk limits for an account
    pub fn set_limits(&mut self, account_id: AccountId, limits: RiskLimits) {
        self.risk_limits.insert(account_id, limits);
    }

    /// Get all account IDs with positions
    pub fn accounts_with_positions(&self) -> Vec<AccountId> {
        self.positions
            .iter()
            .filter(|(_, pos)| pos.net_position != 0)
            .map(|(id, _)| *id)
            .collect()
    }

    /// Get number of accounts being tracked
    pub fn account_count(&self) -> usize {
        self.positions.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_limits() -> RiskLimits {
        RiskLimits {
            max_long_position: 100,
            max_short_position: 100,
            max_order_size: 50,
        }
    }

    #[test]
    fn test_risk_check_order_size() {
        let mgr = AccountManager::new(default_limits());
        
        // Within limit
        assert!(mgr.check_risk(1, Side::Buy, 50).is_ok());
        
        // Exceeds limit
        assert_eq!(
            mgr.check_risk(1, Side::Buy, 51),
            Err(RejectReason::Risk)
        );
    }

    #[test]
    fn test_risk_check_position_limit() {
        let mut mgr = AccountManager::new(default_limits());
        
        // First order: buy 50 (at max order size, position=50)
        assert!(mgr.check_risk(1, Side::Buy, 50).is_ok());
        mgr.apply_fill(1, Side::Buy, 100, 50);
        
        // Second order: buy 40 more would be 90 total (OK, still under max_long=100)
        assert!(mgr.check_risk(1, Side::Buy, 40).is_ok());
        mgr.apply_fill(1, Side::Buy, 100, 40);
        
        // Position is now 90. Third order: buy 20 more would be 110 (exceeds max_long=100)
        assert_eq!(
            mgr.check_risk(1, Side::Buy, 20),
            Err(RejectReason::Risk)
        );
    }

    #[test]
    fn test_position_tracking_long() {
        let mut mgr = AccountManager::new(default_limits());
        
        // Buy 10 @ 100
        mgr.apply_fill(1, Side::Buy, 100, 10);
        let pos = mgr.get_position(1);
        assert_eq!(pos.net_position, 10);
        assert_eq!(pos.avg_price, 100);
        assert_eq!(pos.realized_pnl, 0);
        
        // Buy 10 more @ 110
        mgr.apply_fill(1, Side::Buy, 110, 10);
        let pos = mgr.get_position(1);
        assert_eq!(pos.net_position, 20);
        assert_eq!(pos.avg_price, 105); // (10*100 + 10*110) / 20
    }

    #[test]
    fn test_position_tracking_close_long() {
        let mut mgr = AccountManager::new(default_limits());
        
        // Buy 10 @ 100
        mgr.apply_fill(1, Side::Buy, 100, 10);
        
        // Sell 5 @ 110 (close half, realize 50 P&L)
        mgr.apply_fill(1, Side::Sell, 110, 5);
        let pos = mgr.get_position(1);
        assert_eq!(pos.net_position, 5);
        assert_eq!(pos.realized_pnl, 50); // 5 * (110 - 100)
    }

    #[test]
    fn test_position_tracking_reverse() {
        let mut mgr = AccountManager::new(default_limits());
        
        // Buy 10 @ 100
        mgr.apply_fill(1, Side::Buy, 100, 10);
        
        // Sell 15 @ 110 (close 10 long, open 5 short)
        mgr.apply_fill(1, Side::Sell, 110, 15);
        let pos = mgr.get_position(1);
        assert_eq!(pos.net_position, -5); // Short 5
        assert_eq!(pos.realized_pnl, 100); // 10 * (110 - 100)
        assert_eq!(pos.avg_price, 110); // Short entry at 110
    }

    #[test]
    fn test_custom_limits() {
        let mut mgr = AccountManager::new(default_limits());
        
        // Set tighter limits for account 1
        mgr.set_limits(1, RiskLimits {
            max_long_position: 10,
            max_short_position: 10,
            max_order_size: 5,
        });
        
        // Account 1: order of 6 rejected
        assert_eq!(mgr.check_risk(1, Side::Buy, 6), Err(RejectReason::Risk));
        
        // Account 2: still uses default (50)
        assert!(mgr.check_risk(2, Side::Buy, 50).is_ok());
    }
}
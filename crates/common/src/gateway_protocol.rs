//! Gateway ↔ Engine Protocol
//!
//! This module defines the protocol between the gateway server and engine servers.
//! The gateway wraps client commands with risk approval metadata before forwarding to engines.

use crate::{AccountId, Command, Event, OrderId, SymbolId};
use serde::{Deserialize, Serialize};

/// Commands sent from gateway to engine (risk-approved orders)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GatewayToEngine {
    /// Execute a command (already risk-approved by gateway)
    Execute(ExecuteCommand),

    /// Health check / ping
    Ping,

    /// Query all active orders from this engine (for reconciliation)
    QueryAllOrders,
}

/// Command with risk approval metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteCommand {
    /// The original command from the client
    pub command: Command,

    /// Connection ID for routing responses back to client
    pub conn_id: u64,

    /// Risk approval token (proves gateway checked risk)
    pub risk_token: RiskToken,
}

/// Token proving the gateway has approved this order from a risk perspective
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskToken {
    /// Account ID (for verification)
    pub account_id: AccountId,

    /// Amount of buying power reserved for this order
    pub reserved_amount: i64,

    /// Sequence number from gateway (for idempotency)
    pub gateway_seq: u64,
}

/// Snapshot of a single order (for reconciliation)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderSnapshot {
    pub order_id: OrderId,
    pub account_id: AccountId,
    pub symbol_id: SymbolId,
    pub side: crate::Side,
    pub price: crate::Price,
    pub qty_rem: crate::Qty,
    /// Amount of buying power reserved for this order
    pub reserved_amount: i64,
}

/// Events sent from engine back to gateway
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EngineToGateway {
    /// An event to be forwarded to a client
    ClientEvent {
        /// Connection ID to route to
        conn_id: u64,

        /// The event (Fill, Ack, Reject, etc.)
        event: Event,

        /// Risk token (for releasing reservations)
        risk_token: Option<RiskToken>,
    },

    /// Engine health status
    Pong {
        symbol_id: SymbolId,
        orders_in_book: usize,
    },

    /// Market data broadcast (BookTop, Trade)
    MarketData { symbol_id: SymbolId, event: Event },

    /// Response to QueryAllOrders - all active orders in the engine
    AllOrders(Vec<OrderSnapshot>),

    /// Sent after engine restarts and recovers from persistence
    EngineReady {
        symbol_id: SymbolId,
        orders: Vec<OrderSnapshot>,
    },
}

impl GatewayToEngine {
    /// Create a new execute command with risk approval
    pub fn execute(command: Command, conn_id: u64, risk_token: RiskToken) -> Self {
        Self::Execute(ExecuteCommand {
            command,
            conn_id,
            risk_token,
        })
    }
}

impl EngineToGateway {
    /// Create a client event to be routed back to the client
    pub fn client_event(conn_id: u64, event: Event, risk_token: Option<RiskToken>) -> Self {
        Self::ClientEvent {
            conn_id,
            event,
            risk_token,
        }
    }

    /// Create a market data event
    pub fn market_data(symbol_id: SymbolId, event: Event) -> Self {
        Self::MarketData { symbol_id, event }
    }
}

/// Helper to extract symbol_id from a command
/// Returns None for Authenticate command (not routed to engines)
pub fn command_symbol_id(cmd: &Command) -> Option<SymbolId> {
    match cmd {
        Command::NewOrder(o) => Some(o.symbol_id),
        Command::Cancel(c) => Some(c.symbol_id),
        Command::Replace(r) => Some(r.symbol_id),
        Command::SetRiskLimits(s) => Some(s.symbol_id),
        Command::QueryAccount(q) => Some(q.symbol_id),
        Command::Authenticate(_) => None,  // Auth doesn't have symbol_id
    }
}

/// Helper to extract account_id from a command
/// Returns None for Authenticate command (account_id determined after auth)
pub fn command_account_id(cmd: &Command) -> Option<AccountId> {
    match cmd {
        Command::NewOrder(o) => Some(o.account_id),
        Command::Cancel(c) => Some(c.account_id),
        Command::Replace(r) => Some(r.account_id),
        Command::SetRiskLimits(s) => Some(s.account_id),
        Command::QueryAccount(q) => Some(q.account_id),
        Command::Authenticate(_) => None,  // Auth doesn't have account_id yet
    }
}

/// Helper to extract order_id from a command (if applicable)
pub fn command_order_id(cmd: &Command) -> Option<OrderId> {
    match cmd {
        Command::NewOrder(o) => Some(o.order_id),
        Command::Cancel(c) => Some(c.order_id),
        Command::Replace(r) => Some(r.order_id),
        Command::SetRiskLimits(_) | Command::QueryAccount(_) | Command::Authenticate(_) => None,
    }
}
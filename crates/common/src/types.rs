use serde::{Deserialize, Serialize};

pub type OrderId = u64;
pub type AccountId = u32;
pub type SymbolId = u32;

pub type Price = i64; // ticks
pub type Qty = i64; // lots

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    /// Get the opposite side
    pub fn opposite(self) -> Self {
        match self {
            Side::Buy => Side::Sell,
            Side::Sell => Side::Buy,
        }
    }
}

// ============ Risk Management Types ============

/// Account position state (tracked per account per symbol)
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Position {
    /// Net position: +100 = long 100, -50 = short 50
    pub net_position: i64,
    /// Volume-weighted average entry price (in ticks)
    pub avg_price: i64,
    /// Realized profit/loss (in ticks)
    pub realized_pnl: i64,
}

/// Risk limits per account
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct RiskLimits {
    /// Maximum long position (e.g., +1000 lots)
    pub max_long_position: i64,
    /// Maximum short position magnitude (e.g., 1000 for -1000 lots)
    pub max_short_position: i64,
    /// Maximum single order size
    pub max_order_size: i64,
}

impl Default for RiskLimits {
    fn default() -> Self {
        Self {
            max_long_position: 10_000,
            max_short_position: 10_000,
            max_order_size: 1_000,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TimeInForce {
    Gtc,
    Ioc,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct OrderFlags {
    pub post_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Command {
    NewOrder(NewOrder),
    Cancel(Cancel),
    Replace(Replace),
    SetRiskLimits(SetRiskLimits),
    QueryAccount(QueryAccount),
    Authenticate(Authenticate),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Authenticate {
    pub api_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Copy)]
pub struct NewOrder {
    pub client_seq: u64,
    pub order_id: OrderId,
    pub account_id: AccountId,
    pub symbol_id: SymbolId,
    pub side: Side,
    pub price: Price,
    pub qty: Qty,
    pub tif: TimeInForce,
    pub flags: OrderFlags,
}

#[derive(Debug, Clone, Serialize, Deserialize, Copy)]
pub struct Cancel {
    pub client_seq: u64,
    pub order_id: OrderId,
    pub account_id: AccountId,
    pub symbol_id: SymbolId,
}

#[derive(Debug, Clone, Serialize, Deserialize, Copy)]
pub struct Replace {
    pub client_seq: u64,
    pub order_id: OrderId,
    pub account_id: AccountId,
    pub symbol_id: SymbolId,
    pub new_price: Price,
    pub new_qty: Qty,
}

#[derive(Debug, Clone, Serialize, Deserialize, Copy)]
pub struct SetRiskLimits {
    pub client_seq: u64,
    pub account_id: AccountId,
    pub symbol_id: SymbolId,
    pub limits: RiskLimits,
}

#[derive(Debug, Clone, Serialize, Deserialize, Copy)]
pub struct QueryAccount {
    pub client_seq: u64,
    pub account_id: AccountId,
    pub symbol_id: SymbolId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Event {
    Ack(Ack),
    Reject(Reject),
    Fill(Fill),
    BookTop(BookTop),
    AccountState(AccountState),
    AuthSuccess(AuthSuccess),
    AuthFailure(AuthFailure),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthSuccess {
    pub account_id: AccountId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthFailure {
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Ack {
    pub server_seq: u64,
    pub client_seq: u64,
    pub order_id: OrderId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reject {
    pub server_seq: u64,
    pub client_seq: u64,
    pub order_id: Option<OrderId>,
    pub reason: RejectReason,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RejectReason {
    Invalid,
    Risk,
    Overloaded,
    NotFound,
    PostOnlyWouldCross,
    RateLimitExceeded,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fill {
    pub server_seq: u64,
    pub client_seq: u64,
    pub symbol_id: SymbolId,
    pub taker_order_id: OrderId,
    pub maker_order_id: OrderId,
    pub price: Price,
    pub qty: Qty,
}

#[derive(Debug, Clone, Serialize, Deserialize, Copy)]
pub struct BookTop {
    pub server_seq: u64,
    pub symbol_id: SymbolId,
    pub best_bid_px: Option<Price>,
    pub best_bid_qty: Option<Qty>,
    pub best_ask_px: Option<Price>,
    pub best_ask_qty: Option<Qty>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Copy)]
pub struct AccountState {
    pub server_seq: u64,
    pub client_seq: u64,
    pub account_id: AccountId,
    pub symbol_id: SymbolId,
    pub position: Position,
    pub risk_limits: RiskLimits,
}
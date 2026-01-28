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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum TimeInForce {
    Gtc,
    Ioc,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct OrderFlags {
    pub post_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Copy)]
pub enum Command {
    NewOrder(NewOrder),
    Cancel(Cancel),
    Replace(Replace),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Event {
    Ack(Ack),
    Reject(Reject),
    Fill(Fill),
    BookTop(BookTop),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RejectReason {
    Invalid,
    Risk,
    Overloaded,
    NotFound,
    PostOnlyWouldCross,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookTop {
    pub server_seq: u64,
    pub symbol_id: SymbolId,
    pub best_bid_px: Option<Price>,
    pub best_bid_qty: Option<Qty>,
    pub best_ask_px: Option<Price>,
    pub best_ask_qty: Option<Qty>,
}

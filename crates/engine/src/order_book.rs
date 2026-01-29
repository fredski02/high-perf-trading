use std::collections::{BTreeMap, HashMap, VecDeque};

use slab::Slab;
use serde::{Deserialize, Serialize};

use common::{AccountId, OrderFlags, OrderId, Price, Qty, Side, SymbolId, TimeInForce};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    pub order_id: OrderId,
    pub account_id: AccountId,
    pub symbol_id: SymbolId,
    pub side: Side,
    pub price: Price,
    pub qty_rem: Qty,
    pub tif: TimeInForce,
    pub flags: OrderFlags,
}

#[derive(Debug)]
struct Level {
    queue: VecDeque<usize>, // slab keys (maker FIFO)
    total_qty: Qty,         // aggregate qty at this level
}

impl Level {
    fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            total_qty: 0,
        }
    }
}

#[derive(Debug)]
pub struct OrderBook {
    pub symbol_id: SymbolId,

    // price -> level
    bids: BTreeMap<Price, Level>,
    asks: BTreeMap<Price, Level>,

    // slab storage
    orders: Slab<Order>,

    // order_id -> slab key
    index: HashMap<OrderId, usize>,
}

#[derive(Debug, Clone, Copy)]
pub struct MatchFill {
    pub maker_order_id: OrderId,
    pub taker_order_id: OrderId,
    pub price: Price,
    pub qty: Qty,
}

impl OrderBook {
    pub fn new(symbol_id: SymbolId) -> Self {
        Self {
            symbol_id,
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            orders: Slab::new(),
            index: HashMap::new(),
        }
    }
    pub fn get(&self, order_id: OrderId) -> Option<&Order> {
        let key = *self.index.get(&order_id)?;
        self.orders.get(key)
    }

    pub fn best_bid(&self) -> Option<(Price, Qty)> {
        self.bids
            .iter()
            .next_back()
            .map(|(px, lvl)| (*px, lvl.total_qty))
    }

    pub fn best_ask(&self) -> Option<(Price, Qty)> {
        self.asks
            .iter()
            .next()
            .map(|(px, lvl)| (*px, lvl.total_qty))
    }

    pub fn would_cross(&self, side: Side, px: Price) -> bool {
        match side {
            Side::Buy => self.best_ask().map(|(ask, _)| px >= ask).unwrap_or(false),
            Side::Sell => self.best_bid().map(|(bid, _)| px <= bid).unwrap_or(false),
        }
    }

    pub fn insert_resting(&mut self, order: Order) {
        let key = self.orders.insert(order.clone());
        self.index.insert(order.order_id, key);

        let book = match order.side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        };

        let lvl = book.entry(order.price).or_insert_with(Level::new);
        lvl.total_qty += order.qty_rem;
        lvl.queue.push_back(key);
    }

    /// Cancel an order. Returns true if found+owned+cancelled.
    pub fn cancel(&mut self, order_id: OrderId, account_id: AccountId) -> bool {
        let Some(&key) = self.index.get(&order_id) else {
            return false;
        };
        let Some(ord) = self.orders.get(key) else {
            return false;
        };
        if ord.account_id != account_id {
            return false;
        }

        let side = ord.side;
        let price = ord.price;
        let qty = ord.qty_rem;

        // Mark dead + remove from id index.
        self.index.remove(&order_id);
        if let Some(o) = self.orders.get_mut(key) {
            o.qty_rem = 0;
        }

        let book = match side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        };

        if let Some(lvl) = book.get_mut(&price) {
            lvl.total_qty -= qty;
            // Remove empty level eagerly (queue might still contain cancelled ids; safe).
            if lvl.total_qty <= 0 {
                book.remove(&price);
            }
        }

        true
    }

    /// Replace = cancel + insert_resting with same order_id.
    /// Returns false if original order not found/owned.
    pub fn replace(
        &mut self,
        order_id: OrderId,
        account_id: AccountId,
        new_price: Price,
        new_qty: Qty,
    ) -> bool {
        let Some(&key) = self.index.get(&order_id) else {
            return false;
        };
        let Some(existing) = self.orders.get(key).cloned() else {
            return false;
        };
        if existing.account_id != account_id {
            return false;
        }

        let _ = self.cancel(order_id, account_id);

        let mut ord = existing;
        ord.price = new_price;
        ord.qty_rem = new_qty;
        self.insert_resting(ord);
        true
    }

    /// Match taker against opposite book. Returns fills. (Remaining qty is computed by caller.)
    pub fn match_taker(
        &mut self,
        taker_order_id: OrderId,
        taker_side: Side,
        taker_price: Price,
        mut taker_qty: Qty,
    ) -> Vec<MatchFill> {
        let mut fills = Vec::new();

        match taker_side {
            Side::Buy => {
                while taker_qty > 0 {
                    let best_ask_px = match self.asks.keys().next().cloned() {
                        Some(px) => px,
                        None => break,
                    };
                    if best_ask_px > taker_price {
                        break;
                    }

                    // We need to pop makers from this level FIFO.
                    let remove_level = {
                        let lvl = self.asks.get_mut(&best_ask_px).expect("level exists");
                        let maker_key_opt = Self::pop_live_front(&self.orders, lvl);

                        match maker_key_opt {
                            None => true, // level empty
                            Some(maker_key) => {
                                let maker_order_id;
                                let trade_qty;

                                {
                                    let maker =
                                        self.orders.get_mut(maker_key).expect("maker exists");
                                    maker_order_id = maker.order_id;
                                    trade_qty = taker_qty.min(maker.qty_rem);
                                    maker.qty_rem -= trade_qty;
                                }

                                taker_qty -= trade_qty;
                                lvl.total_qty -= trade_qty;

                                fills.push(MatchFill {
                                    maker_order_id,
                                    taker_order_id,
                                    price: best_ask_px,
                                    qty: trade_qty,
                                });

                                // If maker filled, remove it completely.
                                let maker_done = self
                                    .orders
                                    .get(maker_key)
                                    .map(|o| o.qty_rem == 0)
                                    .unwrap_or(true);
                                if maker_done {
                                    self.index.remove(&maker_order_id);
                                    self.orders.remove(maker_key);
                                    // already popped from queue by pop_live_front
                                } else {
                                    // maker still has qty; put back to FRONT (it remains first in FIFO)
                                    lvl.queue.push_front(maker_key);
                                }

                                lvl.total_qty <= 0
                            }
                        }
                    };

                    if remove_level {
                        self.asks.remove(&best_ask_px);
                    }
                }
            }
            Side::Sell => {
                while taker_qty > 0 {
                    let best_bid_px = match self.bids.keys().next_back().cloned() {
                        Some(px) => px,
                        None => break,
                    };
                    if best_bid_px < taker_price {
                        break;
                    }

                    let remove_level = {
                        let lvl = self.bids.get_mut(&best_bid_px).expect("level exists");
                        let maker_key_opt = Self::pop_live_front(&self.orders, lvl);

                        match maker_key_opt {
                            None => true,
                            Some(maker_key) => {
                                let maker_order_id;
                                let trade_qty;

                                {
                                    let maker =
                                        self.orders.get_mut(maker_key).expect("maker exists");
                                    maker_order_id = maker.order_id;
                                    trade_qty = taker_qty.min(maker.qty_rem);
                                    maker.qty_rem -= trade_qty;
                                }

                                taker_qty -= trade_qty;
                                lvl.total_qty -= trade_qty;

                                fills.push(MatchFill {
                                    maker_order_id,
                                    taker_order_id,
                                    price: best_bid_px,
                                    qty: trade_qty,
                                });

                                let maker_done = self
                                    .orders
                                    .get(maker_key)
                                    .map(|o| o.qty_rem == 0)
                                    .unwrap_or(true);
                                if maker_done {
                                    self.index.remove(&maker_order_id);
                                    self.orders.remove(maker_key);
                                } else {
                                    lvl.queue.push_front(maker_key);
                                }

                                lvl.total_qty <= 0
                            }
                        }
                    };

                    if remove_level {
                        self.bids.remove(&best_bid_px);
                    }
                }
            }
        }

        fills
    }

    /// Pops cancelled/dead orders until we find a live one.
    /// Returns Some(slab_key) (already removed from queue), or None if empty.
    fn pop_live_front(orders: &Slab<Order>, lvl: &mut Level) -> Option<usize> {
        while let Some(k) = lvl.queue.pop_front() {
            let qty_rem = orders.get(k).map(|o| o.qty_rem).unwrap_or(0);
            if qty_rem > 0 {
                return Some(k);
            }
            // else skip cancelled/removed
        }
        None
    }

    /// Count live orders in the book
    pub fn live_order_count(&self) -> usize {
        self.index.len()
    }

    /// Serialize order book state to bytes for snapshotting
    pub fn serialize_snapshot(&self) -> anyhow::Result<Vec<u8>> {
        // Collect all live orders
        let live_orders: Vec<Order> = self.orders
            .iter()
            .filter(|(_, o)| o.qty_rem > 0)
            .map(|(_, o)| o.clone())
            .collect();

        postcard::to_stdvec(&live_orders)
            .map_err(|e| anyhow::anyhow!("snapshot serialize failed: {}", e))
    }

    /// Restore order book from snapshot bytes
    pub fn restore_from_snapshot(&mut self, data: &[u8]) -> anyhow::Result<()> {
        let orders: Vec<Order> = postcard::from_bytes(data)
            .map_err(|e| anyhow::anyhow!("snapshot deserialize failed: {}", e))?;

        // Clear existing state
        self.bids.clear();
        self.asks.clear();
        self.orders.clear();
        self.index.clear();

        // Re-insert all orders
        for order in orders {
            self.insert_resting(order);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::Side;

    fn ob() -> OrderBook {
        OrderBook::new(1)
    }

    #[test]
    fn resting_orders_set_top() {
        let mut b = ob();
        b.insert_resting(Order {
            order_id: 1,
            account_id: 10,
            symbol_id: 1,
            side: Side::Buy,
            price: 99,
            qty_rem: 5,
            flags: OrderFlags { post_only: false },
            tif: TimeInForce::Gtc,
        });
        b.insert_resting(Order {
            order_id: 2,
            account_id: 11,
            symbol_id: 1,
            side: Side::Sell,
            price: 101,
            qty_rem: 7,
            flags: OrderFlags { post_only: false },
            tif: TimeInForce::Gtc,
        });

        assert_eq!(b.best_bid(), Some((99, 5)));
        assert_eq!(b.best_ask(), Some((101, 7)));
        assert!(!b.would_cross(Side::Buy, 100));
        assert!(b.would_cross(Side::Buy, 101));
    }

    #[test]
    fn match_buy_takes_best_ask_fifo() {
        let mut b = ob();

        // two asks at same price, FIFO by insertion
        b.insert_resting(Order {
            order_id: 10,
            account_id: 1,
            symbol_id: 1,
            side: Side::Sell,
            price: 100,
            qty_rem: 3,
            flags: OrderFlags { post_only: false },
            tif: TimeInForce::Gtc,
        });
        b.insert_resting(Order {
            order_id: 11,
            account_id: 1,
            symbol_id: 1,
            side: Side::Sell,
            price: 100,
            qty_rem: 4,
            flags: OrderFlags { post_only: false },
            tif: TimeInForce::Gtc,
        });

        let fills = b.match_taker(200, Side::Buy, 100, 5);

        assert_eq!(fills.len(), 2);
        assert_eq!(fills[0].maker_order_id, 10);
        assert_eq!(fills[0].qty, 3);
        assert_eq!(fills[1].maker_order_id, 11);
        assert_eq!(fills[1].qty, 2);

        // remaining at best ask should be 2 (from order 11)
        assert_eq!(b.best_ask(), Some((100, 2)));
    }

    #[test]
    fn cancel_removes_qty_from_top() {
        let mut b = ob();

        b.insert_resting(Order {
            order_id: 10,
            account_id: 1,
            symbol_id: 1,
            side: Side::Sell,
            price: 100,
            qty_rem: 3,
            flags: OrderFlags { post_only: false },
            tif: TimeInForce::Gtc,
        });
        b.insert_resting(Order {
            order_id: 11,
            account_id: 2,
            symbol_id: 1,
            side: Side::Sell,
            price: 100,
            qty_rem: 4,
            flags: OrderFlags { post_only: false },
            tif: TimeInForce::Gtc,
        });

        assert_eq!(b.best_ask(), Some((100, 7)));

        assert!(b.cancel(10, 1));
        assert_eq!(b.best_ask(), Some((100, 4)));

        // wrong account can't cancel
        assert!(!b.cancel(11, 99));
        assert_eq!(b.best_ask(), Some((100, 4)));
    }

    #[test]
    fn match_sell_takes_best_bid() {
        let mut b = ob();

        b.insert_resting(Order {
            order_id: 1,
            account_id: 1,
            symbol_id: 1,
            side: Side::Buy,
            price: 101,
            qty_rem: 2,
            flags: OrderFlags { post_only: false },
            tif: TimeInForce::Gtc,
        });
        b.insert_resting(Order {
            order_id: 2,
            account_id: 1,
            symbol_id: 1,
            side: Side::Buy,
            price: 100,
            qty_rem: 10,
            flags: OrderFlags { post_only: false },
            tif: TimeInForce::Gtc,
        });

        let fills = b.match_taker(9, Side::Sell, 100, 3);

        assert_eq!(fills.len(), 2);
        assert_eq!(fills[0].price, 101);
        assert_eq!(fills[0].qty, 2);
        assert_eq!(fills[1].price, 100);
        assert_eq!(fills[1].qty, 1);

        assert_eq!(b.best_bid(), Some((100, 9)));
    }
}
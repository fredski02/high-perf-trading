use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender};
use tracing::{info, warn};

use crate::order_book::{MatchFill, Order, OrderBook};
use crate::{Inbound, Outbound};
use admin_http::metrics::Metrics;
use common::{
    Ack, BookTop, Command, Event, Fill, NewOrder, Reject, RejectReason, Side, TimeInForce,
};
pub struct Engine {
    rx: Receiver<Inbound>,
    tx: Sender<Outbound>,
    server_seq: u64,

    // v1: single symbol
    book: OrderBook,
    metrics: Arc<Metrics>,
}

impl Engine {
    pub fn new(rx: Receiver<Inbound>, tx: Sender<Outbound>, metrics: Arc<Metrics>) -> Self {
        Self {
            rx,
            tx,
            server_seq: 1,
            book: OrderBook::new(1),
            metrics,
        }
    }

    pub fn run(mut self) {
        info!("engine: starting");

        while let Ok(inb) = self.rx.recv() {
            self.metrics.queue_dec();
            let conn_id = inb.conn_id;

            let events = self.process(inb.cmd);

            for ev in events {
                if self.tx.send(Outbound { conn_id, ev }).is_err() {
                    warn!("engine: outbound channel closed");
                    return;
                }
            }
        }
    }

    /// Process a command into 0..N events.
    pub fn process(&mut self, cmd: Command) -> Vec<Event> {
        match cmd {
            Command::NewOrder(no) => self.handle_new(no),

            Command::Cancel(c) => {
                let mut evs = Vec::new();

                if c.symbol_id != self.book.symbol_id {
                    evs.push(self.reject(c.client_seq, Some(c.order_id), RejectReason::Invalid));
                    return evs;
                }

                let ok = self.book.cancel(c.order_id, c.account_id);
                if !ok {
                    evs.push(self.reject(c.client_seq, Some(c.order_id), RejectReason::NotFound));
                    return evs;
                }

                // Emit book snapshot then ack (so read-until-ack captures both)
                evs.push(self.book_top_event());
                evs.push(self.ack(c.client_seq, c.order_id));
                evs
            }

            Command::Replace(r) => {
                let mut evs = Vec::new();

                if r.symbol_id != self.book.symbol_id || r.new_qty <= 0 {
                    evs.push(self.reject(r.client_seq, Some(r.order_id), RejectReason::Invalid));
                    return evs;
                }

                // infer side/tif/flags from existing resting order
                let (side, tif, flags) = match self.book.get(r.order_id) {
                    Some(o) if o.account_id == r.account_id => (o.side, o.tif, o.flags),
                    _ => {
                        evs.push(self.reject(
                            r.client_seq,
                            Some(r.order_id),
                            RejectReason::NotFound,
                        ));
                        return evs;
                    }
                };

                // remove existing order first
                let _ = self.book.cancel(r.order_id, r.account_id);

                // now treat as a new order (same ids, inferred side)
                let no = common::NewOrder {
                    client_seq: r.client_seq,
                    order_id: r.order_id,
                    account_id: r.account_id,
                    symbol_id: r.symbol_id,
                    side,
                    price: r.new_price,
                    qty: r.new_qty,
                    tif,
                    flags,
                };

                // This will emit fills/booktop/ack
                self.handle_new(no)
            }
        }
    }

    fn handle_new(&mut self, no: NewOrder) -> Vec<Event> {
        let mut evs = Vec::new();

        // Validate
        if no.symbol_id != self.book.symbol_id || no.qty <= 0 {
            evs.push(self.reject(no.client_seq, Some(no.order_id), RejectReason::Invalid));
            return evs;
        }

        // Post-only check
        if no.flags.post_only && self.book.would_cross(no.side, no.price) {
            evs.push(self.reject(
                no.client_seq,
                Some(no.order_id),
                RejectReason::PostOnlyWouldCross,
            ));
            return evs;
        }

        // Match if crossing
        if self.book.would_cross(no.side, no.price) {
            let fills: Vec<MatchFill> =
                self.book
                    .match_taker(no.order_id, no.side, no.price, no.qty);

            // Emit fills
            for f in fills.iter() {
                self.metrics.inc_fills();
                evs.push(Event::Fill(Fill {
                    server_seq: self.next_seq(),
                    client_seq: no.client_seq,
                    symbol_id: self.book.symbol_id,
                    taker_order_id: f.taker_order_id,
                    maker_order_id: f.maker_order_id,
                    price: f.price,
                    qty: f.qty,
                }));
            }

            // Remaining qty
            let filled_qty: i64 = fills.iter().map(|f| f.qty).sum();
            let rem = no.qty - filled_qty;

            // IOC: discard remainder. GTC: rest remainder.
            if rem > 0 && no.tif == TimeInForce::Gtc {
                self.book.insert_resting(Order {
                    order_id: no.order_id,
                    account_id: no.account_id,
                    symbol_id: no.symbol_id,
                    side: no.side,
                    price: no.price,
                    qty_rem: rem,
                    tif: no.tif,
                    flags: no.flags,
                });
            }
        } else {
            // Not crossing: rest if GTC, discard if IOC.
            if no.tif == TimeInForce::Gtc {
                self.book.insert_resting(Order {
                    order_id: no.order_id,
                    account_id: no.account_id,
                    symbol_id: no.symbol_id,
                    side: no.side,
                    price: no.price,
                    qty_rem: no.qty,
                    tif: no.tif,
                    flags: no.flags,
                });
            }
        }

        // Emit top-of-book then ack (so your read-until-ack sees both)
        evs.push(self.book_top_event());
        evs.push(self.ack(no.client_seq, no.order_id));
        evs
    }

    fn ack(&mut self, client_seq: u64, order_id: u64) -> Event {
        Event::Ack(Ack {
            server_seq: self.next_seq(),
            client_seq,
            order_id,
        })
    }

    fn reject(&mut self, client_seq: u64, order_id: Option<u64>, reason: RejectReason) -> Event {
        self.metrics.inc_rejects();
        Event::Reject(Reject {
            server_seq: self.next_seq(),
            client_seq,
            order_id,
            reason,
        })
    }

    fn book_top_event(&mut self) -> Event {
        let (bid_px, bid_qty) = self
            .book
            .best_bid()
            .map(|(px, qty)| (Some(px), Some(qty)))
            .unwrap_or((None, None));

        let (ask_px, ask_qty) = self
            .book
            .best_ask()
            .map(|(px, qty)| (Some(px), Some(qty)))
            .unwrap_or((None, None));

        Event::BookTop(BookTop {
            server_seq: self.next_seq(),
            symbol_id: self.book.symbol_id,
            best_bid_px: bid_px,
            best_bid_qty: bid_qty,
            best_ask_px: ask_px,
            best_ask_qty: ask_qty,
        })
    }

    pub fn reject_overloaded(&mut self, conn_id: u64, client_seq: u64) {
        let server_seq = self.next_seq();

        let _ = self.tx.send(Outbound {
            conn_id,
            ev: Event::Reject(Reject {
                server_seq,
                client_seq,
                order_id: None,
                reason: RejectReason::Overloaded,
            }),
        });
    }

    fn next_seq(&mut self) -> u64 {
        let s = self.server_seq;
        self.server_seq += 1;
        s
    }

    pub fn replay<I>(&mut self, cmds: I)
    where
        I: IntoIterator<Item = common::Command>,
    {
        for cmd in cmds {
            match cmd {
                common::Command::NewOrder(no) => {
                    // Re-apply matching + book updates, but ignore emitted events.
                    // This keeps book correct.
                    let _ = self.handle_new(no);
                }
                common::Command::Cancel(c) => {
                    let _ = self.book.cancel(c.order_id, c.account_id);
                }
                common::Command::Replace(r) => {
                    // Use your real replace path (infer side), but ignore emitted events.
                    let _ = self.process(common::Command::Replace(r));
                }
            }
        }

        // After replay, reset server_seq if you persisted it elsewhere.
        // For now, server_seq continues from 1, which is fine for a demo.
        // Better: persist server_seq in a checkpoint/snapshot later.
    }
}

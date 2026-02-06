#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crossbeam_channel::unbounded;

    use crate::engine::Engine;
    use common::Metrics;
    use common::{Command, NewOrder, OrderFlags, RejectReason, Side, TimeInForce};

    fn mk_engine() -> Engine {
        let (_, in_rx) = unbounded();
        let (out_tx, _out_rx) = unbounded();
        Engine::new(in_rx, out_tx, Arc::new(Metrics::default()))
    }

    #[test]
    fn post_only_cross_rejects() {
        let mut e = mk_engine();

        // rest ask 100 x5
        let evs1 = e.process(Command::NewOrder(NewOrder {
            client_seq: 1,
            order_id: 101,
            account_id: 1,
            symbol_id: 1,
            side: Side::Sell,
            price: 100,
            qty: 5,
            tif: TimeInForce::Gtc,
            flags: OrderFlags { post_only: false },
        }));
        assert!(evs1.iter().any(|ev| matches!(ev, common::Event::Ack(_))));

        // post-only buy that would cross -> reject
        let evs2 = e.process(Command::NewOrder(NewOrder {
            client_seq: 2,
            order_id: 202,
            account_id: 2,
            symbol_id: 1,
            side: Side::Buy,
            price: 110,
            qty: 1,
            tif: TimeInForce::Gtc,
            flags: OrderFlags { post_only: true },
        }));

        assert!(evs2.iter().any(|ev| match ev {
            common::Event::Reject(r) => matches!(r.reason, RejectReason::PostOnlyWouldCross),
            _ => false,
        }));
    }

    #[test]
    fn ioc_does_not_rest_remainder() {
        let mut e = mk_engine();

        // rest ask 100 x5
        let _ = e.process(Command::NewOrder(NewOrder {
            client_seq: 1,
            order_id: 101,
            account_id: 1,
            symbol_id: 1,
            side: Side::Sell,
            price: 100,
            qty: 5,
            tif: TimeInForce::Gtc,
            flags: OrderFlags { post_only: false },
        }));

        // IOC buy 110 x10: fills 5, remainder discarded
        let evs = e.process(Command::NewOrder(NewOrder {
            client_seq: 2,
            order_id: 202,
            account_id: 2,
            symbol_id: 1,
            side: Side::Buy,
            price: 110,
            qty: 10,
            tif: TimeInForce::Ioc,
            flags: OrderFlags { post_only: false },
        }));

        let filled: i64 = evs
            .iter()
            .filter_map(|ev| match ev {
                common::Event::Fill(f) => Some(f.qty),
                _ => None,
            })
            .sum();
        assert_eq!(filled, 5);

        // book should now be empty (no ask, no bid)
        let top = evs
            .iter()
            .find_map(|ev| match ev {
                common::Event::BookTop(t) => Some(t),
                _ => None,
            })
            .expect("expected BookTop");
        assert_eq!(top.best_ask_px, None);
        assert_eq!(top.best_bid_px, None);
    }

    #[test]
    fn replace_can_cross_and_fill() {
        let mut e = mk_engine();

        // rest ask 100 x5 (maker)
        let _ = e.process(Command::NewOrder(NewOrder {
            client_seq: 1,
            order_id: 101,
            account_id: 1,
            symbol_id: 1,
            side: Side::Sell,
            price: 100,
            qty: 5,
            tif: TimeInForce::Gtc,
            flags: OrderFlags { post_only: false },
        }));

        // rest buy 90 x5 (will be replaced)
        let _ = e.process(Command::NewOrder(NewOrder {
            client_seq: 2,
            order_id: 202,
            account_id: 2,
            symbol_id: 1,
            side: Side::Buy,
            price: 90,
            qty: 5,
            tif: TimeInForce::Gtc,
            flags: OrderFlags { post_only: false },
        }));

        // replace buy -> price crosses
        let evs = e.process(Command::Replace(common::Replace {
            client_seq: 3,
            order_id: 202,
            account_id: 2,
            symbol_id: 1,
            new_price: 110,
            new_qty: 5,
        }));

        assert!(evs.iter().any(|ev| match ev {
            common::Event::Fill(f) =>
                f.price == 100 && f.qty == 5 && f.maker_order_id == 101 && f.taker_order_id == 202,
            _ => false,
        }));
    }
}

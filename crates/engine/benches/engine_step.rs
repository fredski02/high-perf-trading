use criterion::{black_box, criterion_group, criterion_main, Criterion};

use common::{Command, NewOrder, OrderFlags, Side, TimeInForce};
use engine::Engine;

fn bench_engine_process(c: &mut Criterion) {
    // Dummy channels (not used in process bench)
    let (tx_in, rx_in) = crossbeam_channel::bounded(1);
    let (tx_out, _rx_out) = crossbeam_channel::bounded(1);
    let mut eng = Engine::new(rx_in, tx_out);

    let cmd = Command::NewOrder(NewOrder {
        client_seq: 1,
        order_id: 42,
        account_id: 7,
        symbol_id: 1,
        side: Side::Buy,
        price: 100,
        qty: 10,
        tif: TimeInForce::Gtc,
        flags: OrderFlags { post_only: false },
    });

    c.bench_function("engine_process_ack", |b| {
        b.iter(|| {
            let ev = eng.process(black_box(cmd.clone()));
            black_box(ev);
        })
    });

    drop(tx_in);
}

criterion_group!(benches, bench_engine_process);
criterion_main!(benches);

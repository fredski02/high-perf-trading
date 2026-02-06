use std::sync::Arc;
use std::{hint::black_box, time::Duration};

use common::Metrics;
use criterion::{criterion_group, criterion_main, BatchSize, Criterion};

use common::{Command, NewOrder, OrderFlags, Side, TimeInForce};
use engine::Engine;

fn criterion_config() -> Criterion {
    Criterion::default()
        .sample_size(500)
        .warm_up_time(Duration::from_secs(5))
        .measurement_time(Duration::from_secs(20))
        .confidence_level(0.99)
        .nresamples(200_000)
}

fn bench_engine_process(c: &mut Criterion) {
    // Dummy channels (not used in process bench)
    let metrics = Arc::new(Metrics::default());
    let (_tx_in, rx_in) = crossbeam_channel::bounded(1);
    let (tx_out, _rx_out) = crossbeam_channel::bounded(1);
    let mut eng = Engine::new(rx_in, tx_out, metrics);

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
        b.iter_batched(
            || cmd,                                           // setup (not timed)
            |cmd_i| black_box(eng.process(black_box(cmd_i))), // timed
            BatchSize::SmallInput,
        )
    });
}

criterion_group! {
    name = benches;
    config = criterion_config();
    targets = bench_engine_process
}
criterion_main!(benches);

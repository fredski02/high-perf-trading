use bytes::Bytes;
use criterion::{criterion_group, criterion_main, Criterion};
use std::{hint::black_box, time::Duration};

use codecs::{BinaryCodec, Codec};

fn criterion_config() -> Criterion {
    Criterion::default()
        .sample_size(200)
        .warm_up_time(Duration::from_secs(5))
        .measurement_time(Duration::from_secs(10))
        .confidence_level(0.99)
        .nresamples(200_000)
}

fn bench_decode_new_order(c: &mut Criterion) {
    let codec = BinaryCodec;

    let mut v = Vec::new();
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&1u64.to_le_bytes());
    v.extend_from_slice(&123u64.to_le_bytes());
    v.extend_from_slice(&7u32.to_le_bytes());
    v.extend_from_slice(&1u32.to_le_bytes());
    v.push(0);
    v.extend_from_slice(&100i64.to_le_bytes());
    v.extend_from_slice(&10i64.to_le_bytes());
    v.push(0);
    v.push(0);

    let payload = Bytes::from(v);

    c.bench_function("binary_decode_new_order", |b| {
        b.iter(|| {
            let cmd = codec.decode_command(black_box(&payload)).unwrap();
            black_box(cmd);
        })
    });
}

criterion_group! {
    name = benches;
    config = criterion_config();
    targets = bench_decode_new_order
}
criterion_main!(benches);

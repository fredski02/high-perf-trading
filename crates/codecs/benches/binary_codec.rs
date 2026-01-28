use bytes::Bytes;
use criterion::{black_box, criterion_group, criterion_main, Criterion};

use codecs::{BinaryCodec, Codec};
use common::{Command, NewOrder, OrderFlags, Side, TimeInForce};

fn bench_decode_new_order(c: &mut Criterion) {
    let codec = BinaryCodec::default();

    // Build a binary NewOrder payload: [u16 msg_type] + fields.
    let mut v = Vec::new();
    v.extend_from_slice(&1u16.to_le_bytes()); // MT_NEW_ORDER
    v.extend_from_slice(&1u64.to_le_bytes()); // client_seq
    v.extend_from_slice(&123u64.to_le_bytes()); // order_id
    v.extend_from_slice(&7u32.to_le_bytes()); // account_id
    v.extend_from_slice(&1u32.to_le_bytes()); // symbol_id
    v.push(0); // side buy
    v.extend_from_slice(&100i64.to_le_bytes()); // price
    v.extend_from_slice(&10i64.to_le_bytes()); // qty
    v.push(0); // tif gtc
    v.push(0); // post_only false

    let payload = Bytes::from(v);

    c.bench_function("binary_decode_new_order", |b| {
        b.iter(|| {
            let cmd = codec.decode_command(&payload).unwrap();
            black_box(cmd);
        })
    });
}

criterion_group!(benches, bench_decode_new_order);
criterion_main!(benches);

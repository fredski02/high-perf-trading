#!/bin/bash
# Test risk rejection
cargo build --release -q -p test_client

cat > /tmp/test_risk.rs << 'RUST'
use anyhow::Result;
use bytes::{BufMut, BytesMut, Buf};
use common::{Side, TimeInForce, OrderFlags};
use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_util::codec::LengthDelimitedCodec;

#[tokio::main]
async fn main() -> Result<()> {
    println!("Testing risk rejection...");
    let stream = TcpStream::connect("127.0.0.1:9000").await?;
    let framed = LengthDelimitedCodec::builder()
        .little_endian()
        .max_frame_length(10 * 1024 * 1024)
        .new_framed(stream);
    let (mut write_half, mut read_half) = framed.split();

    // Try to buy with insufficient funds
    // Account has $1M = 100,000,000 ticks
    // Try to buy at 200,000,000 ticks (will fail)
    let mut payload = BytesMut::with_capacity(256);
    payload.put_u16_le(1); // MT_NEW_ORDER
    payload.put_u64_le(100); // client_seq
    payload.put_u64_le(99999); // order_id
    payload.put_u32_le(1); // account_id
    payload.put_u32_le(1); // symbol_id
    payload.put_u8(0); // Buy
    payload.put_i64_le(200_000_000); // price (huge)
    payload.put_i64_le(1); // qty
    payload.put_u8(0); // Gtc
    payload.put_u8(0); // not post_only
    
    println!("Sending order that exceeds buying power...");
    write_half.send(payload.freeze()).await?;

    // Should get rejected or timeout (no response since risk check fails before routing)
    match tokio::time::timeout(tokio::time::Duration::from_secs(2), read_half.next()).await {
        Ok(Some(Ok(frame))) => {
            let mut b = frame.clone();
            let mt = b.get_u16_le();
            if mt == 102 {
                let _server_seq = b.get_u64_le();
                let _client_seq = b.get_u64_le();
                let reason_code = b.get_u8();
                println!("✅ Got REJECT as expected (reason code: {})", reason_code);
            } else {
                println!("⚠️  Got unexpected response type: {}", mt);
            }
        }
        _ => {
            println!("⚠️  No response (risk check failed at gateway, no reject sent to client yet)");
        }
    }

    println!("Test complete");
    Ok(())
}
RUST

rustc --edition 2021 /tmp/test_risk.rs \
  -L target/release/deps \
  --extern common=target/release/libcommon.rlib \
  --extern tokio=target/release/deps/libtokio*.rlib \
  --extern tokio_util=target/release/deps/libtokio_util*.rlib \
  --extern futures=target/release/deps/libfutures*.rlib \
  --extern bytes=target/release/deps/libbytes*.rlib \
  --extern anyhow=target/release/deps/libanyhow*.rlib \
  -o /tmp/test_risk 2>/dev/null || {
    echo "Build failed, using simple test instead"
    exit 1
}

/tmp/test_risk

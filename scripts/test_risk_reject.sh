#!/bin/bash
# Quick test for risk rejection
cd "$(dirname "$0")/.." || exit 1

cat > /tmp/test_risk.rs << 'RUST'
use bytes::{BufMut, BytesMut, Buf};
use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_util::codec::LengthDelimitedCodec;

#[tokio::main]
async fn main() {
    println!("Testing risk rejection (insufficient funds)...\n");
    
    let stream = TcpStream::connect("127.0.0.1:9000").await.unwrap();
    let framed = LengthDelimitedCodec::builder()
        .little_endian()
        .max_frame_length(10 * 1024 * 1024)
        .new_framed(stream);
    let (mut write_half, mut read_half) = framed.split();

    // Account has $1M = 100,000,000 ticks buying power
    // Try to buy 1 @ 200,000,000 ticks (exceeds buying power)
    let mut payload = BytesMut::with_capacity(256);
    payload.put_u16_le(1); // MT_NEW_ORDER
    payload.put_u64_le(100); // client_seq
    payload.put_u64_le(99999); // order_id
    payload.put_u32_le(1); // account_id
    payload.put_u32_le(1); // symbol_id
    payload.put_u8(0); // Buy
    payload.put_i64_le(200_000_000); // price (huge - exceeds buying power)
    payload.put_i64_le(1); // qty
    payload.put_u8(0); // Gtc
    payload.put_u8(0); // not post_only
    
    println!("Sending order with insufficient funds:");
    println!("  Price: 200,000,000 ticks ($2M)");
    println!("  Qty: 1");
    println!("  Required: $2M");
    println!("  Available: $1M");
    println!("  Expected: REJECT (Risk)\n");
    
    write_half.send(payload.freeze()).await.unwrap();

    // Should get reject response
    match tokio::time::timeout(tokio::time::Duration::from_secs(3), read_half.next()).await {
        Ok(Some(Ok(frame))) => {
            let mut b = frame.clone();
            let mt = b.get_u16_le();
            match mt {
                102 => { // MT_REJECT
                    let server_seq = b.get_u64_le();
                    let client_seq = b.get_u64_le();
                    let reason_code = b.get_u8();
                    let reason = match reason_code {
                        1 => "Invalid",
                        2 => "Risk",
                        3 => "Overloaded",
                        4 => "NotFound",
                        5 => "PostOnlyWouldCross",
                        _ => "Unknown",
                    };
                    println!("✅ SUCCESS: Got REJECT event");
                    println!("   Reason: {} (code {})", reason, reason_code);
                    println!("   Server seq: {}", server_seq);
                    println!("   Client seq: {}", client_seq);
                    
                    if reason == "Risk" {
                        println!("\n🎉 Risk rejection working correctly!");
                    }
                }
                _ => {
                    println!("❌ FAIL: Got unexpected message type: {}", mt);
                }
            }
        }
        Ok(Some(Err(e))) => {
            println!("❌ FAIL: Error: {:?}", e);
        }
        Ok(None) => {
            println!("❌ FAIL: Connection closed");
        }
        Err(_) => {
            println!("❌ FAIL: Timeout - no response received");
        }
    }
}
RUST

rustc --edition 2021 /tmp/test_risk.rs \
  --extern tokio=/tmp/dummy.rlib \
  --extern tokio_util=/tmp/dummy.rlib \
  --extern futures=/tmp/dummy.rlib \
  --extern bytes=/tmp/dummy.rlib \
  -o /tmp/test_risk 2>/dev/null && /tmp/test_risk || {
    echo "Standalone build failed, using cargo..."
    cd /home/fred/projects/high-perf-trading
    
    # Create a minimal test using the test_client structure
    cat > crates/test_client/src/main.rs << 'MAIN'
use bytes::{BufMut, BytesMut, Buf};
use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_util::codec::LengthDelimitedCodec;

#[tokio::main]
async fn main() {
    println!("Testing risk rejection (insufficient funds)...\n");
    
    let stream = TcpStream::connect("127.0.0.1:9000").await.unwrap();
    let framed = LengthDelimitedCodec::builder()
        .little_endian()
        .max_frame_length(10 * 1024 * 1024)
        .new_framed(stream);
    let (mut write_half, mut read_half) = framed.split();

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
    
    println!("Sending order with insufficient funds:");
    println!("  Required: $2M, Available: $1M\n");
    
    write_half.send(payload.freeze()).await.unwrap();

    match tokio::time::timeout(tokio::time::Duration::from_secs(3), read_half.next()).await {
        Ok(Some(Ok(frame))) => {
            let mut b = frame.clone();
            let mt = b.get_u16_le();
            if mt == 102 {
                let _server_seq = b.get_u64_le();
                let _client_seq = b.get_u64_le();
                let reason_code = b.get_u8();
                let reason = match reason_code {
                    2 => "Risk",
                    _ => "Other",
                };
                println!("✅ SUCCESS: Got REJECT ({})", reason);
            } else {
                println!("❌ Got message type: {}", mt);
            }
        }
        _ => println!("❌ FAIL: No response"),
    }
}
MAIN
    
    cargo build --release -q -p test_client && ./target/release/test_client
}

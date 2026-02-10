//! Test client for authentication flow
//!
//! Tests:
//! 1. Authentication with valid API key
//! 2. Authentication with invalid API key  
//! 3. Placing order without authentication (should reject)
//! 4. Placing order after successful authentication (should work)

use bytes::{Buf, BufMut, BytesMut};
use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_util::codec::LengthDelimitedCodec;

const MT_AUTHENTICATE: u16 = 6;
const MT_NEW_ORDER: u16 = 1;

const MT_AUTH_SUCCESS: u16 = 106;
const MT_AUTH_FAILURE: u16 = 107;
const MT_ACK: u16 = 101;
const MT_REJECT: u16 = 102;

#[tokio::main]
async fn main() {
    println!("=== Authentication Flow Test ===\n");

    // Test 1: Authentication with valid API key
    println!("Test 1: Authenticate with valid API key (test-key-1)");
    let stream = TcpStream::connect("127.0.0.1:9000").await.unwrap();
    let framed = LengthDelimitedCodec::builder()
        .little_endian()
        .max_frame_length(10 * 1024 * 1024)
        .new_framed(stream);
    let (mut write_half, mut read_half) = framed.split();

    send_authenticate(&mut write_half, "test-key-1").await;
    let auth_result = read_auth_response(&mut read_half).await;
    match auth_result {
        Some((true, Some(account_id))) => {
            println!(
                "✓ Successfully authenticated as account_id={}\n",
                account_id
            );
        }
        Some((false, None)) => {
            println!("✗ Authentication failed (unexpected)\n");
            return;
        }
        _ => {
            println!("✗ No response or invalid response\n");
            return;
        }
    }

    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Test 4: Place order after authentication (should succeed)
    println!("Test 4: Place order after successful authentication");
    send_order(&mut write_half, 100, 1000, 0, 50000, 1).await;
    let order_result = read_order_response(&mut read_half).await;
    match order_result {
        Some(true) => println!("✓ Order accepted (Ack received)\n"),
        Some(false) => println!("✗ Order rejected (unexpected)\n"),
        None => println!("✗ No response\n"),
    }

    drop(write_half);
    drop(read_half);

    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Test 2: Authentication with invalid API key
    println!("Test 2: Authenticate with invalid API key (wrong-key)");
    let stream2 = TcpStream::connect("127.0.0.1:9000").await.unwrap();
    let framed2 = LengthDelimitedCodec::builder()
        .little_endian()
        .max_frame_length(10 * 1024 * 1024)
        .new_framed(stream2);
    let (mut write_half2, mut read_half2) = framed2.split();

    send_authenticate(&mut write_half2, "wrong-key").await;
    let auth_result = read_auth_response(&mut read_half2).await;
    match auth_result {
        Some((false, None)) => {
            println!("✓ Authentication rejected as expected\n");
        }
        Some((true, _)) => {
            println!("✗ Authentication succeeded (unexpected)\n");
            return;
        }
        _ => {
            println!("✗ No response or invalid response\n");
            return;
        }
    }

    drop(write_half2);
    drop(read_half2);

    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    // Test 3: Place order without authentication (should reject)
    println!("Test 3: Place order WITHOUT authentication");
    let stream3 = TcpStream::connect("127.0.0.1:9000").await.unwrap();
    let framed3 = LengthDelimitedCodec::builder()
        .little_endian()
        .max_frame_length(10 * 1024 * 1024)
        .new_framed(stream3);
    let (mut write_half3, mut read_half3) = framed3.split();

    // Send order without authenticating first
    send_order(&mut write_half3, 200, 2000, 0, 50000, 1).await;
    let order_result = read_order_response(&mut read_half3).await;
    match order_result {
        Some(false) => println!("✓ Order rejected as expected (not authenticated)\n"),
        Some(true) => println!("✗ Order accepted (unexpected - security violation!)\n"),
        None => println!("✗ No response\n"),
    }

    println!("=== All Tests Complete ===");
}

async fn send_authenticate(
    write_half: &mut futures::stream::SplitSink<
        tokio_util::codec::Framed<TcpStream, LengthDelimitedCodec>,
        bytes::Bytes,
    >,
    api_key: &str,
) {
    let mut payload = BytesMut::with_capacity(256);
    payload.put_u16_le(MT_AUTHENTICATE);

    // String encoding: [u32 length][bytes]
    let key_bytes = api_key.as_bytes();
    payload.put_u32_le(key_bytes.len() as u32);
    payload.put_slice(key_bytes);

    write_half.send(payload.freeze()).await.unwrap();
}

async fn read_auth_response(
    read_half: &mut futures::stream::SplitStream<
        tokio_util::codec::Framed<TcpStream, LengthDelimitedCodec>,
    >,
) -> Option<(bool, Option<u32>)> {
    match tokio::time::timeout(tokio::time::Duration::from_secs(3), read_half.next()).await {
        Ok(Some(Ok(frame))) => {
            let mut b = frame.clone();
            let mt = b.get_u16_le();

            match mt {
                MT_AUTH_SUCCESS => {
                    let account_id = b.get_u32_le();
                    Some((true, Some(account_id)))
                }
                MT_AUTH_FAILURE => {
                    let len = b.get_u32_le() as usize;
                    let mut reason_bytes = vec![0u8; len];
                    b.copy_to_slice(&mut reason_bytes);
                    let reason = String::from_utf8_lossy(&reason_bytes);
                    println!("  Failure reason: {}", reason);
                    Some((false, None))
                }
                _ => {
                    println!("  Unexpected message type: {}", mt);
                    None
                }
            }
        }
        _ => None,
    }
}

async fn send_order(
    write_half: &mut futures::stream::SplitSink<
        tokio_util::codec::Framed<TcpStream, LengthDelimitedCodec>,
        bytes::Bytes,
    >,
    client_seq: u64,
    order_id: u64,
    side: u8,
    price: i64,
    qty: i64,
) {
    let mut payload = BytesMut::with_capacity(256);
    payload.put_u16_le(MT_NEW_ORDER);
    payload.put_u64_le(client_seq);
    payload.put_u64_le(order_id);
    payload.put_u32_le(1); // account_id
    payload.put_u32_le(1); // symbol_id
    payload.put_u8(side);
    payload.put_i64_le(price);
    payload.put_i64_le(qty);
    payload.put_u8(0); // Gtc
    payload.put_u8(0); // not post_only

    write_half.send(payload.freeze()).await.unwrap();
}

async fn read_order_response(
    read_half: &mut futures::stream::SplitStream<
        tokio_util::codec::Framed<TcpStream, LengthDelimitedCodec>,
    >,
) -> Option<bool> {
    match tokio::time::timeout(tokio::time::Duration::from_secs(3), read_half.next()).await {
        Ok(Some(Ok(frame))) => {
            let mut b = frame.clone();
            let mt = b.get_u16_le();

            match mt {
                MT_ACK => {
                    println!("  Received: Ack");
                    Some(true)
                }
                MT_REJECT => {
                    println!("  Received: Reject");
                    Some(false)
                }
                _ => {
                    println!("  Received: Unknown message type {}", mt);
                    None
                }
            }
        }
        _ => {
            println!("  No response (timeout)");
            None
        }
    }
}

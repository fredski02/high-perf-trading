use bytes::{BufMut, BytesMut, Buf};
use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_util::codec::LengthDelimitedCodec;

const MT_NEW_ORDER: u16 = 1;
const MT_QUERY_ACCOUNT: u16 = 5;
const MT_ACCOUNT_STATE: u16 = 105;
const MT_FILL: u16 = 103;
const MT_BOOK_TOP: u16 = 104;
const MT_ACK: u16 = 101;

#[tokio::main]
async fn main() {
    println!("=== Account State Update Test ===\n");
    
    let stream = TcpStream::connect("127.0.0.1:9000").await.unwrap();
    let framed = LengthDelimitedCodec::builder()
        .little_endian()
        .max_frame_length(10 * 1024 * 1024)
        .new_framed(stream);
    let (mut write_half, mut read_half) = framed.split();

    // Step 1: Query initial account state
    println!("1. Querying initial account state...");
    send_query_account(&mut write_half, 1, 1, 1).await;
    read_and_display_account_state(&mut read_half).await;
    
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    
    // Step 2: Place BUY order
    println!("\n2. Placing BUY order (1 @ 10000)...");
    send_order(&mut write_half, 10, 5001, 0, 10000, 1).await;
    drain_responses(&mut read_half, 1).await; // BookTop
    
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    
    // Step 3: Place matching SELL order
    println!("\n3. Placing SELL order (1 @ 10000) - should fill...");
    send_order(&mut write_half, 11, 5002, 1, 10000, 1).await;
    
    // Read fill events
    println!("   Waiting for fill events...");
    drain_responses(&mut read_half, 3).await; // Ack, Fill, BookTop
    
    // Wait for account state to be updated
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    
    // Step 4: Query account state after fill
    println!("\n4. Querying account state after fill...");
    send_query_account(&mut write_half, 20, 1, 1).await;
    read_and_display_account_state(&mut read_half).await;
    
    println!("\n=== Test Complete ===");
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

async fn send_query_account(
    write_half: &mut futures::stream::SplitSink<
        tokio_util::codec::Framed<TcpStream, LengthDelimitedCodec>,
        bytes::Bytes,
    >,
    client_seq: u64,
    account_id: u32,
    symbol_id: u32,
) {
    let mut payload = BytesMut::with_capacity(256);
    payload.put_u16_le(MT_QUERY_ACCOUNT);
    payload.put_u64_le(client_seq);
    payload.put_u32_le(account_id);
    payload.put_u32_le(symbol_id);
    
    write_half.send(payload.freeze()).await.unwrap();
}

async fn read_and_display_account_state(
    read_half: &mut futures::stream::SplitStream<
        tokio_util::codec::Framed<TcpStream, LengthDelimitedCodec>,
    >,
) {
    match tokio::time::timeout(tokio::time::Duration::from_secs(3), read_half.next()).await {
        Ok(Some(Ok(frame))) => {
            let mut b = frame.clone();
            let mt = b.get_u16_le();
            if mt == MT_ACCOUNT_STATE {
                let _server_seq = b.get_u64_le();
                let client_seq = b.get_u64_le();
                let account_id = b.get_u32_le();
                let symbol_id = b.get_u32_le();
                
                // Position
                let net_position = b.get_i64_le();
                let avg_price = b.get_i64_le();
                let realized_pnl = b.get_i64_le();
                
                // Risk limits
                let max_long = b.get_i64_le();
                let max_short = b.get_i64_le();
                let max_order_size = b.get_i64_le();
                
                println!("   Account State (client_seq={}):", client_seq);
                println!("     Account ID: {}", account_id);
                println!("     Symbol ID: {}", symbol_id);
                println!("     Position:");
                println!("       Net: {}", net_position);
                println!("       Avg Price: {}", avg_price);
                println!("       Realized P&L: {}", realized_pnl);
                println!("     Risk Limits:");
                println!("       Max Long: {}", max_long);
                println!("       Max Short: {}", max_short);
                println!("       Max Order Size: {}", max_order_size);
            } else {
                println!("   Got message type: {} (expected {})", mt, MT_ACCOUNT_STATE);
            }
        }
        _ => println!("   No response"),
    }
}

async fn drain_responses(
    read_half: &mut futures::stream::SplitStream<
        tokio_util::codec::Framed<TcpStream, LengthDelimitedCodec>,
    >,
    count: usize,
) {
    for _ in 0..count {
        if let Ok(Some(Ok(frame))) = tokio::time::timeout(
            tokio::time::Duration::from_secs(1),
            read_half.next()
        ).await {
            let mut b = frame.clone();
            let mt = b.get_u16_le();
            match mt {
                MT_FILL => println!("   Got FILL"),
                MT_BOOK_TOP => println!("   Got BOOK_TOP"),
                MT_ACK => println!("   Got ACK"),
                _ => println!("   Got message type {}", mt),
            }
        }
    }
}

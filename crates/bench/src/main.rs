use bytes::{BufMut, BytesMut};
use clap::Parser;
use hdrhistogram::Histogram;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::Instant,
};

use common::{Command, Event, NewOrder, OrderFlags, Side, TimeInForce};

#[derive(Parser, Debug)]
struct Args {
    /// Modes:
    /// - smoke: simple JSON + simple binary (best-effort)
    /// - smoke-match: JSON smoke that asserts matching (recommended)
    /// - bench: binary RTT loop
    #[arg(long, default_value = "smoke")]
    mode: String,

    #[arg(long, default_value = "127.0.0.1:9000")]
    bin_addr: String,

    #[arg(long, default_value = "127.0.0.1:9001")]
    json_addr: String,

    #[arg(long, default_value_t = 1000)]
    iters: u32,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    match args.mode.as_str() {
        "smoke" => {
            smoke_json_simple(&args.json_addr).await?;
            smoke_bin_simple(&args.bin_addr).await?;
            println!("smoke ok");
        }
        "smoke-match" => {
            smoke_json_match(&args.json_addr).await?;
            println!("smoke-match ok");
        }
        "bench" => {
            bench_bin(&args.bin_addr, args.iters).await?;
        }
        other => anyhow::bail!("unknown --mode {other} (use smoke|smoke-match|bench)"),
    }

    Ok(())
}

//
// ------------------------- JSON smoke -------------------------
//

/// Original-style smoke: send 1 JSON order and print events until ACK.
async fn smoke_json_simple(addr: &str) -> anyhow::Result<()> {
    let mut s = TcpStream::connect(addr).await?;

    let cmd = Command::NewOrder(NewOrder {
        client_seq: 1,
        order_id: 1,
        account_id: 7,
        symbol_id: 1,
        side: Side::Buy,
        price: 100,
        qty: 10,
        tif: TimeInForce::Gtc,
        flags: OrderFlags { post_only: false },
    });

    send_json_framed(&mut s, &cmd).await?;
    let events = read_until_ack_json(&mut s, 1, 32).await?;
    for ev in events {
        println!("json ev: {:?}", ev);
    }

    Ok(())
}

/// Phase-1 smoke: asserts matching works.
/// - place resting ask @100 x5
/// - place crossing buy @110 x3
/// - assert we saw Fill(price=100, qty=3, maker=ask, taker=buy) and at least one BookTop
async fn smoke_json_match(addr: &str) -> anyhow::Result<()> {
    let mut s = TcpStream::connect(addr).await?;

    let ask = Command::NewOrder(NewOrder {
        client_seq: 1,
        order_id: 101,
        account_id: 1,
        symbol_id: 1,
        side: Side::Sell,
        price: 100,
        qty: 5,
        tif: TimeInForce::Gtc,
        flags: OrderFlags { post_only: false },
    });

    send_json_framed(&mut s, &ask).await?;
    let evs1 = read_until_ack_json(&mut s, 1, 64).await?;

    let buy = Command::NewOrder(NewOrder {
        client_seq: 2,
        order_id: 202,
        account_id: 2,
        symbol_id: 1,
        side: Side::Buy,
        price: 110,
        qty: 3,
        tif: TimeInForce::Gtc,
        flags: OrderFlags { post_only: false },
    });

    send_json_framed(&mut s, &buy).await?;
    let evs2 = read_until_ack_json(&mut s, 2, 128).await?;

    let all: Vec<Event> = evs1.into_iter().chain(evs2.into_iter()).collect();

    let saw_top = all.iter().any(|e| matches!(e, Event::BookTop(_)));
    let saw_fill = all.iter().any(|e| match e {
        Event::Fill(f) => {
            f.price == 100 && f.qty == 3 && f.taker_order_id == 202 && f.maker_order_id == 101
        }
        _ => false,
    });

    if !saw_top || !saw_fill {
        anyhow::bail!(
            "smoke-match failed: saw_top={} saw_fill={}\nEvents:\n{:#?}",
            saw_top,
            saw_fill,
            all
        );
    }

    println!("smoke-match events:\n{:#?}", all);
    Ok(())
}

async fn send_json_framed(s: &mut TcpStream, cmd: &Command) -> anyhow::Result<()> {
    let payload = serde_json::to_vec(cmd)?;
    let frame = frame(&payload);
    s.write_all(&frame).await?;
    Ok(())
}

async fn read_until_ack_json(
    s: &mut TcpStream,
    client_seq: u64,
    max_events: usize,
) -> anyhow::Result<Vec<Event>> {
    let mut out = Vec::new();

    for _ in 0..max_events {
        let bytes = read_one_frame(s).await?;
        let ev: Event = serde_json::from_slice(&bytes)?;
        out.push(ev.clone());

        if let Event::Ack(a) = ev {
            if a.client_seq == client_seq {
                return Ok(out);
            }
        }
    }

    anyhow::bail!(
        "did not see Ack(client_seq={}) within {} events. Last events:\n{:#?}",
        client_seq,
        max_events,
        out
    );
}

//
// ------------------------- Binary smoke + bench -------------------------
//

/// Best-effort binary smoke:
/// We send 1 order, then read up to a few frames so it doesn't hang when engine emits multiple events.
/// (We don't decode binary events here yet.)
async fn smoke_bin_simple(addr: &str) -> anyhow::Result<()> {
    let mut s = TcpStream::connect(addr).await?;

    // Binary NewOrder payload (same layout as codec expects):
    // [u16 mt=1][u64 client_seq][u64 order_id][u32 account_id][u32 symbol_id]
    // [u8 side][i64 price][i64 qty][u8 tif][u8 post_only]
    let mut p = BytesMut::with_capacity(2 + 8 + 8 + 4 + 4 + 1 + 8 + 8 + 1 + 1);
    p.put_u16_le(1);
    p.put_u64_le(1);
    p.put_u64_le(1);
    p.put_u32_le(7);
    p.put_u32_le(1);
    p.put_u8(0); // buy
    p.put_i64_le(100);
    p.put_i64_le(10);
    p.put_u8(0); // gtc
    p.put_u8(0); // post_only false

    let frame = frame(&p);
    s.write_all(&frame).await?;

    // Read a few frames (ack + booktop, possibly fills in other scenarios).
    // Avoid hanging if server emits multiple messages.
    for i in 0..3 {
        match read_one_frame(&mut s).await {
            Ok(resp) => println!("bin resp[{i}] len={}", resp.len()),
            Err(e) => {
                println!("bin read ended early: {e}");
                break;
            }
        }
    }

    Ok(())
}

async fn bench_bin(addr: &str, iters: u32) -> anyhow::Result<()> {
    let mut s = TcpStream::connect(addr).await?;
    let mut h = Histogram::<u64>::new(3)?;

    for i in 0..iters {
        let mut p = BytesMut::with_capacity(64);
        p.put_u16_le(1);
        p.put_u64_le(i as u64 + 1);
        p.put_u64_le(i as u64 + 1000);
        p.put_u32_le(7);
        p.put_u32_le(1);
        p.put_u8(0);
        p.put_i64_le(100);
        p.put_i64_le(10);
        p.put_u8(0);
        p.put_u8(0);

        let frame = frame(&p);
        let t0 = Instant::now();
        s.write_all(&frame).await?;

        // For bench we only read 1 frame (fast path RTT proxy).
        // If you start emitting multiple frames per request on binary, you might want to
        // read until an ACK (requires binary event decoding).
        let _resp = read_one_frame(&mut s).await?;

        let dt = t0.elapsed().as_nanos() as u64;
        let _ = h.record(dt);
    }

    println!("iters={}", iters);
    println!("p50={}us", h.value_at_quantile(0.50) / 1000);
    println!("p99={}us", h.value_at_quantile(0.99) / 1000);
    println!("p999={}us", h.value_at_quantile(0.999) / 1000);
    Ok(())
}

//
// ------------------------- Framing helpers -------------------------
//

fn frame(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

async fn read_one_frame(s: &mut TcpStream) -> anyhow::Result<Vec<u8>> {
    let mut hdr = [0u8; 4];
    s.read_exact(&mut hdr).await?;
    let len = u32::from_le_bytes(hdr) as usize;
    let mut buf = vec![0u8; len];
    s.read_exact(&mut buf).await?;
    Ok(buf)
}

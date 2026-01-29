use anyhow::Context;
use bytes::{BufMut, BytesMut};
use clap::Parser;
use hdrhistogram::Histogram;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::Instant,
};

#[allow(unused_imports)]
use common::Side;

use common::{Ack, Command, Event, NewOrder, OrderFlags, RejectReason, TimeInForce};
#[derive(Parser, Debug)]
struct Args {
    #[arg(long, default_value = "smoke-all")]
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
            smoke_json(&args.json_addr).await?;
            smoke_bin(&args.bin_addr).await?;
            println!("smoke ok");
        }
        "smoke-match" => {
            smoke_match_json(&args.json_addr).await?;
            println!("smoke-match ok");
        }
        "smoke-postonly" => {
            smoke_postonly_json(&args.json_addr).await?;
            println!("smoke-postonly ok");
        }
        "smoke-ioc" => {
            smoke_ioc_json(&args.json_addr).await?;
            println!("smoke-ioc ok");
        }
        "smoke-all" => {
            smoke_match_json(&args.json_addr).await?;
            smoke_postonly_json(&args.json_addr).await?;
            smoke_ioc_json(&args.json_addr).await?;
            println!("smoke-all ok");
        }
        "bench-bin" => {
            bench_bin(&args.bin_addr, args.iters).await?;
        }
        "smoke-replay" => {
            smoke_replay_json(&args.json_addr).await?;
            println!("smoke-replay ok");
        }
        other => anyhow::bail!("unknown mode: {other}"),
    }

    Ok(())
}

// -------------------- Smoke: JSON basic --------------------

async fn smoke_json(addr: &str) -> anyhow::Result<()> {
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

    write_json_cmd(&mut s, &cmd).await?;
    let events = read_until_ack_json(&mut s, 1).await?;
    for ev in events {
        println!("json ev: {:?}", ev);
    }
    Ok(())
}

// -------------------- Smoke: Binary basic --------------------

async fn smoke_bin(addr: &str) -> anyhow::Result<()> {
    let mut s = TcpStream::connect(addr).await?;

    // Binary NewOrder payload:
    // [u16 mt=1][u64 client_seq][u64 order_id][u32 account_id][u32 symbol_id]
    // [u8 side][i64 price][i64 qty][u8 tif][u8 post_only]
    let mut p = BytesMut::with_capacity(2 + 8 + 8 + 4 + 4 + 1 + 8 + 8 + 1 + 1);
    p.put_u16_le(1);
    p.put_u64_le(1);
    p.put_u64_le(1);
    p.put_u32_le(7);
    p.put_u32_le(1);
    p.put_u8(0);
    p.put_i64_le(100);
    p.put_i64_le(10);
    p.put_u8(0);
    p.put_u8(0);

    let frame = frame(&p);
    s.write_all(&frame).await?;

    let resp = read_one_frame(&mut s).await?;
    println!("bin resp len={}", resp.len());
    Ok(())
}

// -------------------- Smoke: match scenario --------------------
//
// Rest ask 100 x 5, then buy 110 x 3 -> should Fill 3 @ 100, leave ask 100 x 2.
async fn smoke_match_json(addr: &str) -> anyhow::Result<()> {
    let mut s = TcpStream::connect(addr).await?;

    // maker: sell 100 x5
    let ask = Command::NewOrder(NewOrder {
        client_seq: 1,
        order_id: 101,
        account_id: 7,
        symbol_id: 1,
        side: Side::Sell,
        price: 100,
        qty: 5,
        tif: TimeInForce::Gtc,
        flags: OrderFlags { post_only: false },
    });
    write_json_cmd(&mut s, &ask).await?;
    let evs1 = read_until_ack_json(&mut s, 1).await?;

    // taker: buy 110 x3
    let buy = Command::NewOrder(NewOrder {
        client_seq: 2,
        order_id: 202,
        account_id: 8,
        symbol_id: 1,
        side: Side::Buy,
        price: 110,
        qty: 3,
        tif: TimeInForce::Gtc,
        flags: OrderFlags { post_only: false },
    });
    write_json_cmd(&mut s, &buy).await?;
    let evs2 = read_until_ack_json(&mut s, 2).await?;

    let all = [evs1, evs2].concat();

    // Assertions
    let saw_fill = all.iter().any(|ev| match ev {
        Event::Fill(f) => {
            f.price == 100 && f.qty == 3 && f.maker_order_id == 101 && f.taker_order_id == 202
        }
        _ => false,
    });

    let saw_top_after = all.iter().rev().find_map(|ev| match ev {
        Event::BookTop(t) => Some(*t),
        _ => None,
    });

    let top = saw_top_after.context("expected at least one BookTop")?;
    let ok_top = top.best_ask_px == Some(100) && top.best_ask_qty == Some(2);

    if !saw_fill || !ok_top {
        anyhow::bail!(
            "smoke-match failed: saw_fill={} ok_top={}. Events:\n{:#?}",
            saw_fill,
            ok_top,
            all
        );
    }

    println!("smoke-match events:\n{:#?}", all);
    Ok(())
}

// -------------------- Smoke: post-only scenario --------------------
//
// Rest ask 100 x5, then post-only buy 110 x1 -> should Reject(PostOnlyWouldCross)
async fn smoke_postonly_json(addr: &str) -> anyhow::Result<()> {
    let mut s = TcpStream::connect(addr).await?;

    // maker: sell 100 x5
    let ask = Command::NewOrder(NewOrder {
        client_seq: 1,
        order_id: 301,
        account_id: 7,
        symbol_id: 1,
        side: Side::Sell,
        price: 100,
        qty: 5,
        tif: TimeInForce::Gtc,
        flags: OrderFlags { post_only: false },
    });
    write_json_cmd(&mut s, &ask).await?;
    let evs1 = read_until_ack_json(&mut s, 1).await?;

    // post-only taker that would cross -> reject
    let po_buy = Command::NewOrder(NewOrder {
        client_seq: 2,
        order_id: 302,
        account_id: 8,
        symbol_id: 1,
        side: Side::Buy,
        price: 110,
        qty: 1,
        tif: TimeInForce::Gtc,
        flags: OrderFlags { post_only: true },
    });
    write_json_cmd(&mut s, &po_buy).await?;

    // For rejects, we can't "read until ack" (there won't be an ack).
    // We'll read a few frames and assert a reject with PostOnlyWouldCross.
    let evs2 = read_n_events_json(&mut s, 2).await?;

    let all = [evs1, evs2].concat();

    let saw_reject = all.iter().any(|ev| match ev {
        Event::Reject(r) => {
            matches!(r.reason, RejectReason::PostOnlyWouldCross) && r.client_seq == 2
        }
        _ => false,
    });

    if !saw_reject {
        anyhow::bail!(
            "smoke-postonly failed: no PostOnlyWouldCross reject. Events:\n{:#?}",
            all
        );
    }

    println!("smoke-postonly events:\n{:#?}", all);
    Ok(())
}

// -------------------- Smoke: IOC scenario --------------------
//
// Rest ask 100 x5, then IOC buy 110 x10 -> should Fill 5 @ 100, and book becomes empty (no ask).
async fn smoke_ioc_json(addr: &str) -> anyhow::Result<()> {
    let mut s = TcpStream::connect(addr).await?;

    // maker: sell 100 x5
    let ask = Command::NewOrder(NewOrder {
        client_seq: 1,
        order_id: 401,
        account_id: 7,
        symbol_id: 1,
        side: Side::Sell,
        price: 100,
        qty: 5,
        tif: TimeInForce::Gtc,
        flags: OrderFlags { post_only: false },
    });
    write_json_cmd(&mut s, &ask).await?;
    let evs1 = read_until_ack_json(&mut s, 1).await?;

    // IOC buy 110 x10
    let ioc_buy = Command::NewOrder(NewOrder {
        client_seq: 2,
        order_id: 402,
        account_id: 8,
        symbol_id: 1,
        side: Side::Buy,
        price: 110,
        qty: 10,
        tif: TimeInForce::Ioc,
        flags: OrderFlags { post_only: false },
    });
    write_json_cmd(&mut s, &ioc_buy).await?;
    let evs2 = read_until_ack_json(&mut s, 2).await?;

    let all = [evs1, evs2].concat();

    let filled_qty: i64 = all
        .iter()
        .filter_map(|ev| match ev {
            Event::Fill(f) if f.taker_order_id == 402 => Some(f.qty),
            _ => None,
        })
        .sum();

    let top = all
        .iter()
        .rev()
        .find_map(|ev| match ev {
            Event::BookTop(t) => Some(*t),
            _ => None,
        })
        .context("expected BookTop")?;

    let ok = filled_qty == 5 && top.best_ask_px.is_none() && top.best_bid_px.is_none();

    if !ok {
        anyhow::bail!(
            "smoke-ioc failed: filled_qty={} top={:?}. Events:\n{:#?}",
            filled_qty,
            top,
            all
        );
    }

    println!("smoke-ioc events:\n{:#?}", all);
    Ok(())
}

// -------------------- Bench: binary RTT --------------------

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

// -------------------- Helpers --------------------

async fn write_json_cmd(s: &mut TcpStream, cmd: &Command) -> anyhow::Result<()> {
    let payload = serde_json::to_vec(cmd)?;
    let frame = frame(&payload);
    s.write_all(&frame).await?;
    Ok(())
}

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

async fn read_until_ack_json(s: &mut TcpStream, client_seq: u64) -> anyhow::Result<Vec<Event>> {
    let mut out = Vec::new();
    loop {
        let bytes = read_one_frame(s).await?;
        let ev: Event = serde_json::from_slice(&bytes)?;
        out.push(ev.clone());

        if let Event::Ack(Ack { client_seq: cs, .. }) = ev {
            if cs == client_seq {
                return Ok(out);
            }
        }
    }
}

// Read N events (used for reject-only cases)
async fn read_n_events_json(s: &mut TcpStream, n: usize) -> anyhow::Result<Vec<Event>> {
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let bytes = read_one_frame(s).await?;
        let ev: Event = serde_json::from_slice(&bytes)?;
        out.push(ev);
    }
    Ok(out)
}

// -------------------- Smoke: replay scenario --------------------
//
// Sends ONLY a crossing taker buy.
// This should only fill if the server restarted and replayed a resting ask from the journal.
//
// Assumptions:
// - symbol_id=1
// - there exists a resting ask at 100 (e.g. from previous smoke-all run)
//
// Behavior:
// - expects at least one Fill for this taker order_id
async fn smoke_replay_json(addr: &str) -> anyhow::Result<()> {
    let mut s = TcpStream::connect(addr).await?;

    // taker: buy 110 x1
    let buy = Command::NewOrder(NewOrder {
        client_seq: 1,
        order_id: 9001,
        account_id: 42,
        symbol_id: 1,
        side: Side::Buy,
        price: 110,
        qty: 1,
        tif: TimeInForce::Gtc,
        flags: OrderFlags { post_only: false },
    });

    write_json_cmd(&mut s, &buy).await?;

    // Read until ACK for client_seq=1, collecting everything.
    let events = read_until_ack_json(&mut s, 1).await?;

    let saw_fill = events.iter().any(|ev| match ev {
        Event::Fill(f) => f.taker_order_id == 9001 && f.qty >= 1,
        _ => false,
    });

    if !saw_fill {
        anyhow::bail!(
            "smoke-replay failed: no fill observed (book likely not replayed). Events:\n{:#?}",
            events
        );
    }

    println!("smoke-replay events:\n{:#?}", events);
    Ok(())
}

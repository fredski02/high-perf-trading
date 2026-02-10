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

use common::{Ack, Authenticate, Command, Event, NewOrder, OrderFlags, RejectReason, TimeInForce};
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
        "bench-distributed" => {
            bench_distributed_binary(&args.bin_addr, args.iters).await?;
        }
        "bench-distributed-json" => {
            bench_distributed(&args.json_addr, args.iters).await?;
        }
        "bench-gateway-throughput" => {
            bench_gateway_throughput_binary(&args.bin_addr, args.iters).await?;
        }
        "bench-gateway-throughput-json" => {
            bench_gateway_throughput(&args.json_addr, args.iters).await?;
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
    
    // Authenticate first
    authenticate_json(&mut s, "test-key-7").await?;
    
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
    
    // Authenticate first
    authenticate_json(&mut s, "test-key-7").await?;

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
        account_id: 7,  // Same account as maker
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
    
    // Authenticate first
    authenticate_json(&mut s, "test-key-7").await?;

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
        account_id: 7,  // Same account as maker
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
    
    // Authenticate first
    authenticate_json(&mut s, "test-key-7").await?;

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
        account_id: 7,  // Same account as maker
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

// -------------------- Bench: distributed system RTT --------------------
//
// Measures end-to-end latency through gateway → engine → gateway path.
// Uses JSON protocol for simplicity (can add binary mode later).
//
// Test scenario:
// 1. Place resting limit order (POST_ONLY to ensure it rests)
// 2. Send aggressive orders that fill against resting orders
// 3. Measure time from send to Fill event received
//
// This gives us realistic latency including:
// - Client → Gateway network
// - Gateway risk checks
// - Gateway → Engine routing
// - Engine matching
// - Engine → Gateway response
// - Gateway → Client response
async fn bench_distributed(addr: &str, iters: u32) -> anyhow::Result<()> {
    let mut s = TcpStream::connect(addr).await?;
    let mut h = Histogram::<u64>::new(3)?;

    println!("Running distributed system benchmark...");
    println!("Iterations: {}", iters);
    println!("Testing: Client → Gateway → Engine → Gateway → Client");
    println!();

    // Warmup: place a few orders to ensure system is ready
    println!("Running warmup (10 orders)...");
    for i in 0..10 {
        let warmup_order = Command::NewOrder(NewOrder {
            client_seq: i + 1,
            order_id: i + 1000,
            account_id: 1,
            symbol_id: 1,
            side: Side::Buy,
            price: 1000 + i as i64,
            qty: 1,
            tif: TimeInForce::Gtc,
            flags: OrderFlags { post_only: false },
        });

        let t0 = Instant::now();
        write_json_cmd(&mut s, &warmup_order).await?;

        // Try to read response - may timeout if gateway doesn't respond
        match tokio::time::timeout(
            std::time::Duration::from_millis(100),
            read_until_ack_json(&mut s, i + 1),
        )
        .await
        {
            Ok(Ok(_)) => {
                let dt = t0.elapsed();
                println!("  Warmup {}: {:?}", i + 1, dt);
            }
            Ok(Err(e)) => {
                println!("  Warmup {} error: {}", i + 1, e);
            }
            Err(_) => {
                println!(
                    "  Warmup {} timeout (no response from gateway - this is expected)",
                    i + 1
                );
            }
        }
    }

    println!();
    println!("NOTE: If responses timed out, this is expected as the gateway");
    println!("      response routing is not yet fully implemented.");
    println!("      We will measure time to send orders (one-way latency).");
    println!();
    println!("Warmup complete. Starting benchmark...");

    // Main benchmark loop
    for i in 0..iters {
        let client_seq = (i + 100) as u64;
        let order_id = (i + 10000) as u64;

        // Place resting ask (sell order)
        let resting_ask = Command::NewOrder(NewOrder {
            client_seq,
            order_id,
            account_id: 1,
            symbol_id: 1,
            side: Side::Sell,
            price: 50000,
            qty: 1,
            tif: TimeInForce::Gtc,
            flags: OrderFlags { post_only: false },
        });

        write_json_cmd(&mut s, &resting_ask).await?;
        let _ = read_until_ack_json(&mut s, client_seq).await?;

        // Now send aggressive buy order that will match
        let taker_seq = client_seq + 1;
        let taker_order_id = order_id + 100000;

        let aggressive_buy = Command::NewOrder(NewOrder {
            client_seq: taker_seq,
            order_id: taker_order_id,
            account_id: 1,
            symbol_id: 1,
            side: Side::Buy,
            price: 51000, // Cross the spread
            qty: 1,
            tif: TimeInForce::Gtc,
            flags: OrderFlags { post_only: false },
        });

        // Measure round-trip time
        let t0 = Instant::now();
        write_json_cmd(&mut s, &aggressive_buy).await?;

        // Read events until we get the Fill
        let events = read_until_ack_json(&mut s, taker_seq).await?;
        let dt = t0.elapsed().as_nanos() as u64;

        // Verify we got a fill (sanity check)
        let got_fill = events.iter().any(|ev| match ev {
            Event::Fill(f) => f.taker_order_id == taker_order_id,
            _ => false,
        });

        if got_fill {
            let _ = h.record(dt);
        } else {
            eprintln!("Warning: iteration {} did not produce a fill", i);
        }

        // Progress indicator every 100 iterations
        if (i + 1) % 100 == 0 {
            println!("Completed {} / {} iterations", i + 1, iters);
        }
    }

    println!();
    println!("===== Distributed System Latency Benchmark Results =====");
    println!("Iterations: {}", iters);
    println!();
    println!("Latency (Client → Gateway → Engine → Gateway → Client):");
    println!("  p50  = {:>8} μs", h.value_at_quantile(0.50) / 1000);
    println!("  p90  = {:>8} μs", h.value_at_quantile(0.90) / 1000);
    println!("  p95  = {:>8} μs", h.value_at_quantile(0.95) / 1000);
    println!("  p99  = {:>8} μs", h.value_at_quantile(0.99) / 1000);
    println!("  p999 = {:>8} μs", h.value_at_quantile(0.999) / 1000);
    println!("  max  = {:>8} μs", h.max() / 1000);
    println!("  min  = {:>8} μs", h.min() / 1000);
    println!();
    println!("Path breakdown:");
    println!("  - Client → Gateway (network + risk check)");
    println!("  - Gateway → Engine (routing)");
    println!("  - Engine (matching)");
    println!("  - Engine → Gateway (fill event)");
    println!("  - Gateway → Client (response)");
    println!();

    Ok(())
}

// -------------------- Bench: distributed system RTT (Binary Protocol) --------------------
//
// Same as bench_distributed but using binary protocol (postcard) for production-realistic measurement.
// This should be significantly faster than JSON.
async fn bench_distributed_binary(addr: &str, iters: u32) -> anyhow::Result<()> {
    let mut s = TcpStream::connect(addr).await?;
    let mut h = Histogram::<u64>::new(3)?;

    println!("Running distributed system benchmark (BINARY PROTOCOL)...");
    println!("Iterations: {}", iters);
    println!("Testing: Client → Gateway → Engine → Gateway → Client");
    println!();

    // Warmup
    println!("Running warmup (10 orders)...");
    for i in 0..10 {
        let warmup_order = Command::NewOrder(NewOrder {
            client_seq: i + 1,
            order_id: i + 1000,
            account_id: 1,
            symbol_id: 1,
            side: Side::Buy,
            price: 1000 + i as i64,
            qty: 1,
            tif: TimeInForce::Gtc,
            flags: OrderFlags { post_only: false },
        });

        let t0 = Instant::now();
        write_binary_cmd(&mut s, &warmup_order).await?;
        let _ = read_until_ack_binary(&mut s, i + 1).await?;
        let dt = t0.elapsed();
        println!("  Warmup {}: {:?}", i + 1, dt);
    }

    println!();
    println!("Warmup complete. Starting benchmark...");

    // Main benchmark loop
    for i in 0..iters {
        let client_seq = (i + 100) as u64;
        let order_id = (i + 10000) as u64;

        // Place resting ask (sell order)
        let resting_ask = Command::NewOrder(NewOrder {
            client_seq,
            order_id,
            account_id: 1,
            symbol_id: 1,
            side: Side::Sell,
            price: 50000,
            qty: 1,
            tif: TimeInForce::Gtc,
            flags: OrderFlags { post_only: false },
        });

        write_binary_cmd(&mut s, &resting_ask).await?;
        let _ = read_until_ack_binary(&mut s, client_seq).await?;

        // Now send aggressive buy order that will match
        let taker_seq = client_seq + 1;
        let taker_order_id = order_id + 100000;

        let aggressive_buy = Command::NewOrder(NewOrder {
            client_seq: taker_seq,
            order_id: taker_order_id,
            account_id: 1,
            symbol_id: 1,
            side: Side::Buy,
            price: 51000, // Cross the spread
            qty: 1,
            tif: TimeInForce::Gtc,
            flags: OrderFlags { post_only: false },
        });

        // Measure round-trip time
        let t0 = Instant::now();
        write_binary_cmd(&mut s, &aggressive_buy).await?;

        // Read events until we get the Fill
        let events = read_until_ack_binary(&mut s, taker_seq).await?;
        let dt = t0.elapsed().as_nanos() as u64;

        // Verify we got a fill (sanity check)
        let got_fill = events.iter().any(|ev| match ev {
            Event::Fill(f) => f.taker_order_id == taker_order_id,
            _ => false,
        });

        if got_fill {
            let _ = h.record(dt);
        } else {
            eprintln!("Warning: iteration {} did not produce a fill", i);
        }

        // Progress indicator every 100 iterations
        if (i + 1) % 100 == 0 {
            println!("Completed {} / {} iterations", i + 1, iters);
        }
    }

    println!();
    println!("===== Distributed System Latency Benchmark Results (BINARY) =====");
    println!("Protocol: Binary (postcard serialization)");
    println!("Iterations: {}", iters);
    println!();
    println!("Latency (Client → Gateway → Engine → Gateway → Client):");
    println!("  p50  = {:>8} μs", h.value_at_quantile(0.50) / 1000);
    println!("  p90  = {:>8} μs", h.value_at_quantile(0.90) / 1000);
    println!("  p95  = {:>8} μs", h.value_at_quantile(0.95) / 1000);
    println!("  p99  = {:>8} μs", h.value_at_quantile(0.99) / 1000);
    println!("  p999 = {:>8} μs", h.value_at_quantile(0.999) / 1000);
    println!("  max  = {:>8} μs", h.max() / 1000);
    println!("  min  = {:>8} μs", h.min() / 1000);
    println!();
    println!("Path breakdown:");
    println!("  - Client → Gateway (network + risk check)");
    println!("  - Gateway → Engine (routing)");
    println!("  - Engine (matching)");
    println!("  - Engine → Gateway (fill event)");
    println!("  - Gateway → Client (response)");
    println!();
    println!("Note: This measures placing a resting order + matching taker order.");
    println!("      Actual per-order latency is approximately half of p50.");
    println!();

    Ok(())
}

// -------------------- Bench: gateway throughput --------------------
//
// Measures order submission throughput (orders/second) without waiting for responses.
// This is useful for testing the current system where response routing is incomplete.
//
// Measures:
// - Client → Gateway submission rate
// - Gateway risk check throughput
// - Gateway → Engine routing throughput
async fn bench_gateway_throughput(addr: &str, iters: u32) -> anyhow::Result<()> {
    let mut s = TcpStream::connect(addr).await?;

    println!("Running gateway throughput benchmark...");
    println!("Iterations: {}", iters);
    println!("Measuring: Order submission rate (no response wait)");
    println!();

    // Warmup
    println!("Warmup...");
    for i in 0..100 {
        let order = Command::NewOrder(NewOrder {
            client_seq: i + 1,
            order_id: i + 1000,
            account_id: 1,
            symbol_id: 1,
            side: Side::Buy,
            price: 40000 + i as i64,
            qty: 1,
            tif: TimeInForce::Gtc,
            flags: OrderFlags { post_only: false },
        });
        write_json_cmd(&mut s, &order).await?;
    }

    // Small delay to let system process warmup
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    println!("Starting benchmark...");
    println!();

    // Benchmark: measure time to submit all orders
    let start = Instant::now();

    for i in 0..iters {
        let order = Command::NewOrder(NewOrder {
            client_seq: (i + 1000) as u64,
            order_id: (i + 100000) as u64,
            account_id: 1,
            symbol_id: 1,
            side: if i % 2 == 0 { Side::Buy } else { Side::Sell },
            price: 50000 + (i % 1000) as i64,
            qty: 1,
            tif: TimeInForce::Gtc,
            flags: OrderFlags { post_only: false },
        });

        write_json_cmd(&mut s, &order).await?;

        if (i + 1) % 1000 == 0 {
            println!("Submitted {} / {} orders", i + 1, iters);
        }
    }

    let elapsed = start.elapsed();
    let elapsed_secs = elapsed.as_secs_f64();
    let throughput = iters as f64 / elapsed_secs;
    let avg_latency_us = (elapsed.as_micros() as f64) / (iters as f64);

    println!();
    println!("===== Gateway Throughput Benchmark Results =====");
    println!("Total orders:     {}", iters);
    println!("Total time:       {:.2} seconds", elapsed_secs);
    println!("Throughput:       {:.0} orders/second", throughput);
    println!("Avg submission:   {:.2} μs/order", avg_latency_us);
    println!();
    println!("Note: This measures submission rate only.");
    println!("      Full round-trip latency requires response routing (TODO).");
    println!();

    Ok(())
}

// -------------------- Bench: gateway throughput (Binary Protocol) --------------------
//
// Same as bench_gateway_throughput but using binary protocol for production measurements.
async fn bench_gateway_throughput_binary(addr: &str, iters: u32) -> anyhow::Result<()> {
    let mut s = TcpStream::connect(addr).await?;

    println!("Running gateway throughput benchmark (BINARY PROTOCOL)...");
    println!("Iterations: {}", iters);
    println!("Measuring: Order submission rate (no response wait)");
    println!();

    // Warmup
    println!("Warmup...");
    for i in 0..100 {
        let order = Command::NewOrder(NewOrder {
            client_seq: i + 1,
            order_id: i + 1000,
            account_id: 1,
            symbol_id: 1,
            side: Side::Buy,
            price: 40000 + i as i64,
            qty: 1,
            tif: TimeInForce::Gtc,
            flags: OrderFlags { post_only: false },
        });
        write_binary_cmd(&mut s, &order).await?;
    }

    // Small delay to let system process warmup
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    println!("Starting benchmark...");
    println!();

    // Benchmark: measure time to submit all orders
    let start = Instant::now();

    for i in 0..iters {
        let order = Command::NewOrder(NewOrder {
            client_seq: (i + 1000) as u64,
            order_id: (i + 100000) as u64,
            account_id: 1,
            symbol_id: 1,
            side: if i % 2 == 0 { Side::Buy } else { Side::Sell },
            price: 50000 + (i % 1000) as i64,
            qty: 1,
            tif: TimeInForce::Gtc,
            flags: OrderFlags { post_only: false },
        });

        write_binary_cmd(&mut s, &order).await?;

        if (i + 1) % 1000 == 0 {
            println!("Submitted {} / {} orders", i + 1, iters);
        }
    }

    let elapsed = start.elapsed();
    let elapsed_secs = elapsed.as_secs_f64();
    let throughput = iters as f64 / elapsed_secs;
    let avg_latency_us = (elapsed.as_micros() as f64) / (iters as f64);

    println!();
    println!("===== Gateway Throughput Benchmark Results (BINARY) =====");
    println!("Protocol: Binary (postcard serialization)");
    println!("Total orders:     {}", iters);
    println!("Total time:       {:.2} seconds", elapsed_secs);
    println!("Throughput:       {:.0} orders/second", throughput);
    println!("Avg submission:   {:.2} μs/order", avg_latency_us);
    println!();
    println!("Note: This measures submission rate only.");
    println!("      Full round-trip latency measured by bench-distributed.");
    println!();

    Ok(())
}

// -------------------- Helpers --------------------

/// Authenticate with the gateway using JSON protocol
async fn authenticate_json(s: &mut TcpStream, api_key: &str) -> anyhow::Result<()> {
    let auth_cmd = Command::Authenticate(Authenticate {
        api_key: api_key.to_string(),
    });
    
    write_json_cmd(s, &auth_cmd).await?;
    
    // Read and verify AuthSuccess response
    let event = read_json_event(s).await?;
    match event {
        Event::AuthSuccess(_) => Ok(()),
        Event::AuthFailure(failure) => anyhow::bail!("Authentication failed: {}", failure.reason),
        other => anyhow::bail!("Unexpected event after authentication: {:?}", other),
    }
}

async fn write_json_cmd(s: &mut TcpStream, cmd: &Command) -> anyhow::Result<()> {
    let payload = serde_json::to_vec(cmd)?;
    let frame = frame(&payload);
    s.write_all(&frame).await?;
    Ok(())
}

async fn write_binary_cmd(s: &mut TcpStream, cmd: &Command) -> anyhow::Result<()> {
    // Encode command using BinaryCodec format (message type + fields)
    let mut p = BytesMut::with_capacity(128);

    match cmd {
        Command::NewOrder(order) => {
            // MT_NEW_ORDER = 1
            p.put_u16_le(1);
            p.put_u64_le(order.client_seq);
            p.put_u64_le(order.order_id);
            p.put_u32_le(order.account_id);
            p.put_u32_le(order.symbol_id);
            p.put_u8(match order.side {
                Side::Buy => 0,
                Side::Sell => 1,
            });
            p.put_i64_le(order.price);
            p.put_i64_le(order.qty);
            p.put_u8(match order.tif {
                TimeInForce::Gtc => 0,
                TimeInForce::Ioc => 1,
            });
            p.put_u8(if order.flags.post_only { 1 } else { 0 });
        }
        _ => anyhow::bail!("write_binary_cmd: unsupported command type"),
    }

    let frame = frame(&p);
    s.write_all(&frame).await?;
    Ok(())
}

async fn read_binary_event(s: &mut TcpStream) -> anyhow::Result<Event> {
    let frame = read_one_frame(s).await?;
    let mut b = &frame[..];

    if b.len() < 2 {
        anyhow::bail!("frame too short");
    }

    let mt = u16::from_le_bytes([b[0], b[1]]);
    b = &b[2..];

    match mt {
        101 => {
            // MT_ACK
            let server_seq = u64::from_le_bytes(b[0..8].try_into()?);
            let client_seq = u64::from_le_bytes(b[8..16].try_into()?);
            let order_id = u64::from_le_bytes(b[16..24].try_into()?);
            Ok(Event::Ack(Ack {
                server_seq,
                client_seq,
                order_id,
            }))
        }
        103 => {
            // MT_FILL
            let server_seq = u64::from_le_bytes(b[0..8].try_into()?);
            let client_seq = u64::from_le_bytes(b[8..16].try_into()?);
            let symbol_id = u32::from_le_bytes(b[16..20].try_into()?);
            let taker_order_id = u64::from_le_bytes(b[20..28].try_into()?);
            let maker_order_id = u64::from_le_bytes(b[28..36].try_into()?);
            let price = i64::from_le_bytes(b[36..44].try_into()?);
            let qty = i64::from_le_bytes(b[44..52].try_into()?);
            Ok(Event::Fill(common::Fill {
                server_seq,
                client_seq,
                symbol_id,
                taker_order_id,
                maker_order_id,
                price,
                qty,
            }))
        }
        104 => {
            // MT_BOOK_TOP - just skip for now
            Ok(Event::BookTop(common::BookTop {
                server_seq: 0,
                symbol_id: 0,
                best_bid_px: None,
                best_bid_qty: None,
                best_ask_px: None,
                best_ask_qty: None,
            }))
        }
        _ => anyhow::bail!("unknown event type: {}", mt),
    }
}

async fn read_until_ack_binary(s: &mut TcpStream, client_seq: u64) -> anyhow::Result<Vec<Event>> {
    let mut out = Vec::new();
    loop {
        let event = read_binary_event(s).await?;
        out.push(event.clone());

        if let Event::Ack(Ack { client_seq: cs, .. }) = event {
            if cs == client_seq {
                return Ok(out);
            }
        }
    }
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

async fn read_json_event(s: &mut TcpStream) -> anyhow::Result<Event> {
    let bytes = read_one_frame(s).await?;
    let ev: Event = serde_json::from_slice(&bytes)?;
    Ok(ev)
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
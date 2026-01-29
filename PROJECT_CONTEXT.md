# High-Perf Trading Engine (Rust)

Low-latency single-symbol exchange core written in Rust.
Goal: realistic matching engine + gateway similar to real trading venues.

---

## Architecture

Workspace crates:

- common
  Protocol types (Command/Event), serde models, shared structs

- engine
  Single-threaded matching engine
  Owns order book
  Runs on dedicated OS thread (NOT tokio)

- gateway
  Tokio TCP server
  - binary port (fast path)
  - JSON port (debug path)
  Routes inbound commands → engine via bounded channel
  Routes outbound events → clients via router

- admin_http
  Axum server (in-process)
  /health
  /metrics (Prometheus text)

- persistence
  Append-only journal using postcard
  Replays commands on startup to rebuild book

---

## Engine Model

- 1 server = 1 symbol = 1 order book
- Single thread → no locks in hot path
- Price-time FIFO matching
- Data structures:
  - BTreeMap<Price, Level>
  - Slab<Order>
  - VecDeque for FIFO queues

---

## Supported Order Types

- GTC
- IOC
- Post-only
- Cancel
- Replace

Events:
- Ack
- Fill
- BookTop
- Reject

---

## Networking

Protocol:
  [u32 len][payload]

Codecs:
  - JSON (serde)
  - Binary (manual little-endian)

Flow:
  socket → decode → journal append → engine queue → match → events → router → socket

---

## Metrics

Prometheus-style:

- exchange_connections
- exchange_frames_in
- exchange_frames_out
- exchange_fills_total
- exchange_rejects_total
- exchange_engine_in_queue_depth

---

## Persistence

Journal:
  [u32 len][postcard(Command)]

Startup:
  read journal → engine.replay(cmds) → run()

Guarantee:
  restart restores identical orderbook

Snapshots not implemented yet.

---

## Testing

bench crate:

Modes:
- smoke-match
- smoke-postonly
- smoke-ioc
- smoke-replay
- bench-bin

justfile:
- just dev
- just smoke
- just metrics
- just replay-test

---

## Constraints / Design Goals

- low latency
- deterministic
- minimal allocations in hot path
- simple correctness first
- one symbol per process (for now)

---

## Next Tasks

(EDIT THIS EACH SESSION)

- snapshots for faster recovery
- fsync batching
- risk checks
- multi-symbol sharding
- latency benchmarking


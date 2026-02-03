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
  Owns order book AND journal (no locking needed)
  Runs on dedicated OS thread (NOT tokio)
  Handles snapshots and persistence

- server
  Combined TCP + HTTP server
  - Binary protocol port (9000) - fast path
  - JSON protocol port (9001) - debug path  
  - Admin HTTP port (8080) - health + metrics
  Routes inbound commands → engine via bounded channel
  Routes outbound events → clients via router
  Metrics: Prometheus text format at /metrics
  Includes admin HTTP server (Axum) for /health and /metrics endpoints

- persistence
  Append-only journal with CRC32 checksums
  Snapshot support for fast recovery
  Journal rotation to prevent unbounded growth
  Configurable fsync batching

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
  socket → decode → engine queue → journal append → match → events → router → socket

---

## Metrics

Prometheus-style:

Core metrics:
- exchange_connections
- exchange_frames_in
- exchange_frames_out
- exchange_fills_total
- exchange_rejects_total
- exchange_engine_in_queue_depth

Persistence metrics:
- exchange_journal_appends_total
- exchange_journal_flushes_total
- exchange_journal_errors_total
- exchange_snapshots_total
- exchange_journal_rotations_total

---

## Persistence (✅ IMPLEMENTED)

### Architecture
- Engine owns journal (single-threaded, no locks)
- Commands journaled BEFORE processing (deterministic replay)
- Periodic snapshots for fast recovery
- CRC32 checksums for corruption detection

### Journal Format
Frame: [u32 len][postcard(Command)][u32 crc32]
- Append-only with fsync batching
- Configurable durability vs. latency tradeoff
- Automatic rotation after N commands (default: 1M)

### Snapshot Format
Frame: [u64 sequence][u32 len][serialized order book][u32 crc32]
- Periodic snapshots (default: every 100k commands)
- Automatic cleanup (keeps last 3)
- Stores full order book state

### Recovery Process
1. Load latest snapshot (if exists)
2. Restore order book from snapshot
3. Replay journal commands after snapshot sequence
4. Continue processing

### Configuration (CLI args)
```
--journal-path <PATH>           (default: journal.bin)
--snapshot-dir <DIR>            (default: snapshots)
--journal-batch-size <N>        (default: 100)
--snapshot-interval <N>         (default: 100000)
```

### Performance Tuning
- Low latency: batch_size=10-50, frequent fsync
- High throughput: batch_size=100-1000, less frequent fsync
- Fast recovery: smaller snapshot_interval

### Files Created
- `journal.bin` - Current journal
- `journal_<timestamp>.bin` - Rotated backups
- `snapshots/snapshot_<sequence>.bin` - Snapshot files

---

## Testing

bench crate modes:
- smoke-match - Order matching with fills
- smoke-postonly - Post-only order rejection
- smoke-ioc - IOC order behavior
- smoke-replay - Verify persistence replay
- bench-bin - Binary protocol RTT benchmark

justfile recipes:
- just dev - Run server in dev mode
- just dev-fast-snapshot - Dev with aggressive snapshotting
- just dev-high-throughput - Dev with large batches
- just smoke - Run all smoke tests
- just test-persistence - Full persistence cycle test
- just show-persistence - Show journal/snapshot files
- just metrics - Show all metrics
- just metrics-persistence - Show persistence metrics only
- just replay-test - Test journal replay
- just check - Run all quality checks (fmt, clippy, test)
- just pre-commit - Quick check before git commit

---

## Constraints / Design Goals

- low latency
- deterministic
- minimal allocations in hot path
- simple correctness first
- one symbol per process (for now)
- production-ready durability

---

## Completed Features ✅

- ✅ Append-only journal with postcard serialization
- ✅ CRC32 checksums for data integrity
- ✅ Configurable fsync batching (latency vs. durability)
- ✅ Snapshot support for fast recovery
- ✅ Journal rotation to prevent unbounded growth
- ✅ Engine owns persistence (no gateway locking)
- ✅ Full Prometheus metrics for monitoring
- ✅ Deterministic replay guarantees

---

## Risk Management Roadmap

### Phase 1: In-Memory Positions + Snapshot ✅ COMPLETE
**Status**: Fully implemented and tested (13 unit tests passing)

**Implementation Details**:
- Position tracking per account (built from fills)
- Risk limits stored in-memory and snapshotted
- Zero latency impact (~20ns HashMap lookup overhead)
- SetRiskLimits and QueryAccount commands
- Deterministic replay (positions rebuilt from journal)
- Position tracking: net_position, avg_price, realized_pnl
- Default limits: 10k long/short, 1k order size
- Account state integrated into snapshots
- Binary and JSON codec support

**Files Modified**:
- `crates/engine/src/account_manager.rs` (new module, 8 tests)
- `crates/engine/src/engine.rs` (risk checks, position updates)
- `crates/engine/src/order_book.rs` (added maker_account_id to MatchFill)
- `crates/common/src/types.rs` (Position, RiskLimits, new commands/events)
- `crates/codecs/src/binary.rs` (protocol support)
- `crates/server/src/gateway.rs` (command routing)

### Phase 2: Dynamic Risk Limits (FUTURE)
- Move risk limits to config file (risk_limits.toml)
- Add admin API: `POST /admin/accounts/{id}/risk-limits`
- Support SIGHUP reload without restart
- Still zero hot-path latency
- Estimated: 2-4 hours implementation

### Phase 3: External Position Sync (FUTURE)
- Keep hot path 100% in-memory (0µs latency)
- Background async sync to database (PostgreSQL/Redis)
- Positions queryable externally with ~1s lag
- Enables: monitoring dashboards, compliance reporting
- Database only for analytics, NOT hot path
- Estimated: 1 day implementation

---

## Next Tasks

(EDIT THIS EACH SESSION)

- **[COMPLETE]** Phase 1: Risk management (position tracking + limits)
- **[NEXT]** Phase 2: Dynamic risk limits via config file + admin API
- **[FUTURE]** Phase 3: External position sync to database for analytics
- multi-symbol sharding
- latency benchmarking (p50/p99/p999 in metrics endpoint)
- async fsync worker thread (optional optimization)
- compression for snapshots (optional)
- direct I/O for ultra-low latency (optional)
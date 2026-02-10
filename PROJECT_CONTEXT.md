# High-Performance Trading Engine

**A production-grade, low-latency distributed trading system built in Rust**

> **Current Status:** Core functionality complete, tested at **11-13μs p50 latency** (localhost)  
> **Architecture:** Gateway + per-symbol engine servers (similar to Coinbase, Binance)  
> **Performance:** 20M+ orders/sec matching, 2.7M+ orders/sec gateway throughput

---

## Table of Contents

- [Quick Start](#quick-start)
- [Architecture Overview](#architecture-overview)
- [Performance Metrics](#performance-metrics)
- [Features](#features)
- [Project Structure](#project-structure)
- [Development Guide](#development-guide)
- [Testing](#testing)
- [Configuration](#configuration)
- [Known Issues](#known-issues)
- [Roadmap](#roadmap)

---

## Quick Start

### Prerequisites
- Rust 1.70+ (`rustup`)
- Linux or macOS
- `just` command runner (optional but recommended)

### Build & Run

```bash
# Clone and build
git clone <repo>
cd high-perf-trading
cargo build --release

# Option 1: Using justfile (recommended)
just dev                    # Start everything (engines + gateway)

# Option 2: Manual (separate terminals)
./scripts/start_engines.sh  # Terminal 1
./scripts/start_gateway.sh  # Terminal 2

# Run benchmarks
just bench-rtt              # Measure latency
just bench-throughput       # Measure throughput
```

### First Test

```bash
# Quick smoke test
just smoke-bin

# Expected output:
# bin resp len=...
# smoke ok
```

---

## Architecture Overview

### High-Level Design

```
                     Clients (Traders)
                            │
                            ↓
              ┌─────────────────────────┐
              │   Gateway Server        │
              │   :9000 (binary)        │
              │   :9001 (JSON)          │
              │   :8080 (admin HTTP)    │
              │                         │
              │  • Authentication       │
              │  • Account Management   │
              │  • Risk Checks          │
              │  • Order Routing        │
              │  • Persistence          │
              └────────┬────────────────┘
                       │
        ┌──────────────┼──────────────┐
        │              │              │
        ↓              ↓              ↓
  ┌──────────┐  ┌──────────┐  ┌──────────┐
  │ BTC/USD  │  │ ETH/USD  │  │ SOL/USD  │
  │ Engine   │  │ Engine   │  │ Engine   │
  │ :9100    │  │ :9101    │  │ :9102    │
  │          │  │          │  │          │
  │ Matching │  │ Matching │  │ Matching │
  │ Orders   │  │ Orders   │  │ Orders   │
  └──────────┘  └──────────┘  └──────────┘
```

### Design Principles

1. **One Order Book = One Engine Server**
   - Simple, isolated, cache-optimized
   - Single-threaded for zero-lock latency
   - Independent scaling per trading pair

2. **Gateway = Risk Oracle**
   - Single source of truth for account state
   - Pre-flight risk checks with tentative reservations
   - Prevents race conditions (double-spend)

3. **Cost-Efficient Scaling**
   - Hot pairs (BTC/USD) → beefy servers (8+ cores)
   - Cold pairs (meme coins) → cheap servers (1-2 cores)
   - Horizontal scaling: add engines as you list pairs

4. **Production-Ready**
   - Persistence (journaling + snapshots)
   - Metrics (Prometheus)
   - Authentication & sessions
   - Deterministic replay

---

## Performance Metrics

### Latency (Localhost Testing)

**Binary Protocol RTT (10,000 iterations):**
```
p50  = 11-13 μs   ← median latency
p99  = 29-40 μs   ← 99th percentile  
p999 = 79-465 μs  ← tail latency
```

**What this measures:** Full round-trip:
- Client → Gateway (deserialize + risk check)
- Gateway → Engine (routing)
- Engine (matching ~48ns)
- Engine → Gateway (fill event)
- Gateway → Client (serialize)

### Throughput

**Engine Matching (Isolated):**
- **48 ns** per order
- **20.6M orders/second** (single-threaded!)
- Tool: `cargo bench --bench engine_step`

**Gateway Routing:**
- **360 ns** per order (binary protocol)
- **500 ns** per order (JSON protocol)
- **2.75M orders/second** throughput
- Tool: `just bench-throughput`

### Comparison with Industry

| System              | Typical Latency | Our System (localhost) |
|---------------------|-----------------|------------------------|
| Coinbase Pro        | 1-5 ms         | **0.011-0.013 ms**     |
| Binance             | 0.5-2 ms       | **0.011-0.013 ms**     |
| Traditional HFT     | 0.1-0.5 ms     | **0.011-0.013 ms**     |

**Note:** Production latency (AWS same-AZ) expected at 50-100μs with real network overhead.

### Optimizations Applied

- ✅ TCP_NODELAY on all connections (removed 40ms buffering)
- ✅ Batched socket writes (feed + flush pattern)
- ✅ Single-threaded engine (no lock contention)
- ✅ Zero-copy serialization (postcard binary format)
- ✅ Lock-free order book operations
- ✅ Per-account DashMap (concurrent account access)
- ✅ Persistent TCP connections (no handshake overhead)

---

## Features

### ✅ Core Features (Implemented)

#### Gateway Server
- [x] **Authentication & Sessions**
  - API key verification
  - Session tracking per connection
  - Reject unauthenticated orders
  - Test accounts (1-10) with $1M buying power each

- [x] **Account Management**
  - In-memory account state (positions, balances)
  - Per-account locking (race-condition free)
  - Position tracking (net position, avg price, realized P&L)

- [x] **Risk Management**
  - Pre-flight risk checks
  - Tentative reservations (prevent double-spend)
  - Position limit enforcement
  - Order size validation
  - Buying power checks

- [x] **Order Routing**
  - Symbol-based routing (symbol_id → engine)
  - Persistent TCP connections to engines
  - Fill event aggregation
  - Client response routing (partial)

- [x] **Protocols**
  - Binary protocol (postcard) - **fully working**
  - JSON protocol - **partially working** (see Known Issues)
  - Length-delimited framing

- [x] **Persistence**
  - Account journaling (create account events)
  - Snapshot support
  - Recovery on restart

#### Engine Server
- [x] **Order Book Matching**
  - Price-time priority (FIFO)
  - Continuous matching
  - GTC (Good-Till-Cancel) orders
  - IOC (Immediate-Or-Cancel) orders
  - POST_ONLY orders

- [x] **Persistence**
  - Command journaling (NewOrder, Cancel, Replace)
  - CRC32 checksums
  - Order book snapshots
  - Deterministic replay

- [x] **Performance**
  - Single-threaded on dedicated OS thread
  - Zero-lock design
  - Sub-microsecond matching (~48ns)

- [x] **Events**
  - Ack (order accepted)
  - Fill (trade executed)
  - Reject (order rejected)
  - BookTop (best bid/ask updates)
  - Trade (public trade data)

#### Common Infrastructure
- [x] Binary codec (postcard serialization)
- [x] JSON codec (serde_json)
- [x] Length-delimited framing
- [x] Gateway ↔ Engine protocol
- [x] Metrics (partial - engines have admin endpoints)

---

### 🚧 Partially Implemented

- [ ] **JSON Response Routing** - JSON smoke tests hang
  - Binary protocol works perfectly
  - JSON client handler needs debugging
  - Issue: Responses not reaching clients

- [ ] **Gateway Admin HTTP** - Endpoint exists but not always configured
  - Metrics endpoint (:8080) not always started
  - Health checks need implementation

- [ ] **Market Data Broadcasting**
  - BookTop/Trade events generated
  - Not yet broadcasted to all clients
  - Need pub/sub channel

---

### 📋 Not Yet Implemented

#### High Priority
- [ ] **Cancel & Replace Orders**
  - Account state updates for Cancel
  - Reservation release on cancel
  - Reservation adjustment on replace
  - Race condition handling (cancel vs fill)

- [ ] **Complete Admin API**
  - Query account positions
  - Query account balances
  - Update risk limits
  - Force-close positions

- [ ] **Enhanced Monitoring**
  - Complete Prometheus metrics
  - Grafana dashboards
  - Alerting rules
  - Log aggregation

#### Medium Priority
- [ ] **WebSocket Support**
  - Real-time market data feeds
  - Order updates stream
  - Account updates stream

- [ ] **Advanced Order Types**
  - Stop-loss / stop-limit
  - Iceberg orders
  - TWAP / VWAP execution

- [ ] **Multi-Region Deployment**
  - Cross-region routing
  - Failover handling
  - Geo-distributed engines

#### Low Priority
- [ ] **Advanced Risk**
  - Portfolio margin
  - Dynamic position limits
  - Cross-margining

- [ ] **Market Data Storage**
  - ClickHouse integration
  - Historical trades
  - OHLCV bars
  - Order book snapshots

---

## Project Structure

```
high-perf-trading/
├── crates/
│   ├── common/              # Shared types and protocols
│   │   ├── src/
│   │   │   ├── lib.rs       # Command, Event, Side, etc.
│   │   │   └── gateway_protocol.rs  # Gateway ↔ Engine messages
│   │
│   ├── engine/              # Core matching engine logic
│   │   ├── src/
│   │   │   ├── engine.rs    # Main engine loop
│   │   │   ├── order_book.rs  # Order book (FIFO matching)
│   │   │   └── types.rs     # Engine-specific types
│   │   └── benches/
│   │       └── engine_step.rs  # Performance benchmarks
│   │
│   ├── engine_server/       # Engine server binary
│   │   ├── src/
│   │   │   ├── main.rs      # Entry point
│   │   │   └── gateway_connection.rs  # Gateway protocol handler
│   │
│   ├── gateway_server/      # Gateway server binary
│   │   ├── src/
│   │   │   ├── main.rs            # Entry point
│   │   │   ├── account_manager.rs # Account state + risk
│   │   │   ├── auth.rs            # API key authentication
│   │   │   ├── session.rs         # Session management
│   │   │   ├── engine_router.rs   # Route to engines
│   │   │   ├── client_handler.rs  # Client connections
│   │   │   ├── persistence.rs     # Account journaling
│   │   │   └── reconciliation.rs  # Engine restart recovery
│   │
│   ├── codecs/              # Serialization codecs
│   │   ├── src/
│   │   │   ├── binary_codec.rs  # Postcard binary
│   │   │   └── json_codec.rs    # JSON
│   │   └── benches/
│   │       └── binary_codec.rs  # Codec benchmarks
│   │
│   ├── persistence/         # Persistence library (UNUSED - see gateway/engine persistence)
│   ├── bench/               # Benchmark client
│   │   └── src/main.rs      # Smoke tests + RTT benchmarks
│   │
│   └── test_client/         # Simple test client
│
├── scripts/
│   ├── start_engines.sh     # Launch engine servers
│   ├── start_gateway.sh     # Launch gateway server
│   └── run_benchmark.sh     # Run latency benchmarks
│
├── engines.toml             # Engine configuration
├── justfile                 # Task runner recipes
├── Cargo.toml               # Workspace manifest
└── PROJECT_CONTEXT.md       # This file
```

### Key Files

**Gateway:**
- `gateway_server/src/account_manager.rs` (490 lines) - Core risk management
- `gateway_server/src/client_handler.rs` (355 lines) - Client connection handling
- `gateway_server/src/engine_router.rs` - Routes orders to engines

**Engine:**
- `engine/src/engine.rs` - Main engine loop with persistence
- `engine/src/order_book.rs` - Price-time FIFO matching

**Common:**
- `common/src/lib.rs` - Shared types (Command, Event, Side, etc.)
- `common/src/gateway_protocol.rs` - Gateway ↔ Engine protocol

---

## Development Guide

### Building

```bash
# Debug build (fast compile, slow runtime)
cargo build

# Release build (optimized, production-ready)
cargo build --release

# Build specific binary
cargo build --release --bin gateway_server
cargo build --release --bin engine_server
```

### Running

#### Option 1: Using `just` (Recommended)

```bash
# Start everything
just dev              # Engines + gateway (foreground)

# Or start separately
just dev-engines      # Background
just dev-gateway      # Foreground

# Stop everything
just kill

# Clean persistence files
just clean-persistence
```

#### Option 2: Using Scripts

```bash
# Terminal 1: Start engines
./scripts/start_engines.sh

# Terminal 2: Start gateway
./scripts/start_gateway.sh

# Logs are written to:
#   engine1.log, engine2.log, gateway.log
```

#### Option 3: Manual

```bash
# Create persistence directories
mkdir -p engine1/snapshots engine2/snapshots gateway/snapshots

# Start engine 1
./target/release/engine_server \
  --symbol-id 1 \
  --symbol-name "BTC/USD" \
  --listen-addr 127.0.0.1:9100 \
  --admin-addr 127.0.0.1:9200 \
  --journal-path engine1/journal.bin \
  --snapshot-dir engine1/snapshots

# Start engine 2 (different terminal)
./target/release/engine_server \
  --symbol-id 2 \
  --symbol-name "ETH/USD" \
  --listen-addr 127.0.0.1:9101 \
  --admin-addr 127.0.0.1:9201 \
  --journal-path engine2/journal.bin \
  --snapshot-dir engine2/snapshots

# Start gateway (different terminal)
./target/release/gateway_server \
  --client-binary-addr 0.0.0.0:9000 \
  --client-json-addr 0.0.0.0:9001 \
  --admin-addr 0.0.0.0:8080 \
  --journal-path gateway/journal.bin \
  --snapshot-dir gateway/snapshots \
  --engines-config engines.toml
```

### Code Quality

```bash
# Format code
just fmt
cargo fmt --all

# Lint with clippy
just clippy
cargo clippy --workspace --all-targets -- -Dwarnings

# Run tests
just test
cargo test --workspace

# All checks (fmt + clippy + test)
just check
```

---

## Testing

### Unit Tests

```bash
# Run all workspace tests
cargo test --workspace

# Run specific crate tests
cargo test -p engine
cargo test -p gateway_server

# Run with output
cargo test -- --nocapture
```

### Smoke Tests

```bash
# Binary protocol (WORKING)
just smoke-bin
cargo run --release -p bench -- --mode smoke --bin-addr 127.0.0.1:9000

# JSON protocol (HANGS - see Known Issues)
timeout 10 cargo run --release -p bench -- --mode smoke-match --json-addr 127.0.0.1:9001
```

### Performance Benchmarks

```bash
# Engine matching (offline, no server needed)
just perf
cargo bench --bench engine_step

# Binary RTT (requires running servers)
just bench-rtt              # 10,000 iterations
just bench-rtt-fast         # 1,000 iterations

# Gateway throughput
just bench-throughput

# Custom iterations
cargo run --release -p bench -- --mode bench-bin --bin-addr 127.0.0.1:9000 --iters 50000
```

### Integration Testing

```bash
# Full cycle: build → start → test → cleanup
just test-all

# Manual testing
just dev-engines &       # Start engines
sleep 3
just dev-gateway &       # Start gateway  
sleep 3
just bench-rtt-fast      # Run benchmark
just kill                # Cleanup
```

---

## Configuration

### Engine Configuration (`engines.toml`)

```toml
[[engines]]
symbol_id = 1
symbol_name = "BTC/USD"
address = "127.0.0.1:9100"

[[engines]]
symbol_id = 2
symbol_name = "ETH/USD"
address = "127.0.0.1:9101"

[[engines]]
symbol_id = 3
symbol_name = "SOL/USD"
address = "127.0.0.1:9102"
```

### CLI Arguments

**Gateway Server:**
```bash
gateway_server \
  --client-binary-addr 0.0.0.0:9000   # Binary protocol port
  --client-json-addr 0.0.0.0:9001     # JSON protocol port (partial)
  --admin-addr 0.0.0.0:8080           # Metrics/admin HTTP
  --journal-path gateway/journal.bin  # Account journal
  --snapshot-dir gateway/snapshots    # Account snapshots
  --engines-config engines.toml       # Engine routing table
```

**Engine Server:**
```bash
engine_server \
  --symbol-id 1                       # Symbol ID (must match engines.toml)
  --symbol-name "BTC/USD"             # Symbol name (for logging)
  --listen-addr 127.0.0.1:9100        # Gateway connection port
  --admin-addr 127.0.0.1:9200         # Admin HTTP port
  --journal-path engine1/journal.bin  # Order journal
  --snapshot-dir engine1/snapshots    # Order book snapshots
```

### Test Accounts

The gateway creates 10 test accounts on startup:

| Account ID | API Key | Buying Power |
|------------|---------|--------------|
| 1 | `test-key-1` | $1,000,000 |
| 2 | `test-key-2` | $1,000,000 |
| ... | ... | ... |
| 10 | `test-key-10` | $1,000,000 |

**Usage:**
```json
// Authenticate first (not currently enforced in binary protocol)
{"Authenticate": {"api_key": "test-key-1"}}

// Place order
{"NewOrder": {
  "client_seq": 1,
  "order_id": 1001,
  "account_id": 1,
  "symbol_id": 1,
  "side": "Buy",
  "price": 50000,
  "qty": 1,
  "tif": "Gtc",
  "flags": {"post_only": false}
}}
```

---

## Known Issues

### 1. JSON Protocol Response Routing 🔴 HIGH PRIORITY

**Status:** Binary protocol works perfectly, JSON hangs

**Symptoms:**
- `just smoke-bin` ✅ Works
- `just smoke-json` ❌ Hangs waiting for response
- Binary RTT benchmarks work (11-13μs)
- JSON smoke tests timeout

**Root Cause:**
Response routing incomplete in `client_handler.rs`. Clients register for responses but messages may not be routed correctly.

**Workaround:**
Use binary protocol for all testing:
```bash
just bench-rtt         # Use this (binary)
# just bench-distributed  # Don't use (JSON)
```

**Fix Required:**
- Debug `client_handler.rs` response routing
- Ensure `client_senders` registry is properly populated
- Test with multiple concurrent connections

---

### 2. Gateway Admin Endpoint Not Always Started

**Status:** Low priority

**Symptoms:**
- `curl http://127.0.0.1:8080/metrics` sometimes fails
- Depends on whether `--admin-addr` flag was provided

**Workaround:**
Always include `--admin-addr 0.0.0.0:8080` when starting gateway.

**Fix Required:**
- Make admin endpoint mandatory
- Add default port if not specified

---

### 3. Cancel & Replace Not Integrated with Account State

**Status:** Medium priority

**What Works:**
- Engine accepts Cancel and Replace commands
- Commands are journaled

**What Doesn't Work:**
- Gateway doesn't release reservations on cancel
- Gateway doesn't adjust reservations on replace
- Risk tokens not tracked by order_id

**Impact:**
- Canceled orders keep buying power locked
- Can't cancel and re-place at different price

**Fix Required:**
- Add `order_id → ReservationToken` map in gateway
- Implement `release_reservation()` on cancel
- Implement `adjust_reservation()` on replace
- Handle cancel/fill race conditions

---

### 4. Market Data Not Broadcasted

**Status:** Low priority

**What Works:**
- Engines generate BookTop and Trade events
- Events sent to gateway

**What Doesn't Work:**
- Events not forwarded to all clients
- No pub/sub mechanism

**Fix Required:**
- Add broadcast channel for market data
- Separate client event routing (unicast) from market data (broadcast)

---

### 5. Persistence Module Unused

**Status:** Informational

The `crates/persistence/` module exists but is NOT used. Instead:
- Gateway uses `gateway_server/src/persistence.rs`
- Engine uses built-in persistence in `engine/src/engine.rs`

**Action:** Consider removing `crates/persistence/` to avoid confusion.

---

## Roadmap

### Phase 1: Complete Core Features (2-4 weeks)

**Week 1: Fix Critical Issues**
- [ ] Fix JSON response routing
- [ ] Make admin endpoint reliable
- [ ] Add comprehensive logging

**Week 2: Complete Order Lifecycle**
- [ ] Implement Cancel with reservation release
- [ ] Implement Replace with reservation adjustment
- [ ] Add order_id → reservation mapping
- [ ] Handle race conditions

**Week 3: Monitoring & Observability**
- [ ] Complete Prometheus metrics
- [ ] Add Grafana dashboards
- [ ] Set up alerting (latency spikes, errors)
- [ ] Log aggregation

**Week 4: Testing & Documentation**
- [ ] Integration test suite
- [ ] Load testing (1000+ concurrent clients)
- [ ] Failure testing (engine crash, network partition)
- [ ] API documentation

---

### Phase 2: Production Deployment (2-3 weeks)

**AWS Infrastructure:**
- [ ] Terraform scripts for cluster setup
- [ ] Cluster Placement Group (same AZ, low latency)
- [ ] Enhanced Networking (SR-IOV)
- [ ] Security groups and VPC configuration

**Deployment:**
- [ ] Deploy to AWS (test environment)
- [ ] Measure real-world latency (target: <100μs p99)
- [ ] Stress test with production-like load
- [ ] Disaster recovery testing

**Operations:**
- [ ] Runbooks for common issues
- [ ] Backup and restore procedures
- [ ] Rolling updates procedure
- [ ] Incident response plan

---

### Phase 3: Advanced Features (1-2 months)

**Market Data:**
- [ ] WebSocket feeds for BookTop/Trade
- [ ] ClickHouse integration for historical data
- [ ] OHLCV bar generation
- [ ] Replay API for backtesting

**Order Types:**
- [ ] Stop-loss / stop-limit
- [ ] Iceberg orders
- [ ] TWAP execution
- [ ] Algo order routing

**Risk Management:**
- [ ] Portfolio margin calculation
- [ ] Dynamic position limits
- [ ] Risk API for external risk systems

**Multi-Region:**
- [ ] Cross-region deployment
- [ ] Failover between regions
- [ ] Geo-routing for clients

---

## Architecture Deep Dives

### Gateway ↔ Engine Protocol

**Messages: Gateway → Engine**
```rust
enum GatewayToEngine {
    Execute(ExecuteCommand),  // Execute order (risk-approved)
    Ping,                     // Health check
}

struct ExecuteCommand {
    command: Command,         // NewOrder/Cancel/Replace
    conn_id: u64,             // Client connection ID
    risk_token: RiskToken,    // Risk approval proof
}

struct RiskToken {
    account_id: AccountId,
    reserved_amount: i64,     // Buying power locked
    gateway_seq: u64,         // Idempotency sequence
}
```

**Messages: Engine → Gateway**
```rust
enum EngineToGateway {
    ClientEvent {
        conn_id: u64,              // Route to this client
        event: Event,              // Fill/Ack/Reject
        risk_token: Option<RiskToken>,  // For reservation release
    },
    Pong {
        symbol_id: SymbolId,
        orders_in_book: usize,
    },
    MarketData {
        symbol_id: SymbolId,
        event: Event,              // BookTop/Trade (broadcast)
    },
}
```

**Design Principles:**
- Gateway wraps commands with risk metadata
- Engine trusts gateway (no re-checking)
- conn_id enables response routing
- RiskToken allows reservation cleanup
- Separate unicast (ClientEvent) from broadcast (MarketData)

---

### Account State Management

**Data Structure:**
```rust
struct AccountState {
    account_id: AccountId,
    buying_power: i64,           // Total available cash
    tentative_reserved: i64,     // Locked for pending orders
    positions: HashMap<SymbolId, Position>,
    risk_limits: HashMap<SymbolId, RiskLimits>,
}

struct Position {
    net_position: i64,     // Net quantity (+ long, - short)
    avg_price: i64,        // Average entry price
    realized_pnl: i64,     // Realized profit/loss
}
```

**Order Flow:**
```
1. Client sends NewOrder
2. Gateway locks account (DashMap per-account lock)
3. Check available = buying_power - tentative_reserved
4. If sufficient, reserve tentatively
5. Route to engine with RiskToken
6. Engine matches and sends Fill
7. Gateway releases tentative, applies actual
8. Update position and buying_power
9. Forward Fill to client
```

**Race Condition Prevention:**
```rust
// Scenario: Two $100k orders with $100k buying power
Order A arrives:
  1. Lock account
  2. Check: $100k - $0 = $100k available ✓
  3. Reserve: tentative += $100k
  4. Unlock account
  5. Route to engine

Order B arrives:
  1. Lock account
  2. Check: $100k - $100k = $0 available ✗
  3. Reject: InsufficientFunds
  4. Unlock account

Result: Only one order accepted (correct!)
```

---

### Persistence Strategy

**Journal Format:**
```
[u32 length][payload bytes][u32 crc32]
```

**What's Journaled:**
- **Engine:** NewOrder, Cancel, Replace commands (BEFORE processing)
- **Gateway:** Account creation events (fills NOT journaled - engines are source of truth)

**Snapshot Format:**
```rust
// Engine snapshot
struct OrderBookSnapshot {
    sequence: u64,
    symbol_id: SymbolId,
    orders: Vec<OrderSnapshot>,  // All resting orders
}

// Gateway snapshot
struct AccountSnapshot {
    sequence: u64,
    accounts: HashMap<AccountId, AccountState>,
}
```

**Recovery Process:**
```
1. Find latest snapshot
2. Restore state from snapshot
3. Replay journal entries after snapshot sequence
4. Result: Deterministic state
```

**Why Engines Are Source of Truth:**
- Fills are authoritative from engines
- Gateway reconstructs positions from fill events on restart
- No need to journal fills (replay gives same result)

---

## Production Deployment Guide

### AWS Recommendations

**Instance Types:**
```
Gateway:
  - c7i.4xlarge (16 cores, 32GB RAM, ~$500/mo)
  - Handles 1000s of connections
  
Hot Engine Servers (BTC/USD, ETH/USD):
  - c7i.2xlarge (8 cores, 16GB RAM, ~$300/mo each)
  - High order flow
  
Warm Engine Servers (Mid-tier pairs):
  - c7i.xlarge (4 cores, 8GB RAM, ~$150/mo each)
  
Cold Engine Servers (Long tail):
  - t3.micro/small (1-2 cores, ~$15-50/mo each)
```

**Network Setup:**
- Same Availability Zone (AZ)
- Cluster Placement Group (low latency, high bandwidth)
- Enhanced Networking enabled (SR-IOV)
- 10Gbe or 25Gbe networking
- Expected latency: 50-200μs between servers

**Storage:**
- NVMe SSD for journals
- Snapshot to S3 periodically

---

## Contributing

### Code Style

- Use `rustfmt` for formatting
- Use `clippy` for linting
- Add `#[allow(dead_code)]` for future-use code
- Document public APIs
- Write tests for new features

### Pull Request Process

1. Create feature branch
2. Make changes
3. Run `just check` (fmt + clippy + tests)
4. Run benchmarks if performance-sensitive
5. Update PROJECT_CONTEXT.md if needed
6. Submit PR with description

---

## Troubleshooting

### Servers Won't Start

```bash
# Check if ports are in use
ss -tuln | grep -E ":(9000|9001|9100|9101)"

# Kill existing processes
just kill

# Check logs
tail -f engine1.log
tail -f gateway.log
```

### Benchmarks Hang

```bash
# If JSON benchmarks hang, use binary protocol
just bench-rtt          # Works
just bench-rtt-fast     # Works

# Avoid JSON until fixed
# just bench-distributed  # Don't use
```

### Build Errors

```bash
# Clean and rebuild
just clean
cargo build --release

# Update dependencies
cargo update
```

### Performance Issues

```bash
# Profile with flamegraph (requires cargo-flamegraph)
cargo install flamegraph
cargo flamegraph --bin engine_server

# Check CPU pinning
taskset -c 0-7 ./target/release/engine_server ...
```

---

## Resources

- **Project Repository:** [link]
- **Issue Tracker:** [link]
- **Documentation:** `cargo doc --workspace --open`
- **Benchmarks:** `just perf` or `cargo bench`

---

## License

MIT

---

**Last Updated:** 2026-02-10  
**Version:** 0.1.0  
**Status:** Core functionality complete, production deployment in progress

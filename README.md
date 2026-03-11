# ⚡ High-Performance Trading Engine

> **A production-grade, ultra-low-latency distributed matching engine built in Rust**

[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Performance](https://img.shields.io/badge/latency-10μs%20p50-brightgreen.svg)](#performance)

**Built for speed.** Tested at **10-13μs p50 latency** (localhost), this is a high-frequency trading engine capable of matching **20M+ orders/second** with deterministic persistence and sub-microsecond matching.

---

## 🎯 Overview

This is a **distributed order matching system** designed with the same architecture used by major cryptocurrency exchanges (Coinbase, Binance, Kraken). It separates concerns into specialized components:

- **Gateway Server**: Client connections, authentication, risk management, order routing
- **Engine Servers**: Per-symbol matching engines (one process per trading pair)
- **Binary Protocol**: Zero-copy, length-delimited for minimal serialization overhead
- **Persistence**: Journaling + snapshots for crash recovery and deterministic replay

**Use Cases:**
- Cryptocurrency exchange matching engine
- High-frequency trading infrastructure
- Order book simulation and backtesting
- Performance benchmarking and research

---

## 📊 Performance

### Latency (End-to-End RTT)

```
Benchmark: 500,000 iterations (localhost)

p50  = 10 μs     ← Median round-trip time
p99  = 40 μs     ← 99th percentile
p999 = 215 μs    ← Tail latency
```

**What this measures:** Complete cycle from client sending order → gateway risk check → engine matching → fill response back to client.

### Throughput

| Component | Metric | Performance |
|-----------|--------|-------------|
| **Engine Matching** | Single-threaded | **20.6M orders/sec** (48ns/order) |
| **Gateway Routing** | Request handling | **2.7M orders/sec** (360ns/order) |
| **Binary Codec** | Encode/Decode | **~100ns per message** |

> **Benchmark Commands:**
> ```bash
> cargo bench              # Criterion microbenchmarks
> just bench-rtt           # End-to-end latency
> just bench-throughput    # Gateway throughput
> ```

---

## 🏗️ Architecture

### System Design

```
┌─────────────────────────────────────────────────────────────┐
│                       Clients (Traders)                      │
└──────────────────────────┬──────────────────────────────────┘
                           │
                           ↓
              ┌────────────────────────┐
              │   Gateway Server       │
              │   :9000 (binary)       │   ← Single source of truth
              │   :9001 (JSON)         │      for accounts & risk
              │   :8080 (metrics)      │
              │                        │
              │  • Authentication      │
              │  • Account Management  │
              │  • Risk Checks         │
              │  • Order Routing       │
              │  • Persistence         │
              └────────┬───────────────┘
                       │
        ┌──────────────┼──────────────┐
        │              │              │
        ↓              ↓              ↓
  ┌──────────┐  ┌──────────┐  ┌──────────┐
  │ BTC/USD  │  │ ETH/USD  │  │ SOL/USD  │
  │ Engine   │  │ Engine   │  │ Engine   │
  │ :9100    │  │ :9101    │  │ :9102    │
  │          │  │          │  │          │
  │ Order    │  │ Order    │  │ Order    │
  │ Matching │  │ Matching │  │ Matching │
  └──────────┘  └──────────┘  └──────────┘
```

### Key Design Decisions

1. **Per-Symbol Engines**: Each trading pair runs in its own process
   - Zero lock contention (single-threaded matching)
   - Isolated failures (one pair crashes ≠ full exchange down)
   - Independent scaling (allocate resources per pair popularity)

2. **Gateway as Risk Oracle**: Centralized account state prevents race conditions
   - Pre-flight checks with tentative reservations
   - No double-spend or overselling
   - Simplified rollback on failures

3. **Binary Protocol**: Custom length-delimited format
   - Zero-copy deserialization where possible
   - ~100ns encode/decode latency
   - Type-safe with `postcard` serialization

4. **Crash Recovery**: Persistent state with snapshots
   - Journal: All commands logged before execution (CRC32 checksums)
   - Snapshots: Periodic order book state dumps
   - Replay: Restore snapshot + replay delta commands

---

## ✨ Features

### Core Functionality
- ✅ **Price-Time Priority Matching** - FIFO at each price level
- ✅ **Limit Orders** - Maker orders (Post-Only optional)
- ✅ **Market Orders** - Taker orders (immediate execution)
- ✅ **Order Cancellation** - Cancel by order ID
- ✅ **Order Replacement** - Cancel+Replace atomic operation
- ✅ **Partial Fills** - Large orders match incrementally

### Risk Management
- ✅ **Buying Power Checks** - Pre-flight validation
- ✅ **Tentative Reservations** - Prevent race conditions
- ✅ **Position Tracking** - Real-time account state
- ✅ **Session Management** - Secure authentication

### Persistence & Recovery
- ✅ **Write-Ahead Journaling** - All commands logged with CRC32
- ✅ **Periodic Snapshots** - Fast recovery (restore + replay delta)
- ✅ **Configurable Durability** - Batch fsync for latency/durability tradeoff
- ✅ **Deterministic Replay** - Rebuild state from journals

### Observability
- ✅ **Prometheus Metrics** - Latency histograms, order counts, fills
- ✅ **Health Endpoints** - `/health` and `/metrics` on gateway
- ✅ **Tracing** - Structured logging with `tracing` crate
- ✅ **Profiling Support** - Debug symbols for `perf` analysis

### Protocols
- ✅ **Binary Protocol** - High-performance (default)
- ✅ **JSON Protocol** - Human-readable for testing
- ✅ **Length-Delimited Framing** - Clean message boundaries

---

## 🚀 Quick Start

### Prerequisites

- **Rust 1.70+** - Install via [rustup](https://rustup.rs/)
- **Just** (optional) - Command runner: `cargo install just`
- **Linux/macOS** - Primary development platforms

### Installation

```bash
# Clone the repository
git clone https://github.com/yourusername/high-perf-trading
cd high-perf-trading

# Build release binaries (with debug symbols for profiling)
cargo build --release

# Install just (if not already installed)
cargo install just
```

### Running the System

**Option 1: Using `just` (Recommended)**

```bash
# Start everything (engines + gateway)
just dev

# Or start components separately:
just dev-engines    # Start only engine servers
just dev-gateway    # Start only gateway (requires engines running)
```

**Option 2: Manual**

```bash
# Terminal 1: Start engines
./target/release/engine_server --symbol-id 1 --symbol-name "BTC/USD" \
  --listen-addr 127.0.0.1:9100 --admin-addr 127.0.0.1:9200 \
  --journal-path journals/engine1.bin --snapshot-dir snapshots

./target/release/engine_server --symbol-id 2 --symbol-name "ETH/USD" \
  --listen-addr 127.0.0.1:9101 --admin-addr 127.0.0.1:9201 \
  --journal-path journals/engine2.bin --snapshot-dir snapshots

# Terminal 2: Start gateway
./target/release/gateway_server \
  --client-binary-addr 0.0.0.0:9000 \
  --client-json-addr 0.0.0.0:9001 \
  --admin-addr 0.0.0.0:8080 \
  --journal-path journals/gateway.bin \
  --snapshot-dir snapshots \
  --engines-config engines.toml
```

### First Test

```bash
# Run smoke test (basic functionality)
just smoke-bin

# Expected output:
# bin resp len=...
# smoke ok
```

---

## 🧪 Testing & Benchmarking

### Smoke Tests

```bash
just smoke-bin       # Binary protocol basic test
just smoke-json      # JSON protocol basic test
just smoke-match     # Order matching test
just smoke-cancel    # Cancel reservation test
just smoke-replace   # Replace reservation test
just smoke-risk      # Risk rejection test
just smoke           # Run all smoke tests
```

### Benchmarks

```bash
# Latency benchmarks
just bench-rtt              # End-to-end RTT (100k iterations)
just bench-rtt-fast         # Quick test (1k iterations)
just bench-distributed      # Realistic order flow

# Throughput benchmarks
just bench-throughput       # Gateway throughput test

# Microbenchmarks (Criterion)
cargo bench                 # Run all Criterion benchmarks
just perf                   # Alias for cargo bench
```

### Profiling

```bash
# Profile with Firefox Profiler (visual timeline + flamegraph)
just profile           # Default 100k iterations
just profile 500000    # Custom iteration count

# Generate flamegraph SVG (simpler, no Firefox needed)
just profile-flamegraph

# Quick text-based report
just profile-quick

# Analyze existing perf.data
just profile-analyze
```

See [PROFILING.md](PROFILING.md) for detailed profiling guide.

---

## 📦 Project Structure

```
high-perf-trading/
├── crates/
│   ├── common/              # Shared types (OrderId, Price, Symbol, etc.)
│   ├── codecs/              # Binary & JSON protocol implementations
│   ├── persistence/         # Journaling + snapshot system
│   ├── engine/              # Order book + matching logic (core)
│   ├── engine_server/       # Engine server binary
│   ├── gateway_server/      # Gateway server binary
│   ├── bench/               # Benchmarking tools
│   └── test_client/         # Manual testing client
│
├── scripts/                 # Helper scripts
├── engines.toml             # Engine routing configuration
├── justfile                 # Command recipes (dev, test, bench, profile)
├── Cargo.toml               # Workspace configuration
├── PROJECT_CONTEXT.md       # Detailed design documentation
├── PROFILING.md             # Profiling guide
└── README.md                # This file
```

---

## 🛠️ Development Commands

### Build & Run

```bash
just build              # Build release binaries
just build-servers      # Build only server binaries
just dev                # Start full system (engines + gateway)
just restart            # Kill all + clean + restart
```

### Testing

```bash
cargo test              # Run unit tests
just test               # Workspace tests
just smoke              # All smoke tests
just bench-all          # All benchmarks
```

### Code Quality

```bash
just fmt                # Format code
just fmt-check          # Check formatting
just clippy             # Run linter
just check              # fmt-check + clippy + test
just pre-commit         # Quick check before committing
```

### Monitoring

```bash
just health             # Check gateway health
just metrics            # Show Prometheus metrics
just status             # Show running processes
just logs               # Tail server logs
```

### Cleanup

```bash
just kill               # Kill all server processes
just clean              # Clean build artifacts
just clean-persistence  # Clean journals/snapshots
just clean-logs         # Clean log files
just clean-all          # Clean everything
```

### Documentation

```bash
just docs               # Generate and open Rust docs
just tree               # Show project structure
just help               # List all commands
```

---

## ⚙️ Configuration

### Gateway Configuration

**CLI Arguments:**
- `--client-binary-addr 0.0.0.0:9000` - Binary protocol port
- `--client-json-addr 0.0.0.0:9001` - JSON protocol port
- `--admin-addr 0.0.0.0:8080` - Metrics/health endpoint
- `--engines-config engines.toml` - Engine routing config
- `--journal-path journals/gateway.bin` - Journal file path
- `--snapshot-dir snapshots` - Snapshot directory
- `--journal-batch-size 100` - Commands per fsync
- `--snapshot-interval 100000` - Commands between snapshots

### Engine Configuration

**CLI Arguments:**
- `--symbol-id 1` - Unique symbol ID
- `--symbol-name "BTC/USD"` - Symbol name
- `--listen-addr 127.0.0.1:9100` - Engine listen address
- `--admin-addr 127.0.0.1:9200` - Admin endpoint
- `--journal-path journals/engine1.bin` - Journal path
- `--snapshot-dir snapshots` - Snapshot directory

**engines.toml:**
```toml
[[engine]]
symbol_id = 1
symbol_name = "BTC/USD"
addr = "127.0.0.1:9100"

[[engine]]
symbol_id = 2
symbol_name = "ETH/USD"
addr = "127.0.0.1:9101"
```

---

## 📈 Metrics

Access Prometheus metrics at `http://localhost:8080/metrics`

**Key Metrics:**
- `gateway_orders_total{status}` - Order submission counts
- `gateway_order_latency_us` - Histogram of gateway processing time
- `gateway_fills_total` - Fill event counts
- `gateway_connections_active` - Active client connections
- `journal_appends_total` - Journal write counts
- `journal_errors_total` - Journal error counts
- `snapshot_created_total` - Snapshot counts

---

## 🎓 Learn More

### Documentation

- **[PROJECT_CONTEXT.md](PROJECT_CONTEXT.md)** - Detailed design, architecture, and implementation notes
- **[PROFILING.md](PROFILING.md)** - Performance profiling guide
- **Rust Docs** - Run `just docs` to generate and browse API documentation

### Key Concepts

1. **Price-Time Priority**: Orders match in price-time order (best price first, earliest timestamp at same price)
2. **Tentative Reservations**: Gateway reserves buying power tentatively before sending to engine
3. **Deterministic Replay**: Journal all commands → can rebuild exact state
4. **Zero-Lock Matching**: Single-threaded engine = no mutex contention

### Performance Tuning

- See **[PROFILING.md](PROFILING.md)** for profiling guide
- Disable iptables on loopback for ~1-2μs improvement
- Use CPU pinning / `isolcpus` for lower tail latency
- Adjust `journal-batch-size` for latency/durability tradeoff

---

## 🐛 Known Issues

See [PROJECT_CONTEXT.md#known-issues](PROJECT_CONTEXT.md#known-issues) for current limitations and workarounds.

---

## 🗺️ Roadmap

**Completed:**
- ✅ Core matching engine
- ✅ Gateway with risk management
- ✅ Persistence (journaling + snapshots)
- ✅ Binary & JSON protocols
- ✅ Metrics & observability

**Future Enhancements:**
- [ ] WebSocket streaming (real-time market data)
- [ ] Order book depth snapshots
- [ ] Market data API (top-of-book, trades)
- [ ] Admin API (cancel all, shutdown, etc.)
- [ ] Docker deployment
- [ ] Kubernetes manifests
- [ ] Enhanced monitoring dashboards

---

## 📄 License

This project is licensed under the MIT License - see [LICENSE](LICENSE) file for details.

---

## 🙏 Acknowledgments

Inspired by production exchange architectures from:
- Coinbase (per-symbol matching engines)
- Binance (high-throughput design)
- Kraken (risk management patterns)

Built with Rust and amazing open-source libraries:
- [Tokio](https://tokio.rs/) - Async runtime
- [Serde](https://serde.rs/) - Serialization
- [Prometheus](https://prometheus.io/) - Metrics
- [Criterion](https://github.com/bheisler/criterion.rs) - Benchmarking

---

<div align="center">

**⚡ Built with Rust for maximum performance ⚡**

[Report Bug](https://github.com/yourusername/high-perf-trading/issues) · [Request Feature](https://github.com/yourusername/high-perf-trading/issues)

</div>

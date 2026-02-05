# High-Perf Trading Engine (Rust)

Low-latency distributed trading system with gateway + per-symbol engine servers.
Goal: Production-ready matching engine similar to real exchanges (Coinbase, Binance architecture).

---

## Architecture Overview

**Distributed Design**: Gateway server handles global account state and risk, routes to dedicated per-symbol engine servers.

```
                     Clients (traders)
                            ↓
                ┌─────────────────────┐
                │  Gateway Server     │
                │  - Account state    │
                │  - Risk checks      │
                │  - Auth             │
                │  - Order routing    │
                │  - Fill aggregation │
                └──────────┬──────────┘
                           │ (Low-latency network: AWS Enhanced Networking)
                           │ (Same AZ, Cluster Placement Group)
               ┌───────────┼────────────┬─────────────┐
               ↓           ↓            ↓             ↓
         ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────┐
         │ BTC/USD │  │ ETH/USD │  │ SOL/USD │  │ DOGE/   │
         │ Engine  │  │ Engine  │  │ Engine  │  │ Engine  │
         │         │  │         │  │         │  │         │
         │ 8 cores │  │ 8 cores │  │ 4 cores │  │ 1 core  │
         └─────────┘  └─────────┘  └─────────┘  └─────────┘
         Hot Pair      Hot Pair     Warm Pair    Cold Pair
```

### Key Principles

- **1 Order Book = 1 Engine Server** - Simple, isolated, cache-optimized
- **Gateway = Risk Oracle** - Single source of truth for account state
- **Cost-Efficient Scaling** - Hot pairs get beefy servers, cold pairs get cheap ones
- **Horizontal Scaling** - Add engine servers as you list new trading pairs
- **Fault Isolation** - One engine crash doesn't affect other symbols

---

## Workspace Crates

### `gateway_server` (NEW - TO BE IMPLEMENTED)
**Global Account & Risk Management**

Responsibilities:
- Maintain all account state in-memory (positions, buying power, risk limits)
- Handle client connections (binary + JSON protocols)
- Pre-flight risk checks with tentative reservations
- Route approved orders to correct engine server by symbol_id
- Receive fills from engines and update account state
- Persistence: journaling + snapshots (same strategy as engine)
- Admin HTTP API for monitoring, metrics, account queries

Key Components:
- `AccountManager` - In-memory account state with per-account locks
- `RiskChecker` - Pre-flight risk with tentative reservations
- `EngineRouter` - Maintains persistent TCP connections to all engines
- `ClientHandler` - Handles client connections (reuse current server code)
- `Journal` - Account update persistence (deposits, fills, limits)

### `engine_server` (RENAMED FROM `server`)
**Pure Order Book Matching**

Responsibilities:
- Single order book for one symbol
- Accept orders from gateway (already risk-approved)
- Pure price-time FIFO matching (no risk checks)
- Emit fills, BookTop, Trade events back to gateway
- Persistence: order book journaling + snapshots
- Single-threaded for zero-lock latency

Simplified from current design:
- Remove account management (moves to gateway)
- Remove risk checks (gateway does this)
- Remove client connection handling (gateway does this)
- Keep: matching engine, order book, persistence

### `common`
Protocol types (Command/Event), serde models, shared structs (unchanged)

### `codecs`
Binary/JSON serialization (unchanged)

### `persistence`
Append-only journal with CRC32, snapshots (unchanged)

### `market_data` (NEW - FUTURE)
**Time-Series Market Data Storage**

Responsibilities:
- Consume BookTop and Trade events from gateway
- Buffer and batch write to ClickHouse
- Provide query API for historical data
- Non-critical path (async, eventual consistency)

---

## Gateway Architecture (Detailed)

### Account State Management

```rust
struct Gateway {
    // Account state (in-memory, locked per account)
    accounts: DashMap<AccountId, Arc<Mutex<AccountState>>>,
    
    // Routing table: symbol_id -> engine address
    engine_routes: HashMap<SymbolId, SocketAddr>,
    
    // Persistent TCP connections to engines
    engine_conns: HashMap<SymbolId, TcpStream>,
    
    // Persistence
    journal: Journal<AccountUpdate>,
    snapshot_manager: SnapshotManager,
}

struct AccountState {
    account_id: AccountId,
    buying_power: i64,
    tentative_reserved: i64,  // Locked for pending orders
    positions: HashMap<SymbolId, Position>,
    risk_limits: HashMap<SymbolId, RiskLimits>,
}
```

### Tentative Reservations (Race Condition Prevention)

**Problem**: User with $100k tries to place two $100k orders simultaneously.

**Solution**: Lock account and reserve buying power tentatively:

```rust
// When order arrives
fn process_order(&self, order: NewOrder) -> Result<()> {
    let account = self.accounts.get(&order.account_id)?;
    let mut state = account.lock();  // 🔒 Serialize orders per account
    
    // Check available funds
    let required = order.price * order.qty;
    let available = state.buying_power - state.tentative_reserved;
    if required > available {
        return Err(RiskViolation::InsufficientFunds);
    }
    
    // Reserve tentatively (money locked while order pending)
    state.tentative_reserved += required;
    
    // Route to engine
    self.route_to_engine(order, ReservationToken { amount: required })?;
    Ok(())
}

// When fill arrives from engine
fn handle_fill(&self, fill: Fill, token: ReservationToken) {
    let mut state = self.accounts.get(&fill.account_id).lock();
    
    // Release tentative, apply actual
    state.tentative_reserved -= token.amount;
    state.buying_power -= fill.price * fill.qty;
    state.positions[fill.symbol_id] += fill.qty;
    
    // Journal for persistence
    self.journal.append(AccountUpdate::Fill(fill));
}
```

**Result**: Orders for same account are serialized (one at a time), preventing double-spend.

### Account State Updates (Complete Lifecycle)

The gateway maintains account state through the entire order lifecycle. Here's how different operations affect account state:

#### 1. New Order (Buy Side)
```rust
// Initial state
buying_power: $100,000
tentative_reserved: $0
position: 0 BTC

// Order: Buy 1 BTC @ $50,000
1. Check: available = $100,000 - $0 = $100,000 >= $50,000 ✓
2. Reserve: tentative_reserved += $50,000 → $50,000
3. Route to engine

// State after reservation
buying_power: $100,000
tentative_reserved: $50,000  // Locked for pending order
available: $50,000            // Can still place orders with remaining
```

#### 2. Fill (Order Executed)
```rust
// Fill arrives: Bought 1 BTC @ $50,000
1. Release tentative: tentative_reserved -= $50,000 → $0
2. Apply actual cost: buying_power -= $50,000 → $50,000
3. Update position: position += 1 BTC → 1 BTC
4. Journal for persistence

// Final state after fill
buying_power: $50,000
tentative_reserved: $0
position: 1 BTC
```

#### 3. Sell Order (Reduces Position)
```rust
// Current state: 1 BTC, $50,000 cash
// Order: Sell 1 BTC @ $51,000

1. No buying power check needed (selling increases buying power)
2. Check position: 1 BTC >= 1 BTC ✓
3. No tentative reservation needed for sells
4. Route to engine

// Fill arrives: Sold 1 BTC @ $51,000
1. Update position: position -= 1 BTC → 0 BTC
2. Credit proceeds: buying_power += $51,000 → $101,000
3. Realized P&L: $1,000 profit
```

#### 4. Partial Fill
```rust
// Order: Buy 10 BTC @ $50,000 (total $500,000)
1. Reserve: tentative_reserved += $500,000

// Partial fill: 3 BTC @ $50,000
1. Release partial: tentative_reserved -= $150,000 → $350,000
2. Apply cost: buying_power -= $150,000
3. Update position: position += 3 BTC

// Remaining 7 BTC still reserved ($350,000 locked)
```

#### 5. Cancel Order (TODO - NOT YET IMPLEMENTED) 🚧
```rust
// Order: Buy 1 BTC @ $50,000 (reserved $50,000)
// User cancels order

1. Release reservation: tentative_reserved -= $50,000 → $0
2. No buying_power change (nothing executed)
3. No position change
4. Journal cancellation event

// Implementation needed in AccountManager:
fn release_reservation(&self, token: ReservationToken) -> Result<()> {
    let mut state = self.accounts.get(&token.account_id)?.lock();
    state.tentative_reserved -= token.amount;
    Ok(())
}
```

**Status**: Cancel command needs account state integration:
- Gateway receives Cancel command from client
- Gateway must track reservation tokens by order_id
- When cancel confirmed, release reservation
- Handle race conditions (cancel vs fill)

#### 6. Replace Order (TODO - NOT YET IMPLEMENTED) 🚧
```rust
// Original: Buy 1 BTC @ $50,000 (reserved $50,000)
// Replace with: Buy 1 BTC @ $51,000 (needs $51,000)

1. Check if new amount affordable: 
   available = buying_power - (tentative_reserved - old_reservation)
2. If affordable:
   - Adjust reservation: tentative_reserved += ($51,000 - $50,000) → +$1,000
   - Route replace to engine
3. If not affordable:
   - Reject replace
   - Keep original order

// Implementation needed:
fn adjust_reservation(&self, 
    account_id: AccountId,
    old_amount: i64,
    new_amount: i64
) -> Result<ReservationToken> {
    let mut state = self.accounts.get(&account_id)?.lock();
    
    let adjustment = new_amount - old_amount;
    let available = state.buying_power - state.tentative_reserved + old_amount;
    
    if new_amount > available {
        return Err(RiskViolation::InsufficientFunds);
    }
    
    state.tentative_reserved += adjustment;
    Ok(ReservationToken { amount: new_amount, ... })
}
```

**Status**: Replace command needs account state integration:
- Gateway receives Replace command from client  
- Calculate new reservation amount
- Atomic adjustment (release old, reserve new)
- Handle partial fills before replace

#### 7. Rejected Orders
```rust
// Order: Buy 100 BTC @ $50,000 (needs $5M)
// Account only has $100k

1. Risk check fails: available < required
2. No reservation made
3. Reject sent to client immediately
4. No engine routing
```

#### Key Design Principles
- **Atomic operations**: Account lock ensures no race conditions
- **Pessimistic locking**: Reserve before routing (never overspend)
- **Idempotency**: Gateway sequence numbers prevent duplicate processing
- **Deterministic**: Journal replay produces same state
- **Fast path for sells**: No reservation needed (increases buying power)

#### Race Condition Handling
```rust
// Scenario: User sends Cancel while Fill is in flight

Thread 1 (Cancel):          Thread 2 (Fill from engine):
1. Lock account             1. Wait for lock...
2. Check order exists       
3. Release reservation      2. Acquire lock
4. Send cancel to engine    3. Process fill
5. Unlock                   4. Release reservation
                            5. Unlock

Result: Fill processed first, cancel becomes no-op
```

**Implementation note**: Need order_id → reservation mapping in gateway.

### Gateway ↔ Engine Protocol

**Protocol Types** (defined in `common/src/gateway_protocol.rs`):

**Gateway → Engine** (order submission):
```rust
enum GatewayToEngine {
    /// Execute a command (already risk-approved by gateway)
    Execute(ExecuteCommand),
    /// Health check / ping
    Ping,
}

struct ExecuteCommand {
    /// The original command from the client
    command: Command,
    /// Connection ID for routing responses back to client
    conn_id: u64,
    /// Risk approval token (proves gateway checked risk)
    risk_token: RiskToken,
}

struct RiskToken {
    /// Account ID (for verification)
    account_id: AccountId,
    /// Amount of buying power reserved for this order
    reserved_amount: i64,
    /// Sequence number from gateway (for idempotency)
    gateway_seq: u64,
}
```

**Engine → Gateway** (fill events and market data):
```rust
enum EngineToGateway {
    /// An event to be forwarded to a client
    ClientEvent {
        conn_id: u64,
        event: Event,  // Fill, Ack, Reject, etc.
        risk_token: Option<RiskToken>,
    },
    /// Engine health status
    Pong {
        symbol_id: SymbolId,
        orders_in_book: usize,
    },
    /// Market data broadcast (BookTop, Trade)
    MarketData {
        symbol_id: SymbolId,
        event: Event,
    },
}
```

**Key Design Decisions:**
- Gateway wraps client Commands with risk metadata (RiskToken)
- Engine trusts gateway's risk approval (no re-checking)
- conn_id allows engine to route responses back through gateway
- RiskToken returned with events so gateway can release reservations
- Ping/Pong for health monitoring
- Market data events separate from client events

---

## Engine Server Architecture (Simplified)

### Changes from Current Implementation

**Removed**:
- ❌ Client connection handling (moves to gateway)
- ❌ Router (gateway handles routing to clients)
- ❌ Account state management (gateway owns this)
- ❌ Risk checks (gateway pre-approves)

**Kept**:
- ✅ Order book matching (core logic unchanged)
- ✅ Persistence (journaling + snapshots)
- ✅ Single-threaded engine on dedicated thread
- ✅ Metrics

**New**:
- ✅ Listen for orders from gateway (TCP)
- ✅ Send fills back to gateway (TCP)
- ✅ Simpler: just a matching engine service

### Engine Threading Model

```
Gateway Connection (Tokio async)
         ↓
    [Command Queue]
         ↓
  Engine Thread (single-threaded, dedicated OS thread)
    • Match orders
    • Journal to disk
    • Emit fills
         ↓
    [Event Queue]
         ↓
Gateway Connection (Tokio async)
```

---

## Deployment Architecture

### AWS Infrastructure

**Cluster Placement Group** in same Availability Zone:
- Low-latency networking (~50-200μs between servers)
- Enhanced Networking enabled (SR-IOV, lower jitter)
- 10Gbe or 25Gbe network

### Server Sizing

#### Gateway Server
- **Instance**: c7i.4xlarge (16 cores, 32GB RAM)
- **Cost**: ~$500/month
- **Workload**: Handles 1000s of client connections, risk checks, routing
- **NVMe SSD**: For account journal

#### Hot Pair Engines (Top 5-10 pairs)
- **Instance**: c7i.2xlarge (8 cores, 16GB RAM)
- **Cost**: ~$300/month each
- **Pairs**: BTC/USD, ETH/USD, BTC/ETH, SOL/USD, etc.
- **High order flow**: Needs CPU power

#### Warm Pair Engines (Next 20-40 pairs)
- **Instance**: c7i.xlarge (4 cores, 8GB RAM)
- **Cost**: ~$150/month each
- **Pairs**: MATIC/USD, LINK/USD, AVAX/USD, etc.

#### Cold Pair Engines (Long tail, 100+ pairs)
- **Instance**: t3.small or t3.micro (1-2 cores, 2-4GB RAM)
- **Cost**: ~$15-50/month each
- **Pairs**: Meme coins, low-cap tokens
- **Low volume**: 10 orders per day

### Example Cost (50 Trading Pairs)
- Gateway: 1 × $500 = $500
- Hot engines: 5 × $300 = $1,500
- Warm engines: 15 × $150 = $2,250
- Cold engines: 30 × $30 = $900
- ClickHouse (market data): 1 × $200 = $200
- **Total: ~$5,350/month**

---

## Communication & Networking

### Gateway ↔ Engines
- **Protocol**: TCP with length-delimited framing (existing implementation)
- **Connection Model**: Gateway maintains persistent connections to each engine
- **Latency**: 50-200μs in same AZ with Enhanced Networking
- **Codec**: Binary (compact, fast) or JSON (debugging)

### Client ↔ Gateway
- **Protocol**: Same as current (binary on :9000, JSON on :9001)
- **Connection Model**: Long-lived TCP connections
- **Gateway acts as proxy**: Routes orders to engines, aggregates fills

### Market Data → ClickHouse
- **Protocol**: ClickHouse native protocol or HTTP
- **Mode**: Async batched writes (buffer 1000 events, flush every 100ms)
- **Non-critical path**: No impact on order latency

---

## Persistence Strategy

### Gateway Persistence (Account State)
- **Journal**: Account updates (deposits, withdrawals, fills, risk limits)
- **Format**: [u32 len][postcard(AccountUpdate)][u32 crc32]
- **Snapshots**: Periodic dumps of all account state
- **Recovery**: Load snapshot + replay journal

### Engine Persistence (Order Book)
- **Journal**: Commands (NewOrder, Cancel, Replace)
- **Format**: [u32 len][postcard(Command)][u32 crc32]
- **Snapshots**: Periodic order book snapshots
- **Recovery**: Load snapshot + replay journal

Both use **same persistence crate** (already implemented).

---

## Market Data Pipeline

```
Engine Servers
  ↓ (emit BookTop, Trade events)
Gateway
  ↓ (forward to clients + buffer for storage)
Market Data Service
  ↓ (batch write every 100ms)
ClickHouse Database
  ↓ (query API for historical data)
Analytics / Dashboards
```

### ClickHouse Schema

```sql
CREATE TABLE trades (
    timestamp DateTime64(6),
    symbol_id UInt32,
    price Int64,
    qty Int64,
    taker_order_id UInt64,
    maker_order_id UInt64,
    taker_side Enum('Buy', 'Sell')
) ENGINE = MergeTree()
ORDER BY (symbol_id, timestamp);

CREATE TABLE book_snapshots (
    timestamp DateTime64(6),
    symbol_id UInt32,
    best_bid_px Nullable(Int64),
    best_bid_qty Nullable(Int64),
    best_ask_px Nullable(Int64),
    best_ask_qty Nullable(Int64)
) ENGINE = MergeTree()
ORDER BY (symbol_id, timestamp);
```

---

## Configuration Files

### Gateway Config (`gateway.toml`)
```toml
[server]
listen_binary = "0.0.0.0:9000"
listen_json = "0.0.0.0:9001"
admin_http = "0.0.0.0:8080"

[persistence]
journal_path = "/data/accounts.journal"
snapshot_dir = "/data/snapshots"
journal_batch_size = 100
snapshot_interval = 100000

[[engines]]
symbol_id = 1
name = "BTC/USD"
address = "10.0.1.10:9000"

[[engines]]
symbol_id = 2
name = "ETH/USD"
address = "10.0.1.11:9000"

[market_data]
clickhouse_url = "http://10.0.1.20:8123"
buffer_size = 1000
flush_interval_ms = 100
```

### Engine Config (`engine.toml`)
```toml
[engine]
symbol_id = 1
symbol_name = "BTC/USD"
listen_addr = "0.0.0.0:9000"
gateway_addr = "10.0.1.5:8000"

[persistence]
journal_path = "/data/btc_usd.journal"
snapshot_dir = "/data/snapshots"
journal_batch_size = 100
snapshot_interval = 100000
```

---

## Implementation Roadmap

### Phase 1: Refactor Current Code ✅ COMPLETE
- [x] Move to `tokio_util::LengthDelimitedCodec` (battle-tested framing)
- [x] Risk management foundation (positions, limits)
- [x] Persistence with journaling + snapshots

### Phase 2: Split Gateway & Engine (NEXT - 2-3 weeks)
- [ ] Rename `server` crate → `engine_server`
- [ ] Create `gateway_server` crate
- [ ] Move account management to gateway
- [ ] Implement tentative reservations in gateway
- [ ] Define gateway ↔ engine protocol
- [ ] Simplify engine (remove client handling, routing)
- [ ] Test with 2 engines (BTC/USD, ETH/USD)

### Phase 3: Multi-Engine Deployment (2 weeks)
- [ ] Gateway routing table configuration
- [ ] Persistent TCP connection pool in gateway
- [ ] Engine discovery and health checks
- [ ] Test with 10+ engines
- [ ] Deployment scripts for AWS

### Phase 4: Market Data Pipeline (1 week)
- [ ] ClickHouse setup and schema
- [ ] Market data service (buffers events)
- [ ] Batch writer to ClickHouse
- [ ] Query API for historical data

### Phase 5: Production Readiness (2-3 weeks)
- [ ] Monitoring and alerting (Prometheus + Grafana)
- [ ] Admin API for operations (risk limit changes, etc.)
- [ ] Load testing and benchmarking
- [ ] Failover and recovery testing
- [ ] Documentation for deployment

### Phase 6: Advanced Features (Future)
- [ ] Dynamic engine allocation (promote/demote pairs)
- [ ] Multi-region deployment
- [ ] Advanced risk (portfolio margin, dynamic limits)
- [ ] WebSocket API for real-time market data

---

## Testing Strategy

### Unit Tests
- Account state management (reservations, fills)
- Risk checks (position limits, buying power)
- Order book matching (existing tests)

### Integration Tests
- Gateway → Engine flow (full order lifecycle)
- Multiple concurrent orders (race condition tests)
- Persistence and recovery (gateway + engine)

### Performance Tests
- Latency benchmarks (p50/p99/p999)
- Throughput tests (orders per second)
- Multi-engine stress tests

### Smoke Tests (justfile recipes)
- `just smoke-gateway` - Test gateway account management
- `just smoke-distributed` - Gateway + 2 engines
- `just smoke-persistence` - Full persistence cycle
- `just bench-distributed` - Multi-engine RTT benchmark

---

## Distributed System Testing Guide

### Testing Architecture

```
Client (test_client or bench tool)
    ↓
Gateway Server (:9000 binary, :9001 JSON, :8080 admin)
    ├─ Account state & risk checks
    ├─ Routes to engines by symbol_id
    └─ Aggregates responses from engines
    ↓
┌───────┴────────┐
↓                ↓
BTC/USD Engine   ETH/USD Engine
(:9100)          (:9101)
(:8081 admin)    (:8082 admin)
```

### Prerequisites

1. **Build Release Binaries**
   ```bash
   cargo build --release --bin gateway_server
   cargo build --release --bin engine_server
   cargo build --release --bin bench  # optional test client
   ```

2. **Configure Engines** (`engines.toml`)
   ```toml
   [[engines]]
   symbol_id = 1
   symbol_name = "BTC/USD"
   address = "127.0.0.1:9100"

   [[engines]]
   symbol_id = 2
   symbol_name = "ETH/USD"
   address = "127.0.0.1:9101"
   ```

### Manual Testing (Multi-Terminal)

**Terminal 1: Start Engine Servers**
```bash
./start_engines.sh
# Or manually:
# ./target/release/engine_server --symbol-id 1 --symbol-name "BTC/USD" --listen-addr 127.0.0.1:9100
# ./target/release/engine_server --symbol-id 2 --symbol-name "ETH/USD" --listen-addr 127.0.0.1:9101
```

Wait for: "Listening for gateway connection on 127.0.0.1:9100"

**Terminal 2: Start Gateway Server**
```bash
./start_gateway.sh
# Or manually:
# ./target/release/gateway_server --engines-config engines.toml
```

Wait for:
- "Connected to 2 engine(s)"
- "Listening for clients: binary=0.0.0.0:9000, json=0.0.0.0:9001"
- "Created test account: account_id=1, buying_power=$1000000"

**Terminal 3: Send Test Orders**
```bash
# Simple test client
cargo run --bin test_client

# Or benchmark tool
cargo run --release --bin bench -- --mode smoke-match --json-addr 127.0.0.1:9001
```

### Expected System Behavior

#### 1. Engine Startup
- Creates journal file: `engine_1_journal.bin`, `engine_2_journal.bin`
- Creates snapshot directory: `engine_1_snapshots/`, `engine_2_snapshots/`
- Admin HTTP starts on `:8081`, `:8082`
- Waits for gateway connection

#### 2. Gateway Startup
- Loads `engines.toml` configuration
- Connects to all configured engines via persistent TCP
- Creates test account (id=1) with $1M buying power
- Listens for client connections:
  - Binary protocol: `:9000`
  - JSON protocol: `:9001`
  - Admin HTTP: `:8080`

#### 3. Order Flow (End-to-End)
```
Client → Gateway → Risk Check → Route to Engine → Match → Fill → Gateway → Client
```

**Detailed Steps:**
1. Client sends `NewOrder` to gateway
2. Gateway locks account and performs risk check
3. Gateway reserves buying power tentatively
4. Gateway routes order to correct engine (by symbol_id)
5. Engine receives order and performs matching
6. Engine generates Fill/Ack events
7. Engine sends events back to gateway
8. Gateway updates account state (releases reservation, applies fill)
9. Gateway forwards event to client

#### 4. Expected Logs

**Gateway Logs:**
```
Gateway connected to engine: BTC/USD (127.0.0.1:9100)
Gateway connected to engine: ETH/USD (127.0.0.1:9101)
Created test account: account_id=1, buying_power=$1000000
Risk check passed: account_id=1, symbol_id=1, required=$50000
Order routed to engine: symbol_id=1, order_id=12345
Fill received from engine: symbol_id=1, fill_id=67890
```

**Engine Logs:**
```
Listening for gateway connection on 127.0.0.1:9100
Gateway connected
Command received: NewOrder { symbol_id=1, account_id=1, ... }
Fill generated: order_id=12345, price=50000, qty=1
```

**Client Output:**
```
Connected to gateway at 127.0.0.1:9001
Sent: NewOrder { symbol_id=1, side=Buy, price=50000, qty=1 }
Received: Ack { order_id=12345 }
Received: Fill { order_id=12345, price=50000, qty=1 }
```

### Testing Scripts

**`start_engines.sh`** - Launch multiple engine servers
```bash
#!/bin/bash
./target/release/engine_server --symbol-id 1 --symbol-name "BTC/USD" --listen-addr 127.0.0.1:9100 &
./target/release/engine_server --symbol-id 2 --symbol-name "ETH/USD" --listen-addr 127.0.0.1:9101 &
echo "Engines started (PIDs: $!)"
```

**`start_gateway.sh`** - Launch gateway server
```bash
#!/bin/bash
./target/release/gateway_server --engines-config engines.toml
```

**`test_risk.sh`** - Test risk management (reject orders)
```bash
#!/bin/bash
# Send orders that should be rejected due to insufficient funds
cargo run --release --bin test_client -- --test-mode risk-reject
```

**`test_risk_reject.sh`** - Test position limits
```bash
#!/bin/bash
# Send orders that exceed position limits
cargo run --release --bin test_client -- --test-mode position-limit
```

### Known Limitations (Work in Progress)

#### Completed ✅
- ✅ Gateway ↔ Engine persistent TCP connections
- ✅ Risk checks with tentative reservations
- ✅ Order routing by symbol_id
- ✅ Fill updates to account state
- ✅ Client connection handling

#### TODO 🚧
- 🚧 Response routing (order_id → client mapping)
  - Gateway needs to track which client sent which order
  - Currently responses may not reach correct client
  
- 🚧 Risk token lifecycle
  - Need to pass RiskToken through engine
  - Required for releasing reservations on cancels
  
- 🚧 Market data broadcasting
  - BookTop/Trade events not yet broadcasted to clients
  - Need separate pub/sub channel for market data
  
- 🚧 Cancel and Replace commands
  - Account state updates for Cancel/Replace not implemented
  - Need to release reservations on cancel
  - Need to adjust reservations on replace

### Troubleshooting

**Problem: "Failed to connect to engine at 127.0.0.1:9100"**
- Solution: Start engines before gateway
- Check: `lsof -i :9100` to verify port is free

**Problem: "Account not found (id=1)"**
- Solution: Gateway creates test account on startup
- Check gateway logs for "Created test account"

**Problem: "No engine configured for symbol_id=X"**
- Solution: Verify `engines.toml` has correct symbol_ids
- Match symbol_id in orders with configured engines

**Problem: "Risk check failed: InsufficientFunds"**
- Solution: Order requires more buying power than available
- Check: Test account has $1M, verify order size
- Formula: `required = price * qty`

**Problem: Client doesn't receive responses**
- Known issue: Response routing incomplete
- Workaround: Check gateway logs for fill events

### Cleanup

```bash
# Kill all processes
pkill -9 engine_server gateway_server bench test_client

# Clean persistence files
rm -f engine_*_journal.bin gateway_journal.bin
rm -rf engine_*_snapshots gateway_snapshots
```

### Next Steps for Production-Ready Testing

1. **Fix response routing** - Track order_id → conn_id mapping in gateway
2. **Implement cancel/replace** - Account state updates for order modifications
3. **Add integration tests** - Automated test suite for distributed flow
4. **Benchmark latency** - Measure p50/p99/p999 latencies
5. **Multi-client stress test** - 100+ concurrent clients
6. **Failure testing** - Engine crash recovery, network partition handling
7. **AWS deployment** - Test with real network latency in cluster placement group

---

## Metrics (Prometheus)

### Gateway Metrics
- `gateway_connections` - Active client connections
- `gateway_accounts_total` - Total accounts in memory
- `gateway_risk_checks_total{result}` - Risk check results (pass/fail)
- `gateway_orders_routed_total{symbol}` - Orders routed to engines
- `gateway_fills_received_total{symbol}` - Fills from engines
- `gateway_reservation_conflicts_total` - Tentative reservation conflicts
- `gateway_journal_appends_total` - Account journal writes
- `gateway_snapshots_total` - Account snapshots

### Engine Metrics (per engine)
- `engine_orders_received_total` - Orders from gateway
- `engine_fills_total` - Fills generated
- `engine_book_depth` - Order book depth (bids + asks)
- `engine_journal_appends_total` - Journal writes
- `engine_snapshots_total` - Snapshots

---

## Completed Features ✅

### Engine (formerly `server`)
- ✅ Order book matching (FIFO, price-time priority)
- ✅ Persistence (journaling + snapshots)
- ✅ Risk management foundation (positions, limits)
- ✅ Binary and JSON protocols
- ✅ LengthDelimitedCodec for framing
- ✅ Prometheus metrics

### Gateway (TO BE IMPLEMENTED)
- ⏳ Account state management
- ⏳ Tentative reservations
- ⏳ Risk pre-flight checks
- ⏳ Order routing to engines
- ⏳ Fill aggregation
- ⏳ Gateway persistence

---

## Current Task List

### Immediate (This Week)
1. ✅ Rename `server` crate to `engine_server` - COMPLETE
   - Renamed directory: `crates/server` → `crates/engine_server`
   - Updated workspace Cargo.toml
   - Updated package name and binary name
   - Updated all justfile references
   - Verified build and tests pass
2. ✅ Create `gateway_server` crate skeleton - COMPLETE
   - Created directory structure: `crates/gateway_server/src/`
   - Created Cargo.toml with dependencies (dashmap for concurrent hashmap)
   - Added to workspace members
   - Created main.rs with CLI argument parsing (clap)
   - Created module stubs:
     - `account_manager.rs` - Account state + tentative reservations
     - `engine_router.rs` - Routes orders to engine servers
     - `client_handler.rs` - Handles client connections
     - `config.rs` - CLI configuration
   - Verified compilation successful
   - Binary created: `target/debug/gateway_server`
3. ✅ Define gateway ↔ engine protocol types - COMPLETE
   - Created `common/src/gateway_protocol.rs` with protocol types
   - Defined `GatewayToEngine` enum (Execute, Ping)
   - Defined `EngineToGateway` enum (ClientEvent, Pong, MarketData)
   - Defined `ExecuteCommand` struct (wraps Command with risk metadata)
   - Defined `RiskToken` struct (risk approval + reservation tracking)
   - Added helper functions (command_symbol_id, command_account_id, etc.)
   - Exported from common crate
   - Verified workspace builds successfully
   - Updated PROJECT_CONTEXT.md with protocol specification
4. ✅ Implement `AccountManager` in gateway (with tentative reservations) - COMPLETE
   - Implemented full AccountState struct with:
     - buying_power tracking
     - tentative_reserved for pending orders
     - positions HashMap<SymbolId, Position>
     - risk_limits HashMap<SymbolId, RiskLimits>
   - Implemented check_and_reserve with:
     - Account locking via DashMap (per-account concurrent access)
     - Buying power validation
     - Position limit checks
     - Order size validation
     - Tentative reservation to prevent double-spend
   - Implemented apply_fill:
     - Releases tentative reservations
     - Updates actual buying power
     - Updates positions (net_position, avg_price)
     - Handles both buy and sell sides correctly
   - Implemented release_reservation for cancelled orders
   - Added 7 unit tests (all passing):
     - Account creation
     - Sufficient/insufficient funds
     - Double-spend prevention
     - Reservation release
     - Sell orders (no buying power needed)
     - Account not found errors
   - Used atomic counter for gateway sequence numbers (idempotency)
5. ✅ Implement EngineRouter (routing and connection pool) - COMPLETE
   - Implemented full EngineRouter with persistent TCP connections
   - Features:
     - TOML configuration loading (engines.toml)
     - Routing table (symbol_id → engine address)
     - Persistent connection pool using LengthDelimitedCodec
     - Async event receiving from all engines
     - Connection health monitoring
   - Key components:
     - `EngineConnection`: Wraps Framed TCP stream
     - `route_to_engine()`: Sends GatewayToEngine messages
     - `recv_event()`: Receives EngineToGateway events
     - Background tasks per engine for event listening
   - Serialization: postcard (compact binary)
   - Added 2 unit tests (passing):
     - TOML config parsing
     - Routing table construction
   - Created engines.toml.example template
6. ✅ Implement ClientHandler (client connection handling with risk checks) - COMPLETE
   - Implemented full client connection handling (~220 lines)
   - Flow:
     1. Receive command from client
     2. Check risk with AccountManager
     3. Route to appropriate engine via EngineRouter
     4. Wait for response from engine
     5. Update AccountManager with fill
     6. Send response to client
   - Key components:
     - `handle_client_connection()`: Per-client connection handler
     - `client_read_loop()`: Reads commands, does risk checks, routes to engines
     - `handle_engine_responses()`: Background task for routing engine events to clients
     - `GatewayContext`: Shared state (AccountManager, EngineRouter, Metrics)
   - Wired everything together in main.rs:
     - Initialize AccountManager (with test account)
     - Load and connect to engines from engines.toml
     - Start TCP listeners for binary + JSON protocols
     - Spawn background task for engine response handling
     - Accept and handle client connections
   - Created engines.toml configuration file
   - Binary successfully builds and compiles

### Short-term (Next 2 Weeks)
7. ✅ Simplify engine_server: remove client handling, keep only matching - COMPLETE
   - Created new `gateway_connection.rs` (~120 lines)
   - Removed old client connection handling (connection.rs, router.rs, gateway.rs)
   - Engine now ONLY accepts connections from gateway
   - Protocol changes:
     - Receives: `GatewayToEngine` messages (Execute commands, Ping)
     - Sends: `EngineToGateway` events (ClientEvent, Pong, MarketData)
   - Updated CLI args for per-symbol configuration:
     - `--symbol-id`: Required, identifies which symbol this engine handles
     - `--symbol-name`: Required, for logging (e.g., "BTC/USD")
     - `--listen-addr`: Where to listen for gateway (default: 0.0.0.0:9100)
     - Auto-generated paths: `engine_{symbol_id}_journal.bin`, `engine_{symbol_id}_snapshots/`
   - Simplified main.rs:
     - Accepts ONE gateway connection (not multiple clients)
     - Starts engine thread (unchanged)
     - Forwards GatewayToEngine → Engine
     - Forwards Engine events → EngineToGateway
   - Removed 3 files: connection.rs, router.rs, gateway.rs
   - Binary builds successfully
   - Reduced complexity: ~400 lines of code removed
8. ✅ Test gateway + 2 engines locally - COMPLETE
   - Created testing infrastructure:
     - `start_engines.sh` - Launch multiple engine servers
     - `start_gateway.sh` - Launch gateway server
     - `TEST_DISTRIBUTED.md` - Complete testing guide
     - `test_client.rs` - Simple test client for validation
   - Built release binaries successfully
   - Started 2 engine servers (BTC/USD on :9100, ETH/USD on :9101)
   - Started gateway server
   - Verified end-to-end functionality:
     - ✅ Engines start and listen for gateway connection
     - ✅ Gateway loads engines.toml configuration (2 engines)
     - ✅ Gateway connects to both engines successfully
     - ✅ Engine logs confirm gateway connections
     - ✅ Test account created (account_id=1, $1M buying power)
     - ✅ Gateway listens for clients on ports 9000 (binary) and 9001 (JSON)
     - ✅ Test client successfully connects to gateway
     - ✅ Gateway accepts client connections
     - ✅ Client handler receives and processes messages
   - Bugs fixed during testing:
     - 🐛 Fixed client_senders registry not passed to client handlers
     - 🐛 Fixed EngineRouter mutex deadlock (holding lock during blocking recv)
     - 🐛 Refactored EngineRouter to split read/write halves (no mutex contention)
   - Architecture changes:
     - Separated engine connections into read and write halves
     - Read half owned by background task (no lock needed)
     - Write half stored in Arc<Mutex<HashMap>> for sending
     - Each engine has dedicated reader task (no blocking between engines)
   - All core functionality working:
     - ✅ Client → Gateway connection
     - ✅ Gateway → Engine connections (persistent TCP)
     - ✅ Engine → Gateway event channel
     - ✅ Client registration and routing infrastructure
   - Ready for actual order testing with proper Command/Event payloads
9. Add integration tests for distributed flow
10. Benchmark latency (gateway → engine → gateway)

### Medium-term (Next Month)
11. AWS deployment scripts (cluster placement group)
12. ClickHouse setup and market data pipeline
13. Admin API for operations
14. Load testing and optimization
15. Production deployment guide

---

## Design Constraints & Goals

- **Low latency**: Sub-millisecond end-to-end (client → gateway → engine → client)
- **Deterministic**: Replay from journal produces same state
- **Minimal allocations**: Hot path (matching) avoids allocations
- **Fault tolerant**: Engine crashes don't lose data (journaling)
- **Cost-efficient**: Pay for what you need (small servers for cold pairs)
- **Horizontally scalable**: Add engines as you list new pairs
- **Production-ready**: Monitoring, metrics, recovery, durability

---

## Technology Stack

- **Language**: Rust (low-latency, safety, performance)
- **Async Runtime**: Tokio (connection handling, I/O)
- **Serialization**: Postcard (compact binary), serde_json (debug)
- **Persistence**: Custom journal + snapshots (deterministic replay)
- **Networking**: LengthDelimitedCodec (battle-tested framing)
- **Metrics**: Prometheus text format
- **Time-Series DB**: ClickHouse (market data storage)
- **Infrastructure**: AWS EC2 (Enhanced Networking, Cluster Placement Groups)
- **Concurrency**: Crossbeam channels (lock-free), single-threaded engine (zero locks)

---

## References & Inspiration

- **Coinbase Exchange**: Gateway + matching engine architecture
- **Binance**: Distributed order book servers
- **LMAX Disruptor**: Single-threaded sequencer pattern
- **Jane Street**: Tentative reservation pattern for risk
- **Real-world trading systems**: Co-location, AWS placement groups, persistent TCP connections

---

*Last Updated: 2026-02-05*
# Testing Distributed Gateway + Engines

This guide walks through testing the distributed trading system with gateway + multiple engine servers.

## Architecture

```
Client (bench tool)
    ↓
Gateway Server (:9000 binary, :9001 JSON)
    ├─ Account state & risk checks
    ├─ Routes to engines by symbol_id
    └─ Aggregates responses
    ↓
┌───────┴────────┐
↓                ↓
BTC/USD Engine   ETH/USD Engine
(:9100)          (:9101)
```

## Prerequisites

1. **Build binaries:**
```bash
cargo build --release --bin gateway_server --bin engine_server --bin bench
```

2. **Configure engines.toml:**
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

## Manual Testing (3 Terminals)

### Terminal 1: Start Engines

```bash
# Terminal 1
./start_engines.sh
```

This starts:
- BTC/USD engine on port 9100
- ETH/USD engine on port 9101

Wait for logs showing "Listening for gateway connection"

### Terminal 2: Start Gateway

```bash
# Terminal 2
./start_gateway.sh
```

This starts the gateway which will:
- Load engines from engines.toml
- Connect to both engines
- Listen for clients on :9000 (binary) and :9001 (JSON)

Wait for logs showing "Connected to 2 engine(s)"

### Terminal 3: Send Test Orders

```bash
# Terminal 3 - Send test order via JSON protocol
cargo run -p bench -- --mode smoke-match --json-addr 127.0.0.1:9001
```

## What Should Happen

1. **Engine startup:**
   - Each engine creates its journal file
   - Engines wait for gateway connection
   - Admin HTTP starts on :8081, :8082

2. **Gateway startup:**
   - Loads engines.toml
   - Connects to both engines (TCP)
   - Creates test account with $1M buying power
   - Listens for clients

3. **Order flow:**
   ```
   Client → Gateway → Risk Check → Route to Engine → Match → Fill → Gateway → Client
   ```

4. **Expected output:**
   - Gateway logs: "Gateway connected from..." (from engines)
   - Gateway logs: Risk checks passing
   - Gateway logs: Orders routed to symbol_id=X
   - Engine logs: Commands received
   - Client receives: Ack, Fill events

## Known Limitations (TO FIX)

1. **Response routing incomplete:**
   - Gateway needs to track order_id → conn_id mapping
   - Currently responses may not reach correct client

2. **Client sender registry:**
   - Need to register client senders in shared map
   - Background task needs access to send responses

3. **Risk token tracking:**
   - Need to preserve risk tokens through engine
   - Required for releasing reservations on fills

4. **Market data not implemented:**
   - BookTop/Trade events not broadcasted yet

## Troubleshooting

### "Failed to connect to engine"
- Make sure engines are started first
- Check ports are not already in use: `lsof -i :9100`

### "Account not found"
- Gateway creates test account (id=1) on startup
- Check gateway logs for "Created test account"

### "No engine configured for symbol_id"
- Verify engines.toml has correct symbol_ids
- Match symbol_id in test orders with configured engines

## Cleanup

```bash
# Kill all processes
pkill -9 engine_server gateway_server bench

# Clean persistence files
rm -f engine_*_journal.bin gateway_journal.bin
rm -rf engine_*_snapshots gateway_snapshots
```

## Next Steps

Once basic flow works:
1. Fix response routing (order_id → conn_id mapping)
2. Implement proper client sender registry
3. Add integration tests
4. Benchmark latency (p50/p99/p999)
5. Test with multiple concurrent clients

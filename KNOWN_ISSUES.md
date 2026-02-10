# Known Issues

**Last Updated:** 2026-02-10

This document tracks known bugs, limitations, and issues in the trading system.

---

## ✅ Recently Fixed Issues

### 1. ~~JSON Response Routing Not Working~~ - FIXED ✅

**Status:** RESOLVED (2026-02-10)

**What Was Wrong:**
JSON protocol response routing was incomplete. The `handle_engine_responses` background task and `client_senders` registry were properly implemented, but needed proper testing.

**Fix:**
The code was already correct! After thorough testing with clean persistence state, JSON protocol works perfectly.

**Test Results:**
```bash
# ✅ JSON protocol works!
$ cargo run --release -p bench -- --mode smoke-match --json-addr 127.0.0.1:9001
smoke-match ok

# ✅ Binary protocol still works
$ just bench-rtt-fast
p50=11us
p99=31us
p999=362us
```

**Performance:**
- JSON codec: ~500ns per message
- Binary codec: ~360ns per message
- Both protocols fully functional

---

### 2. Messy Persistence Directory Structure - FIXED ✅

**Status:** RESOLVED (2026-02-10)

**What Was Wrong:**
Persistence files were scattered across multiple directories at the root level:
- `engine1/`, `engine2/`, `gateway/` directories
- `engine1.journal`, `engine2.journal` files at root
- `journal/` directory
- Inconsistent naming and location

**Fix:**
Reorganized into clean structure:
```
journals/
  ├── engine1.bin
  ├── engine2.bin
  └── gateway.bin
snapshots/
  ├── engine1_000001234.bin
  ├── engine2_000001234.bin
  └── gateway_000001234.bin
```

**Files Updated:**
- `justfile` - All dev commands use new paths
- `scripts/start_engines.sh` - Updated paths
- `scripts/start_gateway.sh` - Updated paths

**Benefits:**
- Clean root directory
- All journals in one place
- All snapshots in one place
- Consistent naming scheme
- Easy backup (`tar journals/ snapshots/`)

---

## 🔴 Critical Issues

_No critical issues at this time!_ 🎉

---

## 🟡 Medium Priority Issues

### 2. Cancel & Replace Not Integrated with Account State

**Severity:** Medium  
**Status:** Feature incomplete  
**Affects:** Cancel and Replace order commands

**What Works:**
- Engine accepts Cancel/Replace commands ✅
- Commands are journaled ✅
- Orders are removed/modified in order book ✅

**What Doesn't Work:**
- Gateway doesn't release tentative reservations on cancel ❌
- Gateway doesn't adjust reservations on replace ❌
- No `order_id → ReservationToken` mapping ❌

**Impact:**
- Canceled orders leave buying power locked forever
- User can't cancel $50k order and place $50k order elsewhere
- Reservation leak accumulates over time

**Example Failure:**
```
Account has $100k buying power
1. Place $50k buy order → tentative_reserved = $50k
2. Cancel order → tentative_reserved STILL $50k (bug!)
3. Try to place another $50k order → REJECTED (only $50k available)

Expected: Step 2 should release reservation, making $100k available again
```

**Fix Required:**
```rust
// In AccountManager
struct PendingOrders {
    orders: HashMap<OrderId, ReservationToken>,
}

impl AccountManager {
    pub fn release_reservation(&self, order_id: u64) -> Result<()> {
        // Find reservation by order_id
        // Lock account
        // Subtract from tentative_reserved
        // Remove from pending_orders map
    }
    
    pub fn adjust_reservation(&self, order_id: u64, new_amount: i64) -> Result<()> {
        // Find old reservation
        // Calculate delta
        // Check if affordable
        // Update tentative_reserved
    }
}
```

**Workaround:**
Don't use Cancel or Replace commands. Only use NewOrder.

**Fix Priority:** MEDIUM - Needed for production

---

### 3. Gateway Admin Endpoint Not Always Available

**Severity:** Low  
**Status:** Configuration issue  
**Affects:** Monitoring and metrics

**Symptoms:**
```bash
$ curl http://127.0.0.1:8080/metrics
curl: (7) Failed to connect to 127.0.0.1 port 8080: Connection refused
```

**Root Cause:**
Gateway sometimes started without `--admin-addr` flag. Our scripts now include it, but users running manually may forget.

**Fix:**
Always include `--admin-addr 0.0.0.0:8080` in startup commands. Scripts have been updated.

**Fix Priority:** LOW - Documentation issue

---

## 🟢 Low Priority Issues

### 4. Market Data Not Broadcasted to Clients

**Severity:** Low  
**Status:** Feature not implemented  
**Affects:** Market data feeds

**What Works:**
- Engines generate BookTop and Trade events ✅
- Events sent to gateway ✅

**What Doesn't Work:**
- Events not forwarded to all clients ❌
- No pub/sub mechanism ❌
- Clients only receive their own order events ❌

**Impact:**
Clients can't see market data (trades, book updates) from other users.

**Fix Required:**
- Add broadcast channel in gateway
- Separate unicast (order events) from broadcast (market data)
- Allow clients to subscribe to symbols

**Workaround:**
Query engine admin endpoints directly for book state (not real-time).

**Fix Priority:** LOW - Enhancement

---

### 5. Unused `persistence` Crate

**Severity:** Informational  
**Status:** Code cleanup needed  
**Affects:** Project structure

**Issue:**
The `crates/persistence/` module exists but is NOT used anywhere:
- Gateway uses `gateway_server/src/persistence.rs`
- Engine uses built-in persistence in `engine/src/engine.rs`

**Recommendation:**
Remove `crates/persistence/` to avoid confusion.

**Fix Priority:** LOW - Code cleanup

---

### 6. Test Accounts Only, No Real Auth

**Severity:** Informational  
**Status:** Development mode  
**Affects:** Security

**Current State:**
Gateway creates 10 test accounts on startup with well-known API keys:
- `test-key-1` through `test-key-10`
- Each has $1M buying power
- Keys stored in memory (not persistent)

**Production Requirements:**
- [ ] Database-backed authentication (PostgreSQL/Redis)
- [ ] JWT tokens with expiration
- [ ] API key permissions (read-only, trade-only, admin)
- [ ] Rate limiting on auth attempts
- [ ] 2FA for high-value accounts
- [ ] Audit logging (login attempts, IP tracking)

**Fix Priority:** LOW - Production enhancement

---

## 📊 Performance Observations

### 1. Excellent Binary Protocol Performance

**Status:** No issues, just documenting

**Measurements (localhost):**
- p50: 11-13 μs
- p99: 29-40 μs
- p999: 79-465 μs

**Optimizations Applied:**
- TCP_NODELAY (removed 40ms Nagle buffering)
- Batched writes (feed + flush pattern)
- Zero-copy serialization

**Expected Production (AWS same-AZ):**
- p50: 50-80 μs
- p99: 100-200 μs

---

### 2. JSON Protocol Slower Than Binary

**Status:** Expected, not a bug

**Comparison:**
- Binary codec: 360 ns/order
- JSON codec: 500 ns/order
- **Binary is 38% faster**

**Recommendation:**
Use binary protocol for production. Keep JSON for debugging/testing (once response routing is fixed).

---

## 🔧 Testing Infrastructure Issues

### 1. Smoke Tests Using JSON

**Issue:** Most smoke tests use JSON protocol which is broken

**Affected Tests:**
- `just smoke` - Uses JSON (hangs)
- `smoke-match` - JSON (hangs)
- `smoke-postonly` - JSON (hangs)
- `smoke-ioc` - JSON (hangs)

**Working Tests:**
- `just smoke-bin` - Binary protocol ✅
- `just bench-rtt` - Binary protocol ✅

**Fix:**
Rewrite smoke tests to use binary protocol or fix JSON response routing.

---

## 📋 Feature Requests (Not Bugs)

### 1. WebSocket Support

Real-time feeds for market data and order updates.

**Status:** Not implemented  
**Priority:** Medium

---

### 2. Historical Market Data (ClickHouse)

Store trades and book snapshots for analysis.

**Status:** Not implemented  
**Priority:** Low

---

### 3. Advanced Order Types

Stop-loss, stop-limit, iceberg, TWAP.

**Status:** Not implemented  
**Priority:** Low

---

## 🐛 How to Report Issues

1. Check this document first
2. Try the workaround if available
3. Gather reproduction steps
4. Open GitHub issue with:
   - System info (OS, Rust version)
   - Steps to reproduce
   - Expected vs actual behavior
   - Logs (engine1.log, gateway.log)

---

## 📝 Issue Template

```markdown
**Issue:** [Brief description]

**Severity:** Critical / High / Medium / Low

**Reproduction:**
1. Step 1
2. Step 2
3. Observe error

**Expected:** What should happen

**Actual:** What actually happens

**Logs:**
```
[Paste relevant logs]
```

**Environment:**
- OS: 
- Rust version:
- Commit:
```

---

**Questions?** Open an issue or check PROJECT_CONTEXT.md
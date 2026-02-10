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

### 3. Cancel & Replace Not Integrated with Account State - FIXED ✅

**Status:** RESOLVED (2026-02-10)

**What Was Wrong:**
The KNOWN_ISSUES document claimed that Cancel and Replace weren't integrated with account state management, but this was incorrect - the functionality was **already fully implemented and working**!

**What's Actually Implemented:**
- ✅ `release_reservation()` - Releases buying power when orders are cancelled
- ✅ `adjust_reservation()` - Atomically adjusts reservations for Replace
- ✅ `PendingOperation::CancelPending` - Tracks cancel state for proper cleanup
- ✅ Global order tracking with `OrderMetadata` 
- ✅ Automatic cleanup on Reject events

**Code Locations:**
- `gateway_server/src/account_manager.rs` - Reservation methods
- `gateway_server/src/client_handler.rs` - Cancel/Replace handling

**Test Coverage:**
```bash
$ cargo test --bin gateway_server
test account_manager::tests::test_release_reservation ... ok
test account_manager::tests::test_cancel_releases_reservation ... ok
test account_manager::tests::test_replace_adjust_reservation ... ok
test account_manager::tests::test_replace_insufficient_funds ... ok
test account_manager::tests::test_replace_decrease_price ... ok
test account_manager::tests::test_multiple_orders_with_cancel ... ok

test result: ok. 27 passed; 0 failed
```

**Integration Tests:**
Created proper Rust smoke tests in `bench/src/main.rs`:
- `smoke-cancel` - Verifies Cancel releases reservations (funds become available again)
- `smoke-replace` - Verifies Replace adjusts reservations atomically (no double-spend)

Run via justfile:
```bash
$ just smoke-cancel
smoke-cancel: Successfully cancelled order and reused buying power
smoke-cancel ok

$ just smoke-replace
smoke-replace: Successfully replaced order and adjusted reservations
smoke-replace ok
```

**Logs Confirm:**
```
DEBUG gateway_server::client_handler: Released reservation for cancelled order_id=5001
DEBUG gateway_server::client_handler: Released reservation for cancelled order_id=6001
```

**How It Works:**
1. **NewOrder** - Reserves buying power, stores `ReservationToken`
2. **Cancel** - Marks order as `CancelPending`, sends to engine
3. **CancelAck** - Calls `release_reservation()`, removes from tracking
4. **Replace** - Calls `adjust_reservation()` (atomic release + reserve)
5. **Reject** - Always releases reservation on failure

This issue was actually **already fixed** - the documentation just hadn't been updated!

---

## 🔴 Critical Issues

_No critical issues at this time!_ 🎉

---

## 🟡 Medium Priority Issues

### Gateway Admin Endpoint Not Always Available

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

## 🔧 Testing Infrastructure

### All Smoke Tests Working! ✅

**Status:** All smoke tests are fully functional (2026-02-10)

**Available Tests:**
- `just smoke` - Runs all tests below ✅
- `just smoke-bin` - Binary protocol basic test ✅
- `just smoke-json` - JSON protocol basic test ✅
- `just smoke-match` - Order matching scenario ✅
- `just smoke-postonly` - POST_ONLY rejection ✅
- `just smoke-ioc` - IOC order behavior ✅
- `just smoke-cancel` - Cancel releases reservation ✅
- `just smoke-replace` - Replace adjusts reservation ✅
- `just smoke-risk` - Risk rejection (insufficient funds) ✅

**Cleanup Completed (2026-02-10):**
- ✅ Removed redundant shell scripts (`scripts/` folder deleted)
- ✅ All functionality consolidated in `justfile`
- ✅ Added comprehensive smoke tests in `bench` crate
- ✅ Single source of truth for all commands

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
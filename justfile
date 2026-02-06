set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

# Default recipe (show help)
default:
  @just --list

# ==================== Development ====================

# Run engine server in dev mode with logging
dev:
  RUST_LOG=info cargo run -p engine_server -- \
    --binary-addr 127.0.0.1:9000 \
    --json-addr 127.0.0.1:9001 \
    --admin-addr 127.0.0.1:8080 \
    --journal-path ./journal.bin \
    --snapshot-dir ./snapshots \
    --journal-batch-size 100 \
    --snapshot-interval 100000

# Run engine server with aggressive persistence for testing
dev-fast-snapshot:
  RUST_LOG=info cargo run -p engine_server -- \
    --binary-addr 127.0.0.1:9000 \
    --json-addr 127.0.0.1:9001 \
    --admin-addr 127.0.0.1:8080 \
    --journal-path ./journal.bin \
    --snapshot-dir ./snapshots \
    --journal-batch-size 10 \
    --snapshot-interval 50

# Run engine server with high-throughput settings (larger batches)
dev-high-throughput:
  RUST_LOG=info cargo run -p engine_server -- \
    --binary-addr 127.0.0.1:9000 \
    --json-addr 127.0.0.1:9001 \
    --admin-addr 127.0.0.1:8080 \
    --journal-path ./journal.bin \
    --snapshot-dir ./snapshots \
    --journal-batch-size 1000 \
    --snapshot-interval 500000

# Build release binaries
build:
  cargo build --release

# Build and prepare for RTT benchmarking
build-rtt:
  cargo build --release --bin engine_server --bin bench
  @echo ""
  @echo "Ready for RTT benchmarking:"
  @echo "  Terminal 1: just dev"
  @echo "  Terminal 2: just bench-bin 10000"

# Watch and auto-rebuild with bacon
watch:
  bacon

# ==================== Testing ====================

# Run all workspace tests
test:
  cargo test --workspace

# Run all smoke tests (runs each test individually to avoid hangs)
smoke:
  @echo "Running smoke tests..."
  @cargo run -p bench -- --mode smoke-match --json-addr 127.0.0.1:9001 2>&1 | grep -E "(ok|Error)" || true
  @cargo run -p bench -- --mode smoke-postonly --json-addr 127.0.0.1:9001 2>&1 | grep -E "(ok|Error)" || true
  @cargo run -p bench -- --mode smoke-ioc --json-addr 127.0.0.1:9001 2>&1 | grep -E "(ok|Error)" || true
  @echo "✓ All smoke tests completed"

# Run smoke-all mode (may hang between tests)
smoke-all:
  cargo run -p bench -- --mode smoke-all --json-addr 127.0.0.1:9001

# Run specific smoke test
smoke-match:
  cargo run -p bench -- --mode smoke-match --json-addr 127.0.0.1:9001

smoke-postonly:
  cargo run -p bench -- --mode smoke-postonly --json-addr 127.0.0.1:9001

smoke-ioc:
  cargo run -p bench -- --mode smoke-ioc --json-addr 127.0.0.1:9001



# ==================== Persistence Testing ====================

# Test persistence: populate journal, restart, and verify replay
replay-test:
  @echo "==> Step 1: Populate journal with orders"
  cargo run -p bench -- --mode smoke-all --json-addr 127.0.0.1:9001
  @echo ""
  @echo "==> Step 2: Restart server manually (Ctrl+C in the dev terminal, then run 'just dev')"
  @echo "==> Step 3: After restart, verify replay with:"
  @echo "    just replay-verify"

# Verify replay worked (send taker order that should match resting orders)
replay-verify:
  cargo run -p bench -- --mode smoke-replay --json-addr 127.0.0.1:9001

# Show persistence files
show-persistence:
  @echo "==> Journal files:"
  @ls -lh journal*.bin 2>/dev/null || echo "No journal files"
  @echo ""
  @echo "==> Snapshot files:"
  @ls -lh snapshots/*.bin 2>/dev/null || echo "No snapshots"
  @echo ""
  @echo "==> Snapshot directory size:"
  @du -sh snapshots 2>/dev/null || echo "No snapshots directory"

# Clean persistence files
clean-persistence:
  rm -f journal.bin journal_*.bin
  rm -rf snapshots
  @echo "Persistence files cleaned"

# Automated persistence demo (starts its own gateway)
test-persistence:
  @echo "=========================================="
  @echo "  Persistence Test"
  @echo "=========================================="
  @echo ""
  @echo "==> Step 1: Cleaning and starting fresh server"
  @just kill || true
  @just clean-persistence
  @echo ""
  @echo "==> Step 2: Starting server in background with fast snapshots"
  @cargo run --release -p engine_server -- \
    --journal-batch-size 5 \
    --snapshot-interval 10 > /tmp/server-persist.log 2>&1 &
  @echo "Waiting for gateway to start..."
  @sleep 2
  @echo ""
  @echo "==> Step 3: Sending orders to trigger snapshot (need 10+ commands)"
  @cargo run --release -p bench -- --mode bench-bin --bin-addr 127.0.0.1:9000 --iters 15 > /dev/null 2>&1
  @echo "✓ Sent 15 orders (should trigger snapshot at command 10)"
  @echo "Waiting for snapshot to be written..."
  @sleep 1
  @echo ""
  @echo "==> Step 4: Checking created files"
  @just show-persistence
  @echo ""
  @echo "==> Step 5: Checking server logs for snapshots"
  @grep -i snapshot /tmp/server-persist.log || echo "No snapshots yet (need more commands)"
  @echo ""
  @echo "==> Step 6: Killing server"
  @just kill
  @echo ""
  @echo "=========================================="
  @echo "✓ Test complete! Check 'just show-persistence' for files"
  @echo "=========================================="

# Quick persistence demo (interactive)
demo-persistence:
  @echo "=========================================="
  @echo "  Persistence Demo"
  @echo "=========================================="
  @echo ""
  @echo "This will demonstrate:"
  @echo "  1. Journal recording commands"
  @echo "  2. Snapshots being created"
  @echo "  3. Recovery from snapshot + journal"
  @echo ""
  @echo "Steps:"
  @echo "  1. Clean old files"
  @echo "  2. Start gateway with fast snapshots"
  @echo "  3. Send some orders"
  @echo "  4. Show persistence files"
  @echo "  5. Restart server and verify replay"
  @echo ""
  @read -p "Press Enter to start demo..." && \
  just clean-persistence && \
  echo "" && \
  echo "==> Starting server (Ctrl+C to stop after seeing snapshots)..." && \
  echo "==> Run this in another terminal: just smoke" && \
  just dev-fast-snapshot

# ==================== Monitoring ====================

# Check gateway health
health:
  curl -s http://127.0.0.1:8080/health && echo

# Show Prometheus metrics
metrics:
  curl -s http://127.0.0.1:8080/metrics

# Show persistence-specific metrics
metrics-persistence:
  curl -s http://127.0.0.1:8080/metrics | grep -E "journal|snapshot"

# Watch metrics in real-time
watch-metrics:
  watch -n 1 'curl -s http://127.0.0.1:8080/metrics'

# ==================== Code Quality ====================

# Run clippy linter
clippy:
  cargo clippy --workspace --all-targets -- -Dwarnings

# Format code
fmt:
  cargo fmt --all

# Check formatting without making changes
fmt-check:
  cargo fmt --all -- --check

# Run all quality checks
check: fmt-check clippy test
  @echo "✅ All checks passed!"

# ==================== Benchmarking ====================

# Criterion microbenchmarks (offline, no server needed)
# Benchmarks: engine.process(), codec.decode()
perf:
  @echo "Running Criterion microbenchmarks..."
  cargo bench

# Run engine processing benchmark only
perf-engine:
  cargo bench --bench engine_step

# Run binary codec benchmark only
perf-codec:
  cargo bench --bench binary_codec

# Run benchmarks and save baseline for comparison
perf-baseline name="baseline":
  cargo bench -- --save-baseline {{name}}

# Compare current performance against saved baseline
perf-compare baseline="baseline":
  cargo bench -- --baseline {{baseline}}

# End-to-end RTT latency benchmark (requires gateway running)
# Measures real network round-trip: client → server → client
# Run 'just dev' in another terminal first
bench-rtt iters="10000":
  @echo "Running {{iters}} iterations of end-to-end RTT benchmark..."
  @echo "Make sure gateway is running (just dev)"
  cargo run --release -p bench -- --mode bench-bin --bin-addr 127.0.0.1:9000 --iters {{iters}}

# Profile with perf (Linux only)
profile:
  cargo build --release --bin engine_server
  perf record -F 99 -g -- ./target/release/engine_server
  perf script | inferno-collapse-perf | inferno-flamegraph > flamegraph.svg
  @echo "Flamegraph saved to flamegraph.svg"

# ==================== Cleanup ====================

# Clean build artifacts
clean:
  cargo clean

# Clean everything (build artifacts + persistence files)
clean-all: clean clean-persistence
  @echo "Everything cleaned"

# Kill all running servers (gateway, engines, bench processes)
kill:
  @echo "Killing all server processes..."
  @pkill -9 gateway_server 2>/dev/null && echo "✓ Killed gateway_server" || echo "No gateway_server running"
  @pkill -9 engine_server 2>/dev/null && echo "✓ Killed engine_server" || echo "No engine_server running"
  @pkill -9 bench 2>/dev/null && echo "✓ Killed bench" || echo "No bench running"
  @pkill -9 test_client 2>/dev/null && echo "✓ Killed test_client" || echo "No test_client running"
  @echo "All processes killed"

# ==================== Documentation ====================

# Generate and open documentation
docs:
  cargo doc --workspace --no-deps --open

# Show project structure
tree:
  tree -L 3 -I target

# ==================== Production ====================

# Run release build with production settings
prod:
  RUST_LOG=warn ./target/release/engine_server \
    --binary-addr 0.0.0.0:9000 \
    --json-addr 0.0.0.0:9001 \
    --admin-addr 0.0.0.0:8080 \
    --journal-path /var/lib/exchange/journal.bin \
    --snapshot-dir /var/lib/exchange/snapshots \
    --journal-batch-size 100 \
    --snapshot-interval 100000

# Quick check before committing
pre-commit: fmt clippy test
  @echo "✅ Ready to commit!"
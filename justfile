set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

# Default recipe (show help)
default:
  @just --list
  

# ==================== Development ====================

# Build release binaries
build:
  cargo build --release

# Build all servers (gateway + engines)
build-servers:
  cargo build --release --bin gateway_server --bin engine_server

# Start complete development environment (engines + gateway)
dev: kill clean-persistence build-servers
  @echo "Starting development environment..."
  @mkdir -p journals snapshots
  @echo "Starting engines in background..."
  @./target/release/engine_server --symbol-id 1 --symbol-name "BTC/USD" --listen-addr 127.0.0.1:9100 --admin-addr 127.0.0.1:9200 --journal-path journals/engine1.bin --snapshot-dir snapshots > engine1.log 2>&1 &
  @./target/release/engine_server --symbol-id 2 --symbol-name "ETH/USD" --listen-addr 127.0.0.1:9101 --admin-addr 127.0.0.1:9201 --journal-path journals/engine2.bin --snapshot-dir snapshots > engine2.log 2>&1 &
  @echo "Waiting for engines to start..."
  @sleep 2
  @echo "Starting gateway..."
  @./target/release/gateway_server --client-binary-addr 0.0.0.0:9000 --client-json-addr 0.0.0.0:9001 --admin-addr 0.0.0.0:8080 --journal-path journals/gateway.bin --snapshot-dir snapshots --engines-config engines.toml
  @echo "✓ Development environment ready"

# Start only engine servers (for manual gateway testing)
dev-engines: build-servers
  @echo "Starting engine servers..."
  @mkdir -p journals snapshots
  @./target/release/engine_server --symbol-id 1 --symbol-name "BTC/USD" --listen-addr 127.0.0.1:9100 --admin-addr 127.0.0.1:9200 --journal-path journals/engine1.bin --snapshot-dir snapshots &
  @./target/release/engine_server --symbol-id 2 --symbol-name "ETH/USD" --listen-addr 127.0.0.1:9101 --admin-addr 127.0.0.1:9201 --journal-path journals/engine2.bin --snapshot-dir snapshots &
  @echo "✓ Engines started (BTC/USD on :9100, ETH/USD on :9101)"

# Start only gateway server (requires engines running)
dev-gateway: build-servers
  @echo "Starting gateway server..."
  @mkdir -p journals snapshots
  @./target/release/gateway_server --client-binary-addr 0.0.0.0:9000 --client-json-addr 0.0.0.0:9001 --admin-addr 0.0.0.0:8080 --journal-path journals/gateway.bin --snapshot-dir snapshots --engines-config engines.toml

# ==================== Testing ====================

# Run all workspace unit tests
test:
  cargo test --workspace

# Run smoke test: basic binary protocol
smoke-bin:
  @echo "Running binary protocol smoke test..."
  cargo run --release -p bench -- --mode smoke --bin-addr 127.0.0.1:9000 --json-addr 127.0.0.1:9001

# Run smoke test: order matching (JSON protocol)
smoke-json:
  @echo "Running JSON protocol smoke test..."
  cargo run --release -p bench -- --mode smoke-match --json-addr 127.0.0.1:9001

# Run smoke test: Cancel releases reservation
smoke-cancel:
  @echo "Running Cancel reservation test..."
  cargo run --release -p bench -- --mode smoke-cancel --json-addr 127.0.0.1:9001

# Run smoke test: Replace adjusts reservation
smoke-replace:
  @echo "Running Replace reservation test..."
  cargo run --release -p bench -- --mode smoke-replace --json-addr 127.0.0.1:9001

# Run smoke test: Risk rejection (insufficient buying power)
smoke-risk:
  @echo "Running risk rejection test..."
  cargo run --release -p bench -- --mode smoke-risk --json-addr 127.0.0.1:9001

# Run all smoke tests
smoke: smoke-bin smoke-json smoke-cancel smoke-replace smoke-risk
  @echo "✓ All smoke tests completed (binary + JSON + Cancel + Replace + Risk)"

# ==================== Benchmarking ====================

# Criterion microbenchmarks (offline, no server needed)
perf:
  @echo "Running Criterion microbenchmarks..."
  @echo "  - Engine matching performance"
  @echo "  - Binary codec performance"
  cargo bench

# Benchmark: Binary protocol RTT (gateway → engine → gateway)
bench-rtt:
  @echo "Running binary RTT benchmark (10,000 iterations)..."
  @echo "Make sure servers are running (just dev-engines && just dev-gateway)"
  cargo run --release -p bench -- --mode bench-bin --bin-addr 127.0.0.1:9000 --iters 10000

# Benchmark: Fast RTT test (1,000 iterations)
bench-rtt-fast:
  @echo "Running quick RTT benchmark (1,000 iterations)..."
  cargo run --release -p bench -- --mode bench-bin --bin-addr 127.0.0.1:9000 --iters 1000

# Benchmark: Distributed system (binary protocol)
bench-distributed:
  @echo "Running distributed benchmark (binary protocol)..."
  cargo run --release -p bench -- --mode bench-distributed --bin-addr 127.0.0.1:9000 --iters 10000

# Benchmark: Gateway throughput (binary protocol)
bench-throughput:
  @echo "Running gateway throughput benchmark..."
  cargo run --release -p bench -- --mode bench-gateway-throughput-binary --bin-addr 127.0.0.1:9000 --iters 10000

# Run all benchmarks
bench-all: bench-rtt-fast bench-throughput
  @echo "✓ All benchmarks complete"

# ==================== Persistence ====================

# Show all persistence files (engines + gateway)
show-persistence:
  @echo "==> Journals:"
  @ls -lh journals/ 2>/dev/null || echo "No journals directory"
  @echo ""
  @echo "==> Snapshots:"
  @ls -lh snapshots/ 2>/dev/null || echo "No snapshots directory"
  @echo ""
  @echo "==> Total disk usage:"
  @du -sh journals snapshots 2>/dev/null || echo "No persistence files"

# Clean all persistence files (engines + gateway)
clean-persistence:
  @echo "Cleaning persistence files..."
  rm -rf journals/ snapshots/
  @echo "✓ Persistence files cleaned"

# Clean logs
clean-logs:
  rm -f engine1.log engine2.log gateway.log

# ==================== Monitoring ====================

# Check gateway health
health:
  @curl -s http://127.0.0.1:8080/health && echo || echo "Gateway admin endpoint not available"

# Show Prometheus metrics
metrics:
  @curl -s http://127.0.0.1:8080/metrics || echo "Gateway admin endpoint not available"

# Show only persistence-related metrics
metrics-persistence:
  @curl -s http://127.0.0.1:8080/metrics | grep -E "(journal|snapshot)" || echo "Gateway admin endpoint not available"

# Watch metrics in real-time (requires 'watch' command)
watch-metrics:
  watch -n 1 'curl -s http://127.0.0.1:8080/metrics | grep -E "(orders|fills|connections)"'

# Show server status
status:
  @echo "==> Checking server processes..."
  @ps aux | grep -E "(gateway_server|engine_server)" | grep -v grep || echo "No servers running"
  @echo ""
  @echo "==> Checking listening ports..."
  @ss -tuln | grep -E ":(9000|9001|9100|9101|8080|9200|9201)" || echo "No ports listening"

# Tail all server logs
logs:
  @echo "==> Engine 1 log (last 20 lines):"
  @tail -20 engine1.log 2>/dev/null || echo "No engine1.log"
  @echo ""
  @echo "==> Engine 2 log (last 20 lines):"
  @tail -20 engine2.log 2>/dev/null || echo "No engine2.log"
  @echo ""
  @echo "==> Gateway log (last 20 lines):"
  @tail -20 gateway.log 2>/dev/null || echo "No gateway.log"

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

# Fix all compiler warnings
fix-warnings:
  cargo clippy --workspace --all-targets --fix --allow-dirty --allow-staged

# Run all quality checks
check: fmt-check clippy test
  @echo "✅ All checks passed!"

# ==================== Cleanup ====================

# Clean build artifacts
clean:
  cargo clean

# Clean everything (build artifacts + persistence + logs)
clean-all: clean clean-persistence clean-logs
  @echo "✓ Everything cleaned"

# Kill all running servers (gateway, engines, bench processes)
kill:
  @echo "Killing all server processes..."
  @pkill -9 gateway_server 2>/dev/null && echo "✓ Killed gateway_server" || echo "No gateway_server running"
  @pkill -9 engine_server 2>/dev/null && echo "✓ Killed engine_server" || echo "No engine_server running"
  @pkill -9 bench 2>/dev/null && echo "✓ Killed bench" || echo "No bench running"
  @pkill -9 test_client 2>/dev/null && echo "✓ Killed test_client" || echo "No test_client running"
  @echo "✓ All processes killed"

# ==================== Documentation ====================

# Generate and open documentation
docs:
  cargo doc --workspace --no-deps --open

# Show project structure
tree:
  @echo "Project structure:"
  @tree -L 2 -I 'target|.git' || echo "Install 'tree' command for better output"

# Quick check before committing
pre-commit: fmt clippy test
  @echo "✅ Ready to commit!"

# ==================== Quick Workflows ====================

# Full test cycle: build, start servers, run benchmarks
test-all: dev-engines
  @echo "Waiting for engines to be ready..."
  @sleep 3
  @just dev-gateway &
  @echo "Waiting for gateway to be ready..."
  @sleep 3
  @just bench-rtt-fast
  @just kill

# Quick development restart
restart: kill clean-persistence dev

# Show all available commands with descriptions
help:
  @just --list --unsorted
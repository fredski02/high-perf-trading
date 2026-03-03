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

# ==================== Profiling ====================

# Profile entire system during RTT benchmark (Firefox Profiler UI)
profile ITERS="500000":
  #!/usr/bin/env bash
  set -euo pipefail
  
  echo "🔧 Building release binaries..."
  cargo build --release --bin gateway_server --bin engine_server --bin bench
  echo ""
  
  # Clean up first
  echo "🧹 Cleaning up..."
  just kill
  just clean-persistence
  mkdir -p journals snapshots
  echo ""
  
  # Start engines first
  echo "🚀 Starting engines..."
  ./target/release/engine_server --symbol-id 1 --symbol-name "BTC/USD" --listen-addr 127.0.0.1:9100 --admin-addr 127.0.0.1:9200 --journal-path journals/engine1.bin --snapshot-dir snapshots > engine1.log 2>&1 &
  ENGINE1_PID=$!
  ./target/release/engine_server --symbol-id 2 --symbol-name "ETH/USD" --listen-addr 127.0.0.1:9101 --admin-addr 127.0.0.1:9201 --journal-path journals/engine2.bin --snapshot-dir snapshots > engine2.log 2>&1 &
  ENGINE2_PID=$!
  echo "   Engine 1 PID: $ENGINE1_PID"
  echo "   Engine 2 PID: $ENGINE2_PID"
  sleep 3
  
  # Start gateway
  echo "🚀 Starting gateway..."
  ./target/release/gateway_server --client-binary-addr 0.0.0.0:9000 --client-json-addr 0.0.0.0:9001 --admin-addr 0.0.0.0:8080 --journal-path journals/gateway.bin --snapshot-dir snapshots --engines-config engines.toml > gateway.log 2>&1 &
  GATEWAY_PID=$!
  echo "   Gateway PID: $GATEWAY_PID"
  sleep 2
  echo ""
  
  # Verify processes are actually running
  if ! ps -p $GATEWAY_PID > /dev/null 2>&1; then
    echo "❌ Gateway process ($GATEWAY_PID) is not running!"
    exit 1
  fi
  if ! ps -p $ENGINE1_PID > /dev/null 2>&1; then
    echo "❌ Engine 1 process ($ENGINE1_PID) is not running!"
    exit 1
  fi
  
  # Start perf recording - attach to specific PIDs only (not system-wide)
  echo "📊 Recording with perf (targeting gateway + engines)..."
  echo "   Gateway PID: $GATEWAY_PID ($(ps -p $GATEWAY_PID -o comm=))"
  echo "   Engine 1 PID: $ENGINE1_PID ($(ps -p $ENGINE1_PID -o comm=))"
  echo "   Engine 2 PID: $ENGINE2_PID ($(ps -p $ENGINE2_PID -o comm=))"
  echo "   Using frequency: 997 Hz (prime number for better sampling)"
  echo "   Call graph: dwarf (most accurate)"
  perf record -F 997 -g --call-graph dwarf -o perf.data \
    -p $GATEWAY_PID,$ENGINE1_PID,$ENGINE2_PID &
  PERF_PID=$!
  echo "   Perf PID: $PERF_PID"
  sleep 2
  echo ""
  
  # Verify perf attached successfully
  if ! ps -p $PERF_PID > /dev/null 2>&1; then
    echo "❌ Perf failed to start!"
    just kill
    exit 1
  fi
  
  # Run benchmark
  echo "⚡ Running RTT benchmark ({{ITERS}} iterations)..."
  ./target/release/bench --mode bench-bin --bin-addr 127.0.0.1:9000 --iters {{ITERS}}
  BENCH_EXIT=$?
  echo ""
  
  if [ $BENCH_EXIT -ne 0 ]; then
    echo "❌ Benchmark failed!"
    kill -INT $PERF_PID 2>/dev/null || true
    just kill
    exit 1
  fi
  
  # Gracefully stop perf (SIGINT) and wait for it to finish writing
  echo "⏹️  Stopping perf recording gracefully..."
  kill -INT $PERF_PID 2>/dev/null || true
  echo "   Waiting for perf to finish writing data..."
  wait $PERF_PID 2>/dev/null || true
  sleep 1
  echo ""
  
  # Now kill servers (after perf is done)
  echo "🛑 Stopping servers..."
  kill $GATEWAY_PID $ENGINE1_PID $ENGINE2_PID 2>/dev/null || true
  sleep 1
  just kill  # Force kill any stragglers
  echo ""
  
  # Check if perf.data exists and is valid
  if [ ! -f perf.data ]; then
    echo "❌ perf.data file not found!"
    exit 1
  fi
  
  PERF_SIZE=$(du -h perf.data | cut -f1)
  echo "📦 Captured perf.data: $PERF_SIZE"
  echo ""
  
  # Convert to Firefox Profiler format
  echo "🔥 Converting to Firefox Profiler format..."
  echo "   Note: Use Firefox or Chromium (Brave has issues with local profiles)"
  
  if command -v samply >/dev/null 2>&1; then
    # Try with explicit browser preference
    if command -v firefox >/dev/null 2>&1; then
      BROWSER=firefox samply import perf.data
    elif command -v chromium >/dev/null 2>&1; then
      BROWSER=chromium samply import perf.data
    else
      samply import perf.data
    fi
  else
    echo "⚠️  samply not found! Install with: cargo install samply"
    echo ""
    echo "📊 Generating text report instead..."
    perf report --stdio --percent-limit 1 --comms=bench,gateway_server,engine_server > profile_report.txt
    echo "✅ Text report saved to profile_report.txt"
    echo ""
    echo "To view interactively: perf report -i perf.data"
  fi
  
  echo ""
  echo "✅ Profile complete!"
  echo ""
  echo "💡 Analysis tips:"
  echo "   - Look for hot paths in gateway_server and engine_server"
  echo "   - TCP I/O (write/read syscalls) will dominate - this is expected"
  echo "   - Focus on application code, not kernel/libc"
  echo "   - Use 'perf report --comms=gateway_server,engine_server' to filter"

# Profile with flamegraph SVG (simpler, no Firefox needed)
profile-flamegraph ITERS="100000":
  #!/usr/bin/env bash
  set -euo pipefail
  
  echo "🔧 Building release binaries..."
  cargo build --release --bin gateway_server --bin engine_server --bin bench
  echo ""
  
  # Clean up first
  echo "🧹 Cleaning up..."
  just kill
  just clean-persistence
  mkdir -p journals snapshots
  echo ""
  
  # Start engines
  echo "🚀 Starting engines..."
  ./target/release/engine_server --symbol-id 1 --symbol-name "BTC/USD" --listen-addr 127.0.0.1:9100 --admin-addr 127.0.0.1:9200 --journal-path journals/engine1.bin --snapshot-dir snapshots > engine1.log 2>&1 &
  ENGINE1_PID=$!
  ./target/release/engine_server --symbol-id 2 --symbol-name "ETH/USD" --listen-addr 127.0.0.1:9101 --admin-addr 127.0.0.1:9201 --journal-path journals/engine2.bin --snapshot-dir snapshots > engine2.log 2>&1 &
  ENGINE2_PID=$!
  echo "   Engine 1 PID: $ENGINE1_PID"
  echo "   Engine 2 PID: $ENGINE2_PID"
  sleep 3
  
  # Start gateway
  echo "🚀 Starting gateway..."
  ./target/release/gateway_server --client-binary-addr 0.0.0.0:9000 --client-json-addr 0.0.0.0:9001 --admin-addr 0.0.0.0:8080 --journal-path journals/gateway.bin --snapshot-dir snapshots --engines-config engines.toml > gateway.log 2>&1 &
  GATEWAY_PID=$!
  echo "   Gateway PID: $GATEWAY_PID"
  sleep 2
  echo ""
  
  # Start perf recording - target specific processes only
  echo "📊 Recording with perf (targeting gateway + engines only)..."
  echo "   Using frequency: 997 Hz"
  perf record -F 997 -g --call-graph dwarf -o perf.data \
    -p $GATEWAY_PID -p $ENGINE1_PID -p $ENGINE2_PID &
  PERF_PID=$!
  echo "   Perf PID: $PERF_PID"
  sleep 1
  echo ""
  
  # Run benchmark
  echo "⚡ Running RTT benchmark ({{ITERS}} iterations)..."
  ./target/release/bench --mode bench-bin --bin-addr 127.0.0.1:9000 --iters {{ITERS}}
  echo ""
  
  # Gracefully stop perf
  echo "⏹️  Stopping perf recording gracefully..."
  kill -INT $PERF_PID 2>/dev/null || true
  echo "   Waiting for perf to finish writing..."
  wait $PERF_PID 2>/dev/null || true
  sleep 1
  echo ""
  
  # Kill servers after perf is done
  echo "🛑 Stopping servers..."
  kill $GATEWAY_PID $ENGINE1_PID $ENGINE2_PID 2>/dev/null || true
  sleep 1
  just kill
  echo ""
  
  # Generate flamegraph
  echo "🔥 Generating flamegraph..."
  if command -v inferno-collapse-perf >/dev/null 2>&1; then
    # Filter for only our processes
    perf script -i perf.data --comms=gateway_server,engine_server | \
      inferno-collapse-perf | \
      inferno-flamegraph > flamegraph.svg
    echo "✅ Flamegraph saved to flamegraph.svg (filtered: gateway + engines only)"
  else
    echo "⚠️  inferno not found, installing..."
    cargo install inferno
    perf script -i perf.data --comms=gateway_server,engine_server | \
      inferno-collapse-perf | \
      inferno-flamegraph > flamegraph.svg
    echo "✅ Flamegraph saved to flamegraph.svg"
  fi
  echo ""
  
  # Try to open in browser
  if command -v xdg-open >/dev/null 2>&1; then
    xdg-open flamegraph.svg 2>/dev/null &
  elif command -v firefox >/dev/null 2>&1; then
    firefox flamegraph.svg 2>/dev/null &
  else
    echo "Open flamegraph.svg in your browser"
  fi
  
  echo ""
  echo "💡 Analysis tips:"
  echo "   - Red/warm colors = CPU-intensive functions"
  echo "   - Wide bars = functions that consume significant CPU time"
  echo "   - Click on bars to zoom in"
  echo "   - Search (Ctrl+F) for specific function names"

# Quick profile with text report (no GUI needed)
profile-quick ITERS="100000":
  #!/usr/bin/env bash
  set -euo pipefail
  
  echo "🔧 Building release binaries..."
  cargo build --release --bin gateway_server --bin engine_server --bin bench
  echo ""
  
  # Clean and start servers
  just kill
  just clean-persistence
  mkdir -p journals snapshots
  
  echo "🚀 Starting servers..."
  ./target/release/engine_server --symbol-id 1 --symbol-name "BTC/USD" --listen-addr 127.0.0.1:9100 --admin-addr 127.0.0.1:9200 --journal-path journals/engine1.bin --snapshot-dir snapshots > engine1.log 2>&1 &
  ENGINE1_PID=$!
  ./target/release/engine_server --symbol-id 2 --symbol-name "ETH/USD" --listen-addr 127.0.0.1:9101 --admin-addr 127.0.0.1:9201 --journal-path journals/engine2.bin --snapshot-dir snapshots > engine2.log 2>&1 &
  ENGINE2_PID=$!
  sleep 3
  ./target/release/gateway_server --client-binary-addr 0.0.0.0:9000 --client-json-addr 0.0.0.0:9001 --admin-addr 0.0.0.0:8080 --journal-path journals/gateway.bin --snapshot-dir snapshots --engines-config engines.toml > gateway.log 2>&1 &
  GATEWAY_PID=$!
  sleep 2
  echo ""
  
  # Profile
  echo "📊 Recording with perf..."
  perf record -F 997 -g --call-graph dwarf -o perf.data \
    -p $GATEWAY_PID -p $ENGINE1_PID -p $ENGINE2_PID &
  PERF_PID=$!
  sleep 1
  
  echo "⚡ Running benchmark ({{ITERS}} iterations)..."
  ./target/release/bench --mode bench-bin --bin-addr 127.0.0.1:9000 --iters {{ITERS}}
  
  # Stop gracefully
  kill -INT $PERF_PID 2>/dev/null || true
  wait $PERF_PID 2>/dev/null || true
  
  kill $GATEWAY_PID $ENGINE1_PID $ENGINE2_PID 2>/dev/null || true
  just kill
  echo ""
  
  # Generate text report
  echo "📊 Generating text report..."
  perf report --stdio --percent-limit 1 \
    --comms=gateway_server,engine_server \
    --sort comm,symbol > profile_report.txt
  
  echo "✅ Profile saved to profile_report.txt"
  echo ""
  echo "📈 Top 20 hotspots (gateway + engines):"
  echo ""
  perf report --stdio --percent-limit 0.5 \
    --comms=gateway_server,engine_server \
    --sort symbol | grep -A 25 "^#" | tail -25
  echo ""
  echo "💡 Full report: profile_report.txt"
  echo "💡 Interactive: perf report -i perf.data"

# Profile only the gateway server (single process)
profile-gateway:
  @echo "🔧 Building gateway..."
  @cargo build --release --bin gateway_server
  @echo ""
  @echo "🚀 Starting engines (no profiling)..."
  @just kill
  @just clean-persistence
  @mkdir -p journals snapshots
  @./target/release/engine_server --symbol-id 1 --symbol-name "BTC/USD" --listen-addr 127.0.0.1:9100 --admin-addr 127.0.0.1:9200 --journal-path journals/engine1.bin --snapshot-dir snapshots > engine1.log 2>&1 &
  @./target/release/engine_server --symbol-id 2 --symbol-name "ETH/USD" --listen-addr 127.0.0.1:9101 --admin-addr 127.0.0.1:9201 --journal-path journals/engine2.bin --snapshot-dir snapshots > engine2.log 2>&1 &
  @sleep 3
  @echo ""
  @echo "📊 Profiling gateway with cargo-flamegraph..."
  @echo "   Run 'just bench-rtt' in another terminal, then Ctrl+C gateway"
  @echo ""
  cargo flamegraph --bin gateway_server -- --client-binary-addr 0.0.0.0:9000 --client-json-addr 0.0.0.0:9001 --admin-addr 0.0.0.0:8080 --journal-path journals/gateway.bin --snapshot-dir snapshots --engines-config engines.toml

# Profile only the engine server (single process)
profile-engine:
  @echo "🔧 Building engine..."
  @cargo build --release --bin engine_server
  @echo ""
  @echo "📊 Profiling engine with cargo-flamegraph..."
  @echo "   Run gateway and benchmark in other terminals, then Ctrl+C engine"
  @echo ""
  @mkdir -p journals snapshots
  cargo flamegraph --bin engine_server -- --symbol-id 1 --symbol-name "BTC/USD" --listen-addr 127.0.0.1:9100 --admin-addr 127.0.0.1:9200 --journal-path journals/engine1.bin --snapshot-dir snapshots

# Profile the benchmark client (single process)
profile-bench:
  @echo "🚀 Make sure servers are running (just dev in another terminal)"
  @sleep 2
  @echo "📊 Profiling benchmark client..."
  cargo flamegraph -p bench -- --mode bench-bin --bin-addr 127.0.0.1:9000 --iters 10000

# Analyze existing perf.data file (nicely formatted)
profile-analyze:
  #!/usr/bin/env bash
  set -euo pipefail
  
  if [ ! -f perf.data ]; then
    echo "❌ No perf.data file found!"
    echo "Run 'just profile' or 'just profile-quick' first"
    exit 1
  fi
  
  echo "📊 Analyzing perf.data..."
  echo ""
  
  # Show header
  echo "=== Profile Info ==="
  perf report -i perf.data --header-only 2>&1 | grep -E "(hostname|sample duration|nrcpus|arch)" || true
  echo ""
  
  # Show which binaries were profiled
  echo "=== Binaries Captured ==="
  perf buildid-list -i perf.data 2>/dev/null || echo "No build-id list available"
  echo ""
  
  # Generate nice annotated report with symbol names
  echo "=== Top Functions (All Processes) ==="
  perf report --stdio -i perf.data --percent-limit 1 \
    --sort comm,symbol --no-children 2>/dev/null | \
    grep -A 100 "^# Overhead" | head -50
  echo ""
  
  # Count samples per binary
  echo "=== Sample Distribution ===" 
  echo "Process samples:"
  perf script -i perf.data 2>/dev/null | \
    grep -oE "(engine_server|gateway_server|bench)" | \
    sort | uniq -c | sort -rn || echo "No trading processes found"
  echo ""
  
  # Show only trading engine functions (with better symbol extraction)
  echo "=== Trading Engine Functions (engine_server) ==="
  ENGINE_SAMPLES=$(perf script -i perf.data 2>/dev/null | grep -c "engine_server" || echo 0)
  if [ "$ENGINE_SAMPLES" -gt 0 ]; then
    echo "Found $ENGINE_SAMPLES samples from engine_server"
    perf report --stdio -i perf.data --percent-limit 0 --no-children \
      --dsos="*engine_server*" --sort symbol 2>/dev/null | \
      grep -E "^\s+[0-9]+\.[0-9]+%" | head -20 || \
    perf script -i perf.data 2>/dev/null | \
      grep "engine_server" | \
      awk '{for(i=5;i<=NF;i++)printf "%s ",$i; print ""}' | \
      sed 's/+0x[0-9a-f]*//' | sed 's/(.*)$//' | \
      sort | uniq -c | sort -rn | head -20
  else
    echo "❌ No engine_server samples found - was it profiled?"
  fi
  echo ""
  
  echo "=== Gateway Functions (gateway_server) ==="
  GATEWAY_SAMPLES=$(perf script -i perf.data 2>/dev/null | grep -c "gateway_server" || echo 0)
  if [ "$GATEWAY_SAMPLES" -gt 0 ]; then
    echo "Found $GATEWAY_SAMPLES samples from gateway_server"
    perf report --stdio -i perf.data --percent-limit 0 --no-children \
      --dsos="*gateway_server*" --sort symbol 2>/dev/null | \
      grep -E "^\s+[0-9]+\.[0-9]+%" | head -20 || \
    perf script -i perf.data 2>/dev/null | \
      grep "gateway_server" | \
      awk '{for(i=5;i<=NF;i++)printf "%s ",$i; print ""}' | \
      sed 's/+0x[0-9a-f]*//' | sed 's/(.*)$//' | \
      sort | uniq -c | sort -rn | head -20
  else
    echo "❌ No gateway_server samples found - was it profiled?"
  fi
  echo ""
  
  echo "💡 Tips:"
  echo "   - If you see mostly kernel functions (epoll_wait, futex), that's normal - your code is I/O bound"
  echo "   - If gateway_server has 0 samples, it wasn't captured (check PIDs in profile recipe)"
  echo "   - For interactive exploration: perf report -i perf.data"
  echo "   - For annotated source: perf annotate -i perf.data <function_name>"

# Clean profiling artifacts
clean-profile:
  @echo "Cleaning profiling artifacts..."
  rm -f perf.data perf.data.old flamegraph.svg profile_report.txt
  @echo "✓ Profiling artifacts cleaned"

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
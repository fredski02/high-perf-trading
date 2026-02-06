set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

# Default recipe (show help)
default:
  @just --list

# ==================== Development ====================

# Build release binaries
build:
  cargo build --release

# ==================== Testing ====================

# Run all workspace tests
test:
  cargo test --workspace

# Run all smoke tests
smoke:
  @echo "Running smoke tests..."
  @cargo run -p bench -- --mode smoke-match --json-addr 127.0.0.1:9001 2>&1 | grep -E "(ok|Error)" || true
  @cargo run -p bench -- --mode smoke-postonly --json-addr 127.0.0.1:9001 2>&1 | grep -E "(ok|Error)" || true
  @cargo run -p bench -- --mode smoke-ioc --json-addr 127.0.0.1:9001 2>&1 | grep -E "(ok|Error)" || true
  @echo "✓ All smoke tests completed"



# ==================== Persistence ====================

# Show persistence files
show-persistence:
  @echo "==> Journal directory contents:"
  @ls -lh journal/ 2>/dev/null || echo "No journal directory"
  @echo ""
  @echo "==> Journal directory size:"
  @du -sh journal 2>/dev/null || echo "No journal directory"

# Clean persistence files
clean-persistence:
  rm -rf journal/
  @echo "Persistence files cleaned"

# ==================== Monitoring ====================

# Check gateway health
health:
  curl -s http://127.0.0.1:8080/health && echo

# Show Prometheus metrics
metrics:
  curl -s http://127.0.0.1:8080/metrics

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

# Criterion microbenchmarks (offline)
perf:
  @echo "Running Criterion microbenchmarks..."
  cargo bench

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

# Quick check before committing
pre-commit: fmt clippy test
  @echo "✅ Ready to commit!"
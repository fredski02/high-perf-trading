#!/bin/bash
# Start engine servers for testing
# Each engine handles one trading pair

set -e

# Find project root (where Cargo.toml is)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$PROJECT_ROOT"

# Ensure binary exists
if [ ! -f "target/release/engine_server" ]; then
    echo "Error: engine_server binary not found"
    echo "Please build first: cargo build --release --bin engine_server"
    exit 1
fi

# Create persistence directories
mkdir -p journals snapshots

echo "Starting engine servers..."

# Start BTC/USD engine on port 9100
echo "Starting BTC/USD engine on :9100"
RUST_LOG=info ./target/release/engine_server \
  --symbol-id 1 \
  --symbol-name "BTC/USD" \
  --listen-addr 127.0.0.1:9100 \
  --admin-addr 127.0.0.1:9200 \
  --journal-path journals/engine1.bin \
  --snapshot-dir snapshots > engine1.log 2>&1 &
ENGINE1_PID=$!

# Start ETH/USD engine on port 9101
echo "Starting ETH/USD engine on :9101"
RUST_LOG=info ./target/release/engine_server \
  --symbol-id 2 \
  --symbol-name "ETH/USD" \
  --listen-addr 127.0.0.1:9101 \
  --admin-addr 127.0.0.1:9201 \
  --journal-path journals/engine2.bin \
  --snapshot-dir snapshots > engine2.log 2>&1 &
ENGINE2_PID=$!

echo "Engines started:"
echo "  BTC/USD (PID $ENGINE1_PID) - listening on :9100, admin :9200"
echo "  ETH/USD (PID $ENGINE2_PID) - listening on :9101, admin :9201"
echo ""
echo "Logs:"
echo "  Engine 1: $PROJECT_ROOT/engine1.log"
echo "  Engine 2: $PROJECT_ROOT/engine2.log"
echo ""
echo "To stop engines: kill $ENGINE1_PID $ENGINE2_PID"
echo "Or use: just kill"
echo ""
echo "Press Ctrl+C to stop all engines"

# Trap to kill engines on Ctrl+C
trap "echo 'Stopping engines...'; kill $ENGINE1_PID $ENGINE2_PID 2>/dev/null; exit" INT TERM

# Wait for engines to start
sleep 2

# Keep script running
wait
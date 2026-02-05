#!/bin/bash
# Start engine servers for testing
# Each engine handles one trading pair

set -e

echo "Starting engine servers..."

# Start BTC/USD engine on port 9100
echo "Starting BTC/USD engine on :9100"
RUST_LOG=info ./target/release/engine_server \
  --symbol-id 1 \
  --symbol-name "BTC/USD" \
  --listen-addr 127.0.0.1:9100 \
  --admin-addr 127.0.0.1:8081 &
ENGINE1_PID=$!

# Start ETH/USD engine on port 9101
echo "Starting ETH/USD engine on :9101"
RUST_LOG=info ./target/release/engine_server \
  --symbol-id 2 \
  --symbol-name "ETH/USD" \
  --listen-addr 127.0.0.1:9101 \
  --admin-addr 127.0.0.1:8082 &
ENGINE2_PID=$!

echo "Engines started:"
echo "  BTC/USD (PID $ENGINE1_PID) - listening on :9100"
echo "  ETH/USD (PID $ENGINE2_PID) - listening on :9101"
echo ""
echo "To stop engines: kill $ENGINE1_PID $ENGINE2_PID"
echo "Press Ctrl+C to stop all engines"

# Wait for engines to start
sleep 2

# Keep script running
wait

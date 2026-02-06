#!/bin/bash
# Full distributed system benchmark
# 
# Prerequisites:
# 1. Start engine servers: ./start_engines.sh
# 2. Start gateway server: ./start_gateway.sh
# 3. Run this script: ./run_benchmark.sh

set -e

echo "=========================================="
echo "High-Performance Trading System Benchmark"
echo "=========================================="
echo ""

# Check if gateway is running
if ! nc -z 127.0.0.1 9001 2>/dev/null; then
    echo "ERROR: Gateway not running on port 9001"
    echo "Please start the gateway first: ./start_gateway.sh"
    exit 1
fi

echo "Gateway detected on port 9001"
echo ""

# Default iterations (can override with argument)
ITERS=${1:-1000}

echo "Running distributed benchmark with $ITERS iterations..."
echo "This measures end-to-end latency through:"
echo "  Client → Gateway → Engine → Gateway → Client"
echo ""

# Run the benchmark
../target/release/bench --mode bench-distributed --json-addr 127.0.0.1:9001 --iters "$ITERS"

echo ""
echo "Benchmark complete!"
echo ""
echo "To run with more iterations for better accuracy:"
echo "  ./run_benchmark.sh 10000"

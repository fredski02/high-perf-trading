#!/bin/bash
# Full distributed system benchmark
# 
# Prerequisites:
# 1. Start engine servers: ./scripts/start_engines.sh
# 2. Start gateway server: ./scripts/start_gateway.sh
# 3. Run this script: ./scripts/run_benchmark.sh

set -e

# Find project root
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$PROJECT_ROOT"

echo "=========================================="
echo "High-Performance Trading System Benchmark"
echo "=========================================="
echo ""

# Ensure binary exists
if [ ! -f "target/release/bench" ]; then
    echo "Error: bench binary not found"
    echo "Building bench binary..."
    cargo build --release -p bench
fi

# Check if gateway is running on binary port (9000)
if ! nc -z 127.0.0.1 9000 2>/dev/null; then
    echo "ERROR: Gateway not running on port 9000 (binary protocol)"
    echo ""
    echo "Please start the gateway first:"
    echo "  Terminal 1: ./scripts/start_engines.sh"
    echo "  Terminal 2: ./scripts/start_gateway.sh"
    echo ""
    echo "Or use: just dev-engines (in one terminal) then just dev-gateway (in another)"
    exit 1
fi

echo "Gateway detected on port 9000"
echo ""

# Default iterations (can override with argument)
ITERS=${1:-1000}

echo "Running binary RTT benchmark with $ITERS iterations..."
echo "This measures end-to-end latency through:"
echo "  Client → Gateway → Engine → Gateway → Client"
echo ""
echo "Note: Using binary protocol (JSON currently has response routing issues)"
echo ""

# Run the benchmark (using bench-bin which works)
./target/release/bench --mode bench-bin --bin-addr 127.0.0.1:9000 --iters "$ITERS"

echo ""
echo "✅ Benchmark complete!"
echo ""
echo "To run with more iterations for better accuracy:"
echo "  ./scripts/run_benchmark.sh 10000"
echo ""
echo "To run other benchmarks:"
echo "  just bench-rtt          - Binary RTT (10k iterations)"
echo "  just bench-rtt-fast     - Quick test (1k iterations)"
echo "  just bench-throughput   - Gateway throughput test"

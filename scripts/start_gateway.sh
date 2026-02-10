#!/bin/bash
# Start gateway server for testing

set -e

# Find project root (where Cargo.toml is)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$PROJECT_ROOT"

# Ensure binary exists
if [ ! -f "target/release/gateway_server" ]; then
    echo "Error: gateway_server binary not found"
    echo "Please build first: cargo build --release --bin gateway_server"
    exit 1
fi

# Ensure engines.toml exists
if [ ! -f "engines.toml" ]; then
    echo "Error: engines.toml not found in $PROJECT_ROOT"
    echo "Please create engines.toml with engine configuration"
    exit 1
fi

# Create persistence directories
mkdir -p journals snapshots

echo "Starting gateway server..."
echo "  Client binary: :9000"
echo "  Client JSON: :9001"
echo "  Admin HTTP: :8080"
echo ""
echo "Make sure engines are running first!"
echo "Use: ./scripts/start_engines.sh or just dev-engines"
echo ""
echo "Logs: $PROJECT_ROOT/gateway.log"
echo ""

RUST_LOG=info ./target/release/gateway_server \
  --client-binary-addr 0.0.0.0:9000 \
  --client-json-addr 0.0.0.0:9001 \
  --admin-addr 0.0.0.0:8080 \
  --journal-path journals/gateway.bin \
  --snapshot-dir snapshots \
  --engines-config engines.toml
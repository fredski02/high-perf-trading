#!/bin/bash
# Start gateway server for testing

set -e

echo "Starting gateway server..."
echo "  Client binary: :9000"
echo "  Client JSON: :9001"
echo "  Admin HTTP: :8080"
echo ""
echo "Make sure engines.toml is configured with:"
echo "  - BTC/USD on 127.0.0.1:9100"
echo "  - ETH/USD on 127.0.0.1:9101"
echo ""

RUST_LOG=info ./target/release/gateway_server \
  --client-binary-addr 127.0.0.1:9000 \
  --client-json-addr 127.0.0.1:9001 \
  --admin-addr 127.0.0.1:8080 \
  --engines-config engines.toml

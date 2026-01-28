set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

dev:
  RUST_LOG=info cargo run -p gateway -- \
    --binary-addr 127.0.0.1:9000 \
    --json-addr 127.0.0.1:9001 \
    --admin-addr 127.0.0.1:8080 \
    --journal-path ./journal.bin

test:
  cargo test --workspace

clippy:
  cargo clippy --workspace --all-targets -- -Dwarnings

fmt:
  cargo fmt --all

bacon:
  bacon

smoke:
  cargo run -p bench -- --mode smoke-all --bin-addr 127.0.0.1:9000 --json-addr 127.0.0.1:9001

health:
  curl -s http://127.0.0.1:8080/health && echo

metrics:
  curl -s http://127.0.0.1:8080/metrics

# Replay verification workflow:
# 1) Run smoke-all once to create resting orders
# 2) Restart gateway
# 3) Run smoke-replay which sends only a taker and expects a fill
replay-test:
  echo "==> 1) Run smoke-all to populate journal"
  cargo run -p bench -- --mode smoke-all --json-addr 127.0.0.1:9001
  echo "==> 2) Restart gateway now (Ctrl+C then just dev again)"
  echo "==> 3) After restart, run:"
  echo "    cargo run -p bench -- --mode smoke-replay --json-addr 127.0.0.1:9001"


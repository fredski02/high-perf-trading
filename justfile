set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

# Run the whole service (gateway + in-process admin HTTP)
dev:
  RUST_LOG=info cargo run -p gateway -- \
    --binary-addr 127.0.0.1:9000 \
    --json-addr 127.0.0.1:9001 \
    --admin-addr 127.0.0.1:8080

# Keep for muscle memory; same as dev now
dev-all:
  just dev

test:
  cargo test --workspace

clippy:
  cargo clippy --workspace --all-targets -- -Dwarnings

fmt:
  cargo fmt --all

bacon:
  bacon

# Real smoke: asserts matching + booktop + ack ordering (JSON)
smoke:
  cargo run -p bench -- --mode smoke-match --bin-addr 127.0.0.1:9000 --json-addr 127.0.0.1:9001

# Quick check endpoints (requires dev running)
health:
  curl -s http://127.0.0.1:8080/health && echo

metrics:
  curl -s http://127.0.0.1:8080/metrics


use std::sync::Arc;

use clap::Parser;

use admin_http::{metrics::Metrics, server};

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "127.0.0.1:8080")]
    addr: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let metrics = Arc::new(Metrics::default());
    server::run(args.addr, metrics).await
}

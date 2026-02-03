mod admin_server;
mod config;
mod gateway;

use clap::Parser;
use config::Args;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    gateway::run(args).await
}
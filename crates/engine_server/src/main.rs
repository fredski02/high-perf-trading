mod admin_server;
mod config;
mod gateway_connection;

use std::sync::Arc;

use clap::Parser;
use common::Metrics;
use config::Args;
use crossbeam_channel as cb;
use engine::{Engine, EngineConfig, Inbound, Outbound};
use persistence::JournalConfig;
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    info!(
        "Starting engine server for {} (symbol_id={})",
        args.symbol_name, args.symbol_id
    );
    info!("  Gateway listen addr: {}", args.listen_addr);
    info!("  Admin HTTP addr: {}", args.admin_addr);
    info!("  Journal path: {}", args.get_journal_path());
    info!("  Snapshot dir: {}", args.get_snapshot_dir());

    let metrics = Arc::new(Metrics::default());

    // Spawn admin server in-process
    let admin_addr = args.admin_addr.clone();
    let metrics_admin = metrics.clone();
    tokio::spawn(async move {
        if let Err(e) = admin_server::run(admin_addr, metrics_admin).await {
            tracing::warn!("admin server failed: {e:#}");
        }
    });

    // Create channels for engine communication
    let (in_tx, in_rx) = cb::bounded::<Inbound>(args.ingress_cap);
    let (out_tx, out_rx) = cb::unbounded::<Outbound>();
    let (query_tx, query_rx) = cb::unbounded::<engine::EngineQuery>(); // Query channel for reconciliation

    // Spawn engine thread
    let journal_config = JournalConfig {
        batch_size: args.journal_batch_size,
        ..Default::default()
    };

    let engine_config = EngineConfig {
        journal_path: args.get_journal_path(),
        snapshot_dir: args.get_snapshot_dir(),
        journal_config,
        snapshot_interval: args.snapshot_interval,
    };

    let metrics_engine = metrics.clone();
    std::thread::spawn(move || {
        let mut engine = Engine::new_with_config(in_rx, out_tx, query_rx, metrics_engine, engine_config);

        // Restore from persistence
        if let Err(e) = engine.restore_from_persistence() {
            tracing::error!("failed to restore from persistence: {e:#}");
        }

        engine.run();
    });

    // Listen for gateway connection (only ONE connection expected)
    let listener = TcpListener::bind(&args.listen_addr).await?;
    info!("Listening for gateway connection on {}", args.listen_addr);

    // Accept the gateway connection
    let (stream, addr) = listener.accept().await?;
    info!("Gateway connected from {}", addr);

    // Handle the gateway connection (blocks until disconnected)
    gateway_connection::handle_gateway_connection(
        stream,
        in_tx,
        out_rx,
        query_tx, // Pass query channel to handle QueryAllOrders
        metrics,
        args.max_frame,
        args.symbol_id,
    )
    .await?;

    info!("Gateway disconnected, shutting down");

    Ok(())
}
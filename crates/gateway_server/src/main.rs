mod account_manager;
mod client_handler;
mod config;
mod engine_router;

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::collections::HashMap;

use clap::Parser;
use codecs::{BinaryCodec, JsonCodec};
use common::Metrics;
use config::Args;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tracing::info;
use tracing_subscriber::EnvFilter;

use account_manager::AccountManager;
use engine_router::{EngineRouter, EnginesConfig};
use client_handler::{GatewayContext, handle_client_connection, handle_engine_responses};

static NEXT_CONN_ID: AtomicU64 = AtomicU64::new(1);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    
    tracing::info!("Starting gateway server...");
    tracing::info!("  Client binary addr: {}", args.client_binary_addr);
    tracing::info!("  Client JSON addr: {}", args.client_json_addr);
    tracing::info!("  Admin HTTP addr: {}", args.admin_addr);
    tracing::info!("  Account journal: {}", args.journal_path);
    tracing::info!("  Snapshot dir: {}", args.snapshot_dir);
    tracing::info!("  Engines config: {}", args.engines_config);

    let metrics = Arc::new(Metrics::default());

    // Initialize account manager
    let account_manager = Arc::new(AccountManager::new());
    
    // TODO: Load accounts from snapshot/journal
    // For now, create a test account
    account_manager.create_account(1, 1_000_000); // Account 1 with $1M buying power
    tracing::info!("Created test account: id=1, buying_power=1000000");

    // Load engine configuration and connect
    let engines_config = EnginesConfig::from_file(&args.engines_config)?;
    tracing::info!("Loaded {} engine(s) from {}", engines_config.engines.len(), args.engines_config);
    
    for engine in &engines_config.engines {
        tracing::info!("  - {} (symbol_id={}) at {}", engine.symbol_name, engine.symbol_id, engine.address);
    }

    let engine_router = Arc::new(EngineRouter::new(engines_config.engines).await?);
    tracing::info!("Connected to {} engine(s)", engine_router.num_connections().await);

    // Create gateway context
    let ctx = Arc::new(GatewayContext {
        account_manager,
        engine_router,
        metrics: metrics.clone(),
        pending_orders: Arc::new(RwLock::new(HashMap::new())),
    });

    // Client senders registry (conn_id -> mpsc sender)
    let client_senders: Arc<RwLock<HashMap<u64, tokio::sync::mpsc::Sender<bytes::Bytes>>>> = 
        Arc::new(RwLock::new(HashMap::new()));

    // Spawn admin server (TODO: implement)
    // For now, just log
    tracing::info!("Admin HTTP would listen on {}", args.admin_addr);

    // Spawn background task to handle engine responses
    let ctx_responses = ctx.clone();
    let codec_responses = Arc::new(BinaryCodec) as Arc<dyn codecs::Codec>;
    let client_senders_responses = client_senders.clone();
    tokio::spawn(async move {
        handle_engine_responses(ctx_responses, codec_responses, client_senders_responses).await;
    });

    // Spawn TCP listeners for clients
    let binary_listener = TcpListener::bind(&args.client_binary_addr).await?;
    let json_listener = TcpListener::bind(&args.client_json_addr).await?;

    info!("Binary protocol listening on {}", args.client_binary_addr);
    info!("JSON protocol listening on {}", args.client_json_addr);

    // Spawn binary protocol listener
    let binary_task = {
        let ctx = ctx.clone();
        let max_frame = args.max_frame;
        let client_senders = client_senders.clone();
        tokio::spawn(async move {
            run_listener(
                binary_listener,
                Arc::new(BinaryCodec),
                ctx,
                max_frame,
                client_senders,
            )
            .await
        })
    };

    // Spawn JSON protocol listener
    let json_task = {
        let ctx = ctx.clone();
        let max_frame = args.max_frame;
        let client_senders = client_senders.clone();
        tokio::spawn(async move {
            run_listener(
                json_listener,
                Arc::new(JsonCodec),
                ctx,
                max_frame,
                client_senders,
            )
            .await
        })
    };

    // Wait for both listeners
    let (binary_result, json_result) = tokio::join!(binary_task, json_task);
    binary_result??;
    json_result??;

    Ok(())
}

/// Run a TCP listener for client connections
async fn run_listener(
    listener: TcpListener,
    codec: Arc<dyn codecs::Codec>,
    ctx: Arc<GatewayContext>,
    max_frame: usize,
    client_senders: Arc<RwLock<HashMap<u64, tokio::sync::mpsc::Sender<bytes::Bytes>>>>,
) -> anyhow::Result<()> {
    loop {
        let (stream, _addr) = listener.accept().await?;
        let conn_id = NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed);

        let codec = codec.clone();
        let ctx = ctx.clone();
        let client_senders = client_senders.clone();

        tokio::spawn(async move {
            if let Err(e) = handle_client_connection(
                stream,
                conn_id,
                codec,
                ctx,
                max_frame,
                client_senders,
            )
            .await
            {
                tracing::debug!("client connection {conn_id} error: {e:#}");
            }
        });
    }
}
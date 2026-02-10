mod account_manager;
mod auth;
mod client_handler;
mod config;
mod engine_router;
mod persistence;
mod reconciliation;
mod session;

use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use clap::Parser;
use codecs::{BinaryCodec, JsonCodec};
use common::Metrics;
use config::Args;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tracing::info;
use tracing_subscriber::EnvFilter;

use account_manager::AccountManager;
use auth::AuthService;
use client_handler::{
    handle_client_connection, handle_engine_responses, ClientInfo, GatewayContext,
};
use engine_router::{EngineRouter, EnginesConfig};
use session::SessionManager;

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

    // Initialize authentication service
    let auth_service = Arc::new(AuthService::new());

    // Initialize session manager
    let session_manager = Arc::new(SessionManager::new());

    // Initialize account manager with persistence
    let account_manager = {
        // Open journal for persistence
        let journal_config = persistence::JournalConfig {
            batch_size: args.journal_batch_size,
            sync_interval: std::time::Duration::from_millis(100),
        };

        let mut journal =
            persistence::AccountJournal::open_with_config(&args.journal_path, journal_config)?;

        // Try to load latest snapshot
        let snapshot = persistence::AccountSnapshot::load_latest(&args.snapshot_dir)?;

        let account_manager = if let Some(snapshot) = snapshot {
            tracing::info!(
                "Loaded snapshot (seq={}, accounts={})",
                snapshot.sequence,
                snapshot.accounts.len()
            );

            // Read journal and replay updates after snapshot
            let updates = journal.read_all()?;
            tracing::info!("Loaded {} updates from journal", updates.len());

            let mgr = Arc::new(AccountManager::with_journal(journal));
            mgr.restore_from_snapshot(&snapshot);
            mgr.replay_journal(updates);

            mgr
        } else {
            tracing::info!("No snapshot found, starting with empty state");

            // Read journal (if any) and replay
            let updates = journal.read_all()?;
            if !updates.is_empty() {
                tracing::info!("Loaded {} updates from journal", updates.len());
            }

            let mgr = Arc::new(AccountManager::with_journal(journal));
            mgr.replay_journal(updates);

            mgr
        };

        account_manager
    };

    // Create test accounts with API keys (only if no accounts exist)
    let account_count = account_manager.create_snapshot(0).accounts.len();
    if account_count == 0 {
        tracing::info!("No accounts found, creating 10 test accounts");
        for account_id in 1..=10 {
            account_manager.create_account(account_id, 1_000_000); // Each account with $1M buying power

            // Register API key for each account (format: "test-key-{account_id}")
            let api_key = format!("test-key-{}", account_id);
            auth_service
                .register_api_key(api_key.clone(), account_id)
                .await;
        }
        tracing::info!("Created 10 test accounts (id=1-10), each with buying_power=1000000");
        tracing::info!("Registered 10 test API keys (test-key-1 through test-key-10)");
    } else {
        tracing::info!("Recovered {} accounts from persistence", account_count);

        // Still register test API keys for recovered accounts
        for account_id in 1..=10 {
            let api_key = format!("test-key-{}", account_id);
            auth_service
                .register_api_key(api_key.clone(), account_id)
                .await;
        }
    }

    // Load engine configuration and connect
    let engines_config = EnginesConfig::from_file(&args.engines_config)?;
    tracing::info!(
        "Loaded {} engine(s) from {}",
        engines_config.engines.len(),
        args.engines_config
    );

    for engine in &engines_config.engines {
        tracing::info!(
            "  - {} (symbol_id={}) at {}",
            engine.symbol_name,
            engine.symbol_id,
            engine.address
        );
    }

    let engine_router = Arc::new(EngineRouter::new(engines_config.engines).await?);
    tracing::info!(
        "Connected to {} engine(s)",
        engine_router.num_connections().await
    );

    // Reconcile reservation state with engines
    tracing::info!("Reconciling reservation state with engines...");
    let reconciliation =
        reconciliation::Reconciliation::new(account_manager.clone(), engine_router.clone());

    if let Err(e) = reconciliation.rebuild_reservations().await {
        tracing::error!("Reconciliation failed: {}", e);
        tracing::warn!("Continuing with empty reservation state");
    } else {
        tracing::info!("Reservation reconciliation complete");
    }

    // Create gateway context
    let ctx = Arc::new(GatewayContext {
        account_manager,
        engine_router,
        auth_service,
        session_manager,
        metrics: metrics.clone(),
        pending_orders: Arc::new(RwLock::new(HashMap::new())),
    });

    // Client senders registry (conn_id -> ClientInfo with codec)
    let client_senders: Arc<RwLock<HashMap<u64, ClientInfo>>> =
        Arc::new(RwLock::new(HashMap::new()));

    // Spawn admin server (TODO: implement)
    // For now, just log
    tracing::info!("Admin HTTP would listen on {}", args.admin_addr);

    // Spawn background task to handle engine responses
    let ctx_responses = ctx.clone();
    let client_senders_responses = client_senders.clone();
    tokio::spawn(async move {
        handle_engine_responses(ctx_responses, client_senders_responses).await;
    });

    // Spawn periodic snapshot task
    let ctx_snapshot = ctx.clone();
    let snapshot_dir = args.snapshot_dir.clone();
    let snapshot_interval = args.snapshot_interval;
    tokio::spawn(async move {
        let mut updates_since_snapshot = 0u64;
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));

        loop {
            interval.tick().await;

            // Check if we should create a snapshot
            updates_since_snapshot += 1;

            if updates_since_snapshot >= snapshot_interval / 60 {
                let sequence = updates_since_snapshot;
                let snapshot = ctx_snapshot.account_manager.create_snapshot(sequence);

                match snapshot.save(&snapshot_dir) {
                    Ok(path) => {
                        tracing::info!("Snapshot saved: {:?}", path);

                        // Cleanup old snapshots (keep last 3)
                        if let Err(e) = persistence::AccountSnapshot::cleanup_old(&snapshot_dir, 3)
                        {
                            tracing::warn!("Failed to cleanup old snapshots: {}", e);
                        }

                        updates_since_snapshot = 0;
                    }
                    Err(e) => {
                        tracing::error!("Failed to save snapshot: {}", e);
                    }
                }

                // Flush journal
                if let Err(e) = ctx_snapshot.account_manager.flush_journal() {
                    tracing::error!("Failed to flush journal: {}", e);
                }
            }
        }
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
    client_senders: Arc<RwLock<HashMap<u64, ClientInfo>>>,
) -> anyhow::Result<()> {
    loop {
        let (stream, _addr) = listener.accept().await?;
        let conn_id = NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed);

        let codec = codec.clone();
        let ctx = ctx.clone();
        let client_senders = client_senders.clone();

        tokio::spawn(async move {
            if let Err(e) =
                handle_client_connection(stream, conn_id, codec, ctx, max_frame, client_senders)
                    .await
            {
                tracing::debug!("client connection {conn_id} error: {e:#}");
            }
        });
    }
}

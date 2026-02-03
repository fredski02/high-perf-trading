use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use codecs::{BinaryCodec, JsonCodec};
use common::Metrics;
use crossbeam_channel as cb;
use engine::{Engine, EngineConfig, Inbound, Outbound};
use persistence::JournalConfig;
use tokio::net::TcpListener;
use tracing::info;

use crate::{connection, router::Router};

static NEXT_CONN_ID: AtomicU64 = AtomicU64::new(1);

/// Main gateway entry point
pub async fn run(args: crate::config::Args) -> anyhow::Result<()> {
    let metrics = Arc::new(Metrics::default());

    // Spawn admin server in-process
    let admin_addr = args.admin_addr.clone();
    let metrics_admin = metrics.clone();
    tokio::spawn(async move {
        if let Err(e) = crate::admin_server::run(admin_addr, metrics_admin).await {
            tracing::warn!("admin server failed: {e:#}");
        }
    });

    let (in_tx, in_rx) = cb::bounded::<Inbound>(args.ingress_cap);
    let (out_tx, out_rx) = cb::unbounded::<Outbound>();

    // Create router
    let router = Arc::new(Router::new(out_rx, metrics.clone()));

    // Spawn router task
    let router_task = router.clone();
    tokio::spawn(async move {
        router_task.run().await;
    });

    // Spawn engine thread
    let journal_config = JournalConfig {
        batch_size: args.journal_batch_size,
        ..Default::default()
    };

    let engine_config = EngineConfig {
        journal_path: args.journal_path.clone(),
        snapshot_dir: args.snapshot_dir.clone(),
        journal_config,
        snapshot_interval: args.snapshot_interval,
    };

    let metrics_engine = metrics.clone();
    std::thread::spawn(move || {
        let mut engine = Engine::new_with_config(in_rx, out_tx, metrics_engine, engine_config);

        // Restore from persistence
        if let Err(e) = engine.restore_from_persistence() {
            tracing::error!("failed to restore from persistence: {e:#}");
        }

        engine.run();
    });

    // Spawn TCP listeners
    let binary_listener = TcpListener::bind(&args.binary_addr).await?;
    let json_listener = TcpListener::bind(&args.json_addr).await?;

    info!("binary protocol listening on {}", args.binary_addr);
    info!("json protocol listening on {}", args.json_addr);
    info!("admin http listening on {}", args.admin_addr);

    // Spawn binary protocol listener
    let binary_task = {
        let router = router.clone();
        let in_tx = in_tx.clone();
        let metrics = metrics.clone();
        let max_frame = args.max_frame;
        tokio::spawn(async move {
            run_listener(
                binary_listener,
                Arc::new(BinaryCodec),
                router,
                in_tx,
                metrics,
                max_frame,
            )
            .await
        })
    };

    // Spawn JSON protocol listener
    let json_task = {
        let router = router.clone();
        let in_tx = in_tx.clone();
        let metrics = metrics.clone();
        let max_frame = args.max_frame;
        tokio::spawn(async move {
            run_listener(
                json_listener,
                Arc::new(JsonCodec),
                router,
                in_tx,
                metrics,
                max_frame,
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

/// Run a TCP listener for a specific protocol
async fn run_listener(
    listener: TcpListener,
    codec: Arc<dyn codecs::Codec>,
    router: Arc<Router>,
    engine_in: cb::Sender<Inbound>,
    metrics: Arc<Metrics>,
    max_frame: usize,
) -> anyhow::Result<()> {
    loop {
        let (stream, _addr) = listener.accept().await?;
        let conn_id = NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed);

        let codec = codec.clone();
        let router = router.clone();
        let engine_in = engine_in.clone();
        let metrics = metrics.clone();

        tokio::spawn(async move {
            if let Err(e) = connection::handle_connection(
                stream, conn_id, codec, router, engine_in, metrics, max_frame,
            )
            .await
            {
                tracing::debug!("connection {conn_id} error: {e:#}");
            }
        });
    }
}
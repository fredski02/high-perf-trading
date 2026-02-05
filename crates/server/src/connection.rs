use std::sync::Arc;

use anyhow::Context;
use bytes::Bytes;
use codecs::Codec;
use common::{Command, Metrics};
use crossbeam_channel as cb;
use engine::Inbound;
use futures::{SinkExt, StreamExt};
use tokio::{net::TcpStream, sync::mpsc};
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tracing::warn;

use crate::router::Router;

const WRITE_BUF_SIZE: usize = 1024;

/// Handle a single TCP connection
pub async fn handle_connection(
    stream: TcpStream,
    conn_id: u64,
    codec: Arc<dyn Codec>,
    router: Arc<Router>,
    engine_in: cb::Sender<Inbound>,
    metrics: Arc<Metrics>,
    max_frame: usize,
) -> anyhow::Result<()> {
    metrics.inc_connections();

    // Create length-delimited framed stream
    let framed = LengthDelimitedCodec::builder()
        .little_endian()
        .max_frame_length(max_frame)
        .new_framed(stream);

    let (mut write_half, mut read_half) = framed.split();

    // Channel for outbound frames
    let (out_tx, mut out_rx) = mpsc::channel::<Bytes>(WRITE_BUF_SIZE);

    // Register with router
    router.register(conn_id, out_tx, codec.clone());

    // Spawn write loop
    let write_task = tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            if write_half.send(frame).await.is_err() {
                break;
            }
        }
    });

    // Run read loop (blocks until connection closes)
    let read_result = read_loop(
        &mut read_half,
        conn_id,
        codec,
        router.clone(),
        engine_in,
        metrics.clone(),
    )
    .await;

    // Cleanup
    router.unregister(conn_id);
    write_task.abort();

    read_result
}

/// Read loop: receives frames from client and sends to engine
async fn read_loop(
    read_half: &mut futures::stream::SplitStream<Framed<TcpStream, LengthDelimitedCodec>>,
    conn_id: u64,
    codec: Arc<dyn Codec>,
    router: Arc<Router>,
    engine_in: cb::Sender<Inbound>,
    metrics: Arc<Metrics>,
) -> anyhow::Result<()> {
    while let Some(frame_result) = read_half.next().await {
        let frame = frame_result.context("read frame failed")?;
        
        metrics.inc_frames_in();

        let cmd = match codec.decode_command(&frame.freeze()) {
            Ok(c) => c,
            Err(e) => {
                warn!("decode error: {e:#}");
                continue;
            }
        };

        // Send to engine (engine handles journaling)
        if engine_in.try_send(Inbound { conn_id, cmd }).is_ok() {
            metrics.queue_inc();
        } else {
            let client_seq = match &cmd {
                Command::NewOrder(x) => x.client_seq,
                Command::Cancel(x) => x.client_seq,
                Command::Replace(x) => x.client_seq,
                Command::SetRiskLimits(x) => x.client_seq,
                Command::QueryAccount(x) => x.client_seq,
            };
            router.send_reject_overloaded(conn_id, client_seq);
        }
    }
    
    Ok(())
}
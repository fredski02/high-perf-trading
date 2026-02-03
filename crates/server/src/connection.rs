use std::sync::Arc;

use anyhow::Context;
use bytes::{Bytes, BytesMut};
use codecs::Codec;
use common::{Command, Metrics};
use crossbeam_channel as cb;
use engine::Inbound;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{
        tcp::{OwnedReadHalf, OwnedWriteHalf},
        TcpStream,
    },
    sync::mpsc,
};
use tracing::warn;

use crate::{protocol::try_read_frame, router::Router};

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

    let (read_half, write_half) = stream.into_split();

    // Channel for outbound frames
    let (out_tx, out_rx) = mpsc::channel::<Bytes>(WRITE_BUF_SIZE);

    // Register with router
    router.register(conn_id, out_tx, codec.clone());

    // Spawn write loop
    let write_task = tokio::spawn(write_loop(write_half, out_rx));

    // Run read loop (blocks until connection closes)
    let read_result = read_loop(
        read_half,
        conn_id,
        codec,
        router.clone(),
        engine_in,
        metrics.clone(),
        max_frame,
    )
    .await;

    // Cleanup
    router.unregister(conn_id);
    write_task.abort();

    read_result
}

/// Write loop: sends frames to client
async fn write_loop(
    mut writer: OwnedWriteHalf,
    mut out_rx: mpsc::Receiver<Bytes>,
) -> anyhow::Result<()> {
    while let Some(frame) = out_rx.recv().await {
        writer
            .write_all(&frame)
            .await
            .context("write failed")?;
    }
    Ok(())
}

/// Read loop: receives frames from client and sends to engine
async fn read_loop(
    mut reader: OwnedReadHalf,
    conn_id: u64,
    codec: Arc<dyn Codec>,
    router: Arc<Router>,
    engine_in: cb::Sender<Inbound>,
    metrics: Arc<Metrics>,
    max_frame: usize,
) -> anyhow::Result<()> {
    let mut buf = BytesMut::with_capacity(max_frame);

    loop {
        let n = reader
            .read_buf(&mut buf)
            .await
            .context("read failed")?;

        if n == 0 {
            return Ok(()); // EOF
        }

        // Try to parse complete frames
        while let Some(frame) = try_read_frame(&mut buf, max_frame)? {
            metrics.inc_frames_in();

            let cmd = match codec.decode_command(&frame) {
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
    }
}

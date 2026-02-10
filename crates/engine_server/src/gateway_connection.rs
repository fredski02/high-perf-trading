//! Gateway connection handling for engine server
//!
//! The engine server now ONLY accepts connections from the gateway server.
//! It receives GatewayToEngine messages and sends back EngineToGateway events.

use anyhow::{Context, Result};
use bytes::Bytes;
use common::{EngineToGateway, GatewayToEngine, Metrics};
use crossbeam_channel as cb;
use engine::{Inbound, Outbound};
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tracing::warn;

/// Handle a connection from the gateway server
pub async fn handle_gateway_connection(
    stream: TcpStream,
    engine_in: cb::Sender<Inbound>,
    engine_out: cb::Receiver<Outbound>,
    query_tx: cb::Sender<engine::EngineQuery>, // Query channel for reconciliation
    metrics: Arc<Metrics>,
    max_frame: usize,
    symbol_id: u32,
) -> Result<()> {
    tracing::info!("Gateway connected for symbol_id={}", symbol_id);

    // Enable TCP_NODELAY to disable Nagle's algorithm (critical for low latency)
    stream
        .set_nodelay(true)
        .context("Failed to set TCP_NODELAY")?;

    // Set larger socket buffers for better throughput
    // Note: Would need socket2 crate for buffer tuning, skipping for now
    // let _ = stream.set_recv_buffer_size(256 * 1024);
    // let _ = stream.set_send_buffer_size(256 * 1024);

    // Create length-delimited framed stream
    let framed = LengthDelimitedCodec::builder()
        .little_endian()
        .max_frame_length(max_frame)
        .new_framed(stream);

    let (mut write_half, mut read_half) = framed.split();

    // Channel for query responses (read loop will send responses here)
    let (query_response_tx, mut query_response_rx) =
        tokio::sync::mpsc::unbounded_channel::<EngineToGateway>();

    // Spawn write loop to send events back to gateway
    let metrics_write = metrics.clone();
    let write_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_micros(100));
        loop {
            tokio::select! {
                // Poll engine events periodically
                _ = interval.tick() => {
                    while let Ok(outbound) = engine_out.try_recv() {
                        // Wrap in EngineToGateway protocol
                        let gateway_event = EngineToGateway::client_event(
                            outbound.conn_id,
                            outbound.ev,
                            None, // TODO: Track risk tokens
                        );

                        // Serialize with postcard
                        let serialized = match postcard::to_allocvec(&gateway_event) {
                            Ok(bytes) => bytes,
                            Err(e) => {
                                warn!("Failed to serialize EngineToGateway: {}", e);
                                continue;
                            }
                        };

                        // Use feed() instead of send() to avoid auto-flush
                        if write_half.feed(Bytes::from(serialized)).await.is_err() {
                            return;
                        }

                        metrics_write.inc_frames_out();
                    }

                    // Flush after processing all available events
                    if write_half.flush().await.is_err() {
                        return;
                    }
                }

                // Handle query responses
                Some(response) = query_response_rx.recv() => {
                    let serialized = match postcard::to_allocvec(&response) {
                        Ok(bytes) => bytes,
                        Err(e) => {
                            warn!("Failed to serialize query response: {}", e);
                            continue;
                        }
                    };

                    if write_half.feed(Bytes::from(serialized)).await.is_err() {
                        break;
                    }

                    if write_half.flush().await.is_err() {
                        break;
                    }

                    metrics_write.inc_frames_out();
                }
            }
        }
    });

    // Read loop: receive commands from gateway
    let read_result = gateway_read_loop(
        &mut read_half,
        query_response_tx,
        engine_in,
        query_tx,
        metrics,
    )
    .await;

    // Cleanup
    write_task.abort();

    read_result
}

/// Read loop: receives GatewayToEngine messages and forwards to engine
async fn gateway_read_loop(
    read_half: &mut futures::stream::SplitStream<Framed<TcpStream, LengthDelimitedCodec>>,
    query_response_tx: tokio::sync::mpsc::UnboundedSender<EngineToGateway>,
    engine_in: cb::Sender<Inbound>,
    query_tx: cb::Sender<engine::EngineQuery>,
    metrics: Arc<Metrics>,
) -> Result<()> {
    while let Some(frame_result) = read_half.next().await {
        let frame = frame_result.context("read frame failed")?;

        metrics.inc_frames_in();

        // Deserialize GatewayToEngine message
        let gateway_msg = match postcard::from_bytes::<GatewayToEngine>(&frame) {
            Ok(msg) => msg,
            Err(e) => {
                warn!("Failed to deserialize GatewayToEngine: {}", e);
                continue;
            }
        };

        match gateway_msg {
            GatewayToEngine::Execute(exec) => {
                // Extract the command and conn_id
                let inbound = Inbound {
                    conn_id: exec.conn_id,
                    cmd: exec.command,
                };

                // Send to engine
                if engine_in.try_send(inbound).is_ok() {
                    metrics.queue_inc();
                } else {
                    warn!("Engine queue full, dropping command");
                    // TODO: Send reject back to gateway
                }
            }
            GatewayToEngine::Ping => {
                // Health check - respond with Pong
                // TODO: Implement Pong response
                tracing::debug!("Received ping from gateway");
            }
            GatewayToEngine::QueryAllOrders => {
                // Query all orders request for reconciliation
                tracing::debug!("Received QueryAllOrders from gateway");

                // Create oneshot channel for response
                let (response_tx, response_rx) = tokio::sync::oneshot::channel();

                // Send query to engine thread
                let query = engine::EngineQuery::GetAllOrders { response_tx };
                if let Err(e) = query_tx.send(query) {
                    warn!("Failed to send query to engine: {}", e);
                    continue;
                }

                // Wait for response from engine thread
                match response_rx.await {
                    Ok(orders) => {
                        tracing::info!(
                            "Received {} orders from engine, sending to gateway",
                            orders.len()
                        );

                        // Send AllOrders response back to gateway via channel
                        let response = EngineToGateway::AllOrders(orders);
                        if query_response_tx.send(response).is_err() {
                            warn!("Failed to send AllOrders response to write loop");
                            break;
                        }
                    }
                    Err(e) => {
                        warn!("Engine query failed: {}", e);
                    }
                }
            }
        }
    }

    Ok(())
}

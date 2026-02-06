//! Gateway connection handling for engine server
//!
//! The engine server now ONLY accepts connections from the gateway server.
//! It receives GatewayToEngine messages and sends back EngineToGateway events.

use anyhow::{Context, Result};
use bytes::Bytes;
use common::{GatewayToEngine, EngineToGateway, Metrics};
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
    metrics: Arc<Metrics>,
    max_frame: usize,
    symbol_id: u32,
) -> Result<()> {
    tracing::info!("Gateway connected for symbol_id={}", symbol_id);

    // Enable TCP_NODELAY to disable Nagle's algorithm (critical for low latency)
    stream.set_nodelay(true)
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

    // Spawn write loop to send events back to gateway
    let metrics_write = metrics.clone();
    let write_task = tokio::spawn(async move {
        while let Ok(outbound) = engine_out.recv() {

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
                break;
            }

            metrics_write.inc_frames_out();
            
            // Try to batch more events if available (non-blocking)
            while let Ok(outbound) = engine_out.try_recv() {
                let gateway_event = EngineToGateway::client_event(
                    outbound.conn_id,
                    outbound.ev,
                    None,
                );
                
                let serialized = match postcard::to_allocvec(&gateway_event) {
                    Ok(bytes) => bytes,
                    Err(_) => continue,
                };
                
                if write_half.feed(Bytes::from(serialized)).await.is_err() {
                    return;
                }
                
                metrics_write.inc_frames_out();
            }
            
            // Now flush once for the batch
            if write_half.flush().await.is_err() {
                break;
            }
        }
    });

    // Read loop: receive commands from gateway
    let read_result = gateway_read_loop(&mut read_half, engine_in, metrics).await;

    // Cleanup
    write_task.abort();

    read_result
}

/// Read loop: receives GatewayToEngine messages and forwards to engine
async fn gateway_read_loop(
    read_half: &mut futures::stream::SplitStream<Framed<TcpStream, LengthDelimitedCodec>>,
    engine_in: cb::Sender<Inbound>,
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
        }
    }
    
    Ok(())
}
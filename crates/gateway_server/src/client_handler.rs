//! Handles client connections (similar to current engine_server connection handling)
//!
//! Flow for each client connection:
//! 1. Receive command from client
//! 2. Check risk with AccountManager
//! 3. Route to appropriate engine via EngineRouter
//! 4. Wait for response from engine (via EngineRouter event channel)
//! 5. Update AccountManager with fill
//! 6. Send response to client

use anyhow::{Context, Result};
use bytes::Bytes;
use codecs::Codec;
use common::{
    command_symbol_id, Command, EngineToGateway, Event, GatewayToEngine, Metrics, OrderId, Side,
};
use futures::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::{
    net::TcpStream,
    sync::{mpsc, RwLock},
};
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tracing::{info, warn};

use crate::account_manager::{AccountManager, ReservationToken};
use crate::engine_router::EngineRouter;

const WRITE_BUF_SIZE: usize = 1024;

/// Order metadata for tracking fills
#[derive(Clone, Debug)]
pub struct OrderMetadata {
    pub reservation_token: ReservationToken,
    pub side: Side,
}

/// Client connection info for routing responses
#[derive(Clone)]
pub struct ClientInfo {
    pub sender: mpsc::Sender<Bytes>,
    pub codec: Arc<dyn Codec>,
}

/// Context shared across all client connections
pub struct GatewayContext {
    pub account_manager: Arc<AccountManager>,
    pub engine_router: Arc<EngineRouter>,
    pub auth_service: Arc<crate::auth::AuthService>,
    pub session_manager: Arc<crate::session::SessionManager>,
    pub metrics: Arc<Metrics>,
    /// Global order tracking: order_id -> metadata
    pub pending_orders: Arc<RwLock<HashMap<OrderId, OrderMetadata>>>,
}

/// Handle a single client connection
pub async fn handle_client_connection(
    stream: TcpStream,
    conn_id: u64,
    codec: Arc<dyn Codec>,
    ctx: Arc<GatewayContext>,
    max_frame: usize,
    client_senders: Arc<tokio::sync::RwLock<HashMap<u64, ClientInfo>>>,
) -> Result<()> {
    ctx.metrics.inc_connections();

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

    // Channel for outbound frames to this client
    let (out_tx, mut out_rx) = mpsc::channel::<Bytes>(WRITE_BUF_SIZE);

    // Register this connection in the client_senders registry
    {
        let mut senders = client_senders.write().await;
        let client_info = ClientInfo {
            sender: out_tx.clone(),
            codec: codec.clone(),
        };
        senders.insert(conn_id, client_info);
    }

    // Spawn write loop
    let write_task = tokio::spawn(async move {
        while let Some(frame) = out_rx.recv().await {
            // Use feed() instead of send() to avoid auto-flush
            if write_half.feed(frame).await.is_err() {
                break;
            }

            // Try to batch more frames if available (non-blocking)
            while let Ok(frame) = out_rx.try_recv() {
                if write_half.feed(frame).await.is_err() {
                    return;
                }
            }

            // Now flush once for the batch
            if write_half.flush().await.is_err() {
                break;
            }
        }
    });

    // Run read loop (blocks until connection closes)
    let read_result = client_read_loop(&mut read_half, conn_id, codec, out_tx, ctx.clone()).await;

    // Cleanup: remove from registry, unregister session, and abort write task
    {
        let mut senders = client_senders.write().await;
        senders.remove(&conn_id);
    }
    
    // Unregister session if client was authenticated
    if let Some(account_id) = ctx.session_manager.unregister(conn_id).await {
        info!("Client disconnected: conn_id={}, account_id={}", conn_id, account_id);
    } else {
        info!("Client disconnected: conn_id={} (was not authenticated)", conn_id);
    }
    
    write_task.abort();

    read_result
}

/// Read loop: receives commands from client, does risk checks, routes to engines
async fn client_read_loop(
    read_half: &mut futures::stream::SplitStream<Framed<TcpStream, LengthDelimitedCodec>>,
    conn_id: u64,
    codec: Arc<dyn Codec>,
    out_tx: mpsc::Sender<Bytes>,
    ctx: Arc<GatewayContext>,
) -> Result<()> {
    // Track pending orders: order_id -> (reservation_token, is_buy)
    let mut pending_orders: HashMap<u64, (ReservationToken, bool)> = HashMap::new();

    while let Some(frame_result) = read_half.next().await {
        let frame = frame_result.context("read frame failed")?;

        ctx.metrics.inc_frames_in();

        // Decode command
        let cmd = match codec.decode_command(&frame.freeze()) {
            Ok(c) => c,
            Err(e) => {
                warn!("decode error: {e:#}");
                continue;
            }
        };

        // Handle QueryAccount directly (no need to route to engine)
        if let Command::QueryAccount(query) = &cmd {
            if let Some(account_state) = ctx.account_manager.get_account(query.account_id) {
                // Find position and risk limits for this symbol
                let position = account_state
                    .positions
                    .get(&query.symbol_id)
                    .copied()
                    .unwrap_or_default();
                let risk_limits = account_state
                    .risk_limits
                    .get(&query.symbol_id)
                    .copied()
                    .unwrap_or_default();

                let account_state_event = Event::AccountState(common::AccountState {
                    server_seq: ctx.account_manager.next_seq(),
                    client_seq: query.client_seq,
                    account_id: query.account_id,
                    symbol_id: query.symbol_id,
                    position,
                    risk_limits,
                });

                let mut response_payload = bytes::BytesMut::with_capacity(256);
                if codec
                    .encode_event(&account_state_event, &mut response_payload)
                    .is_ok()
                {
                    let _ = out_tx.send(response_payload.freeze()).await;
                    ctx.metrics.inc_frames_out();
                }
            } else {
                // Account not found
                let reject = Event::Reject(common::Reject {
                    server_seq: ctx.account_manager.next_seq(),
                    client_seq: query.client_seq,
                    order_id: None,
                    reason: common::RejectReason::NotFound,
                });

                let mut reject_payload = bytes::BytesMut::with_capacity(256);
                if codec.encode_event(&reject, &mut reject_payload).is_ok() {
                    let _ = out_tx.send(reject_payload.freeze()).await;
                    ctx.metrics.inc_frames_out();
                }
            }

            continue;
        }

        // Handle Authenticate directly (no need to route to engine)
        if let Command::Authenticate(auth) = &cmd {
            // Verify API key and get account ID
            let auth_result = ctx.auth_service.authenticate(&auth.api_key).await;

            let event = match auth_result {
                Ok(account_id) => {
                    // Register the session
                    ctx.session_manager.register(conn_id, account_id).await;
                    info!("Client authenticated: conn_id={}, account_id={}", conn_id, account_id);
                    Event::AuthSuccess(common::AuthSuccess { account_id })
                }
                Err(err) => {
                    warn!("Authentication failed: conn_id={}, error={}", conn_id, err);
                    Event::AuthFailure(common::AuthFailure {
                        reason: format!("Authentication failed: {}", err),
                    })
                }
            };

            let mut response_payload = bytes::BytesMut::with_capacity(256);
            if codec.encode_event(&event, &mut response_payload).is_ok() {
                let _ = out_tx.send(response_payload.freeze()).await;
                ctx.metrics.inc_frames_out();
            }

            continue;
        }

        // All other commands require authentication
        if !ctx.session_manager.is_authenticated(conn_id).await {
            warn!("Unauthenticated command rejected: conn_id={}", conn_id);
            
            let reject = Event::Reject(common::Reject {
                server_seq: ctx.account_manager.next_seq(),
                client_seq: 0, // Don't know client_seq for unauthenticated commands
                order_id: None,
                reason: common::RejectReason::Invalid,
            });

            let mut reject_payload = bytes::BytesMut::with_capacity(256);
            if codec.encode_event(&reject, &mut reject_payload).is_ok() {
                let _ = out_tx.send(reject_payload.freeze()).await;
                ctx.metrics.inc_frames_out();
            }

            continue;
        }

        // Extract info we'll need (safe to unwrap since Authenticate was handled above)
        let symbol_id = command_symbol_id(&cmd).expect("command should have symbol_id");
        let is_buy = if let Command::NewOrder(order) = &cmd {
            order.side == common::Side::Buy
        } else {
            false
        };

        // Do risk check and get reservation token
        let reservation_token = match ctx.account_manager.check_and_reserve(&cmd) {
            Ok(token) => token,
            Err(err) => {
                // Risk check failed - send reject to client
                warn!("Risk check failed for conn_id={}: {:?}", conn_id, err);

                // Get client_seq and order_id for the reject message
                let (client_seq, order_id) = match &cmd {
                    Command::NewOrder(order) => (order.client_seq, Some(order.order_id)),
                    Command::Cancel(cancel) => (cancel.client_seq, Some(cancel.order_id)),
                    Command::Replace(replace) => (replace.client_seq, Some(replace.order_id)),
                    _ => (0, None),
                };

                // Create reject event
                let reject = Event::Reject(common::Reject {
                    server_seq: ctx.account_manager.next_seq(),
                    client_seq,
                    order_id,
                    reason: common::RejectReason::Risk,
                });

                // Send reject to client
                let mut reject_payload = bytes::BytesMut::with_capacity(256);
                if codec.encode_event(&reject, &mut reject_payload).is_ok() {
                    let _ = out_tx.send(reject_payload.freeze()).await;
                    ctx.metrics.inc_frames_out();
                }

                continue;
            }
        };

        // Track pending order (both locally and globally)
        if let Command::NewOrder(order) = &cmd {
            pending_orders.insert(order.order_id, (reservation_token.clone(), is_buy));

            // Also store in global tracking for handle_engine_responses
            let side = if is_buy { Side::Buy } else { Side::Sell };
            let metadata = OrderMetadata {
                reservation_token: reservation_token.clone(),
                side,
            };
            ctx.pending_orders
                .write()
                .await
                .insert(order.order_id, metadata);
        }

        // Create gateway message with risk approval
        let gateway_seq = ctx.account_manager.next_seq();
        let risk_token = common::RiskToken {
            account_id: reservation_token.account_id,
            reserved_amount: reservation_token.amount,
            gateway_seq,
        };

        let gateway_msg = GatewayToEngine::execute(cmd.clone(), conn_id, risk_token);

        // Route to engine
        if let Err(e) = ctx
            .engine_router
            .route_to_engine(&gateway_msg, symbol_id)
            .await
        {
            warn!(
                "Failed to route to engine for symbol_id={}: {}",
                symbol_id, e
            );

            // Release reservation since we couldn't route
            ctx.account_manager.release_reservation(&reservation_token);

            if let Command::NewOrder(order) = &cmd {
                pending_orders.remove(&order.order_id);
            }

            // Get client_seq and order_id for the reject message
            let (client_seq, order_id) = match &cmd {
                Command::NewOrder(order) => (order.client_seq, Some(order.order_id)),
                Command::Cancel(cancel) => (cancel.client_seq, Some(cancel.order_id)),
                Command::Replace(replace) => (replace.client_seq, Some(replace.order_id)),
                _ => (0, None),
            };

            // Send reject to client (engine not available)
            let reject = Event::Reject(common::Reject {
                server_seq: ctx.account_manager.next_seq(),
                client_seq,
                order_id,
                reason: common::RejectReason::Overloaded,
            });

            let mut reject_payload = bytes::BytesMut::with_capacity(256);
            if codec.encode_event(&reject, &mut reject_payload).is_ok() {
                let _ = out_tx.send(reject_payload.freeze()).await;
                ctx.metrics.inc_frames_out();
            }

            continue;
        }

        // Wait for response from engine (in background)
        // Note: In a real implementation, we'd spawn a task to handle responses
        // and match them back to this connection. For now, this is a simplified version.

        // TODO: Implement proper response handling
        // The gateway needs a way to route engine responses back to the correct client connection
        // This requires maintaining a mapping of order_id -> conn_id
    }

    Ok(())
}

/// Background task to handle responses from engines and route them to clients
///
/// This task:
/// 1. Receives events from EngineRouter
/// 2. Looks up which client connection to send to (by conn_id)
/// 3. Updates AccountManager with fills
/// 4. Sends event to client
pub async fn handle_engine_responses(
    ctx: Arc<GatewayContext>,
    client_senders: Arc<tokio::sync::RwLock<HashMap<u64, ClientInfo>>>,
) {
    loop {
        // Receive next event from any engine
        let engine_event = match ctx.engine_router.recv_event().await {
            Some(event) => event,
            None => {
                warn!("Engine event channel closed");
                break;
            }
        };

        match engine_event {
            EngineToGateway::ClientEvent {
                conn_id,
                event,
                risk_token: _,
            } => {
                // Update account manager if this is a fill
                if let Event::Fill(ref fill) = &event {
                    // Handle BOTH maker and taker fills for account updates
                    // First, collect the metadata we need while holding read lock
                    let (maker_metadata, taker_metadata) = {
                        let pending_orders = ctx.pending_orders.read().await;
                        let maker = pending_orders.get(&fill.maker_order_id).cloned();
                        let taker = pending_orders.get(&fill.taker_order_id).cloned();
                        (maker, taker)
                    }; // Read lock released

                    // Update maker order account state
                    if let Some(metadata) = maker_metadata {
                        let is_buy = metadata.side == Side::Buy;
                        ctx.account_manager
                            .apply_fill(&event, &metadata.reservation_token, is_buy);

                        // Remove from pending (assuming full fill for now)
                        ctx.pending_orders
                            .write()
                            .await
                            .remove(&fill.maker_order_id);
                    }

                    // Update taker order account state
                    if let Some(metadata) = taker_metadata {
                        let is_buy = metadata.side == Side::Buy;
                        ctx.account_manager
                            .apply_fill(&event, &metadata.reservation_token, is_buy);

                        // Remove from pending (assuming full fill for now)
                        ctx.pending_orders
                            .write()
                            .await
                            .remove(&fill.taker_order_id);
                    }
                }

                // Send event to client using their codec
                let clients = client_senders.read().await;
                if let Some(client_info) = clients.get(&conn_id) {
                    let mut payload = bytes::BytesMut::with_capacity(256);
                    if client_info.codec.encode_event(&event, &mut payload).is_ok() {
                        let _ = client_info.sender.send(payload.freeze()).await;
                        ctx.metrics.inc_frames_out();
                    }
                } else {
                    warn!("No client found for conn_id={}", conn_id);
                }
            }
            EngineToGateway::MarketData { symbol_id, event } => {
                // Broadcast market data to all subscribed clients
                // TODO: Implement market data subscription tracking
                tracing::debug!("Market data for symbol_id={}: {:?}", symbol_id, event);
            }
            EngineToGateway::Pong {
                symbol_id,
                orders_in_book,
            } => {
                // Engine health check response
                tracing::debug!(
                    "Engine health: symbol_id={}, orders_in_book={}",
                    symbol_id,
                    orders_in_book
                );
            }
        }
    }
}
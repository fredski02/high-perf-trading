//! Routes orders to the correct engine server based on symbol_id
//!
//! Maintains persistent TCP connections to all engine servers and handles:
//! - Routing commands to engines by symbol_id
//! - Receiving events (fills, acks, rejects) from engines
//! - Connection health monitoring

use anyhow::{Context, Result};
use bytes::Bytes;
use futures::{stream::SplitSink, stream::SplitStream, SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex};
use tokio_util::codec::{Framed, LengthDelimitedCodec};

use common::{EngineToGateway, GatewayToEngine, SymbolId};

/// Configuration for a single engine (loaded from TOML)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineConfig {
    pub symbol_id: SymbolId,
    pub symbol_name: String,
    pub address: String, // "host:port" format
}

/// Configuration file format (engines.toml)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnginesConfig {
    pub engines: Vec<EngineConfig>,
}

impl EnginesConfig {
    /// Load engine configuration from TOML file
    pub fn from_file(path: &str) -> Result<Self> {
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read engines config: {}", path))?;
        let config: EnginesConfig = toml::from_str(&contents)
            .with_context(|| format!("Failed to parse engines config: {}", path))?;
        Ok(config)
    }
}

/// Write half of an engine connection
type EngineSink = SplitSink<Framed<TcpStream, LengthDelimitedCodec>, Bytes>;

/// Routes commands to engine servers
pub struct EngineRouter {
    /// Write halves of connections to engines (for sending commands)
    senders: Arc<Mutex<HashMap<SymbolId, EngineSink>>>,

    /// Channel to receive events from all engines
    event_rx: Arc<Mutex<mpsc::UnboundedReceiver<EngineToGateway>>>,

    /// Sender side of event channel (cloned for each engine listener)
    event_tx: mpsc::UnboundedSender<EngineToGateway>,
}

impl EngineRouter {
    /// Create a new router from engine configurations
    pub async fn new(configs: Vec<EngineConfig>) -> Result<Self> {
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        let router = Self {
            senders: Arc::new(Mutex::new(HashMap::new())),
            event_rx: Arc::new(Mutex::new(event_rx)),
            event_tx,
        };

        // Connect to all engines
        for config in configs {
            router.connect_to_engine(&config).await?;
        }

        Ok(router)
    }

    /// Connect to a single engine and start listening for events
    async fn connect_to_engine(&self, config: &EngineConfig) -> Result<()> {
        let addr = config
            .address
            .parse::<SocketAddr>()
            .with_context(|| format!("Invalid address: {}", config.address))?;

        let stream = TcpStream::connect(addr)
            .await
            .with_context(|| format!("Failed to connect to engine at {}", addr))?;

        // Enable TCP_NODELAY to disable Nagle's algorithm (critical for low latency)
        stream
            .set_nodelay(true)
            .context("Failed to set TCP_NODELAY")?;

        // Set larger socket buffers for better throughput
        // Note: Tokio doesn't expose buffer size methods directly, would need socket2 crate
        // Leaving commented for future optimization if needed
        // let _ = stream.set_recv_buffer_size(256 * 1024);
        // let _ = stream.set_send_buffer_size(256 * 1024);

        let framed = LengthDelimitedCodec::builder()
            .little_endian()
            .max_frame_length(10 * 1024 * 1024) // 10MB max frame
            .new_framed(stream);

        tracing::info!(
            "Connected to engine for symbol_id={} at {}",
            config.symbol_id,
            addr
        );

        // Split into read and write halves
        let (write_half, read_half) = framed.split();

        // Store write half for sending commands
        self.senders
            .lock()
            .await
            .insert(config.symbol_id, write_half);

        // Spawn a task to listen for events from this engine (owns read half)
        let symbol_id = config.symbol_id;
        let event_tx = self.event_tx.clone();

        tokio::spawn(async move {
            engine_reader_task(symbol_id, read_half, event_tx).await;
        });

        Ok(())
    }

    /// Route a command to the appropriate engine server
    pub async fn route_to_engine(&self, msg: &GatewayToEngine, symbol_id: SymbolId) -> Result<()> {
        let serialized =
            postcard::to_allocvec(msg).context("Failed to serialize GatewayToEngine")?;

        let mut senders = self.senders.lock().await;

        let sender = senders
            .get_mut(&symbol_id)
            .with_context(|| format!("No engine configured for symbol_id={}", symbol_id))?;

        // Use feed() + flush() instead of send() for better batching
        sender
            .feed(Bytes::from(serialized))
            .await
            .context("Failed to feed to engine")?;

        sender.flush().await.context("Failed to flush to engine")?;

        Ok(())
    }

    /// Receive the next event from any engine (blocks until event available)
    pub async fn recv_event(&self) -> Option<EngineToGateway> {
        let mut rx = self.event_rx.lock().await;
        rx.recv().await
    }

    /// Get the number of connected engines
    pub async fn num_connections(&self) -> usize {
        self.senders.lock().await.len()
    }
}

/// Background task that reads events from an engine
async fn engine_reader_task(
    symbol_id: SymbolId,
    mut read_half: SplitStream<Framed<TcpStream, LengthDelimitedCodec>>,
    event_tx: mpsc::UnboundedSender<EngineToGateway>,
) {
    loop {
        match read_half.next().await {
            Some(Ok(bytes)) => match postcard::from_bytes::<EngineToGateway>(&bytes) {
                Ok(event) => {
                    if event_tx.send(event).is_err() {
                        tracing::warn!("Event channel closed for symbol_id={}", symbol_id);
                        break;
                    }
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to deserialize EngineToGateway for symbol_id={}: {}",
                        symbol_id,
                        e
                    );
                }
            },
            Some(Err(e)) => {
                tracing::error!("Error receiving from engine symbol_id={}: {}", symbol_id, e);
                break;
            }
            None => {
                tracing::warn!("Connection closed for symbol_id={}", symbol_id);
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engines_config_parse() {
        let toml_str = r#"
[[engines]]
symbol_id = 1
symbol_name = "BTC/USD"
address = "127.0.0.1:9001"

[[engines]]
symbol_id = 2
symbol_name = "ETH/USD"
address = "127.0.0.1:9002"
"#;

        let config: EnginesConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.engines.len(), 2);
        assert_eq!(config.engines[0].symbol_id, 1);
        assert_eq!(config.engines[0].symbol_name, "BTC/USD");
        assert_eq!(config.engines[1].symbol_id, 2);
    }

    #[test]
    fn test_routing_table() {
        let configs = [
            EngineConfig {
                symbol_id: 1,
                symbol_name: "BTC/USD".to_string(),
                address: "127.0.0.1:9001".to_string(),
            },
            EngineConfig {
                symbol_id: 2,
                symbol_name: "ETH/USD".to_string(),
                address: "127.0.0.1:9002".to_string(),
            },
        ];

        let routing_table: HashMap<SymbolId, SocketAddr> = configs
            .iter()
            .map(|cfg| {
                let addr = cfg.address.parse::<SocketAddr>().unwrap();
                (cfg.symbol_id, addr)
            })
            .collect();

        assert_eq!(routing_table.len(), 2);
        assert!(routing_table.contains_key(&1));
        assert!(routing_table.contains_key(&2));
    }
}

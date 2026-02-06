use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "engine_server")]
#[command(about = "Trading engine server - handles order matching for a single symbol", long_about = None)]
pub struct Args {
    /// Symbol ID this engine handles (e.g., 1 for BTC/USD)
    #[arg(long)]
    pub symbol_id: u32,

    /// Symbol name for logging (e.g., "BTC/USD")
    #[arg(long)]
    pub symbol_name: String,

    /// Address to listen for gateway connections
    #[arg(long, default_value = "0.0.0.0:9100")]
    pub listen_addr: String,

    /// Admin HTTP server address
    #[arg(long, default_value = "0.0.0.0:8080")]
    pub admin_addr: String,

    /// Path to order book journal file
    #[arg(long)]
    pub journal_path: Option<String>,

    /// Directory for order book snapshots
    #[arg(long)]
    pub snapshot_dir: Option<String>,

    /// Journal batch size before fsync
    #[arg(long, default_value_t = 100)]
    pub journal_batch_size: usize,

    /// Snapshot interval (commands)
    #[arg(long, default_value_t = 100_000)]
    pub snapshot_interval: u64,

    /// max framed payload size
    #[arg(long, default_value_t = 64 * 1024)]
    pub max_frame: usize,

    /// engine ingress queue capacity
    #[arg(long, default_value_t = 100_000)]
    pub ingress_cap: usize,
}

impl Args {
    /// Get the journal path (with symbol-specific default)
    pub fn get_journal_path(&self) -> String {
        self.journal_path
            .clone()
            .unwrap_or_else(|| format!("journal/engine_{}_journal.bin", self.symbol_id))
    }

    /// Get the snapshot directory (with symbol-specific default)
    pub fn get_snapshot_dir(&self) -> String {
        self.snapshot_dir
            .clone()
            .unwrap_or_else(|| format!("journal/engine_{}_snapshots", self.symbol_id))
    }
}

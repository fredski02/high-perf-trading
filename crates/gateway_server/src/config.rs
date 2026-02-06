use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "gateway")]
#[command(about = "Trading gateway server - handles accounts, risk, and routing", long_about = None)]
pub struct Args {
    /// Client connection address for binary protocol
    #[arg(long, default_value = "0.0.0.0:9000")]
    pub client_binary_addr: String,

    /// Client connection address for JSON protocol
    #[arg(long, default_value = "0.0.0.0:9001")]
    pub client_json_addr: String,

    /// Admin HTTP server address
    #[arg(long, default_value = "0.0.0.0:8080")]
    pub admin_addr: String,

    /// Path to account journal file
    #[arg(long, default_value = "journal/gateway_journal.bin")]
    pub journal_path: String,

    /// Directory for account snapshots
    #[arg(long, default_value = "journal/gateway_snapshots")]
    pub snapshot_dir: String,

    /// Journal fsync batch size (lower = more durable, higher = faster)
    #[arg(long, default_value_t = 100)]
    pub journal_batch_size: usize,

    /// Snapshot interval in commands
    #[arg(long, default_value_t = 100000)]
    pub snapshot_interval: u64,

    /// Ingress channel capacity (backpressure threshold)
    #[arg(long, default_value_t = 100000)]
    pub ingress_cap: usize,

    /// Maximum frame size in bytes
    #[arg(long, default_value_t = 10 * 1024 * 1024)]
    pub max_frame: usize,

    /// Engine configuration file (TOML with engine addresses)
    #[arg(long, default_value = "engines.toml")]
    pub engines_config: String,
}

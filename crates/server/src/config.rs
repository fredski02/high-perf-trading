use clap::Parser;

#[derive(Parser, Debug)]
pub struct Args {
    #[arg(long, default_value = "0.0.0.0:9000")]
    pub binary_addr: String,
    #[arg(long, default_value = "0.0.0.0:9001")]
    pub json_addr: String,

    #[arg(long, default_value = "0.0.0.0:8080")]
    pub admin_addr: String,

    #[arg(long, default_value = "journal.bin")]
    pub journal_path: String,

    #[arg(long, default_value = "snapshots")]
    pub snapshot_dir: String,

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

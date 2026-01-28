mod server;

use clap::Parser;
use tracing_subscriber::EnvFilter;

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

    /// max framed payload size
    #[arg(long, default_value_t = 64 * 1024)]
    pub max_frame: usize,

    /// engine ingress queue capacity
    #[arg(long, default_value_t = 100_000)]
    pub ingress_cap: usize,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    server::run(args).await
}

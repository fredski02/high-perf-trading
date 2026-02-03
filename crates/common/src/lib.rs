pub mod types;
pub mod metrics;

pub use types::*;
pub use metrics::Metrics;

#[derive(Debug, thiserror::Error)]
pub enum ProtoError {
    #[error("frame too large: {0}")]
    FrameTooLarge(usize),
    #[error("malformed message: {0}")]
    Malformed(&'static str),
}
pub mod types;
pub mod metrics;
pub mod gateway_protocol;

pub use types::*;
pub use metrics::Metrics;
pub use gateway_protocol::*;

#[derive(Debug, thiserror::Error)]
pub enum ProtoError {
    #[error("frame too large: {0}")]
    FrameTooLarge(usize),
    #[error("malformed message: {0}")]
    Malformed(&'static str),
}
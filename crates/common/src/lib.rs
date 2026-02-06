pub mod gateway_protocol;
pub mod metrics;
pub mod types;

pub use gateway_protocol::*;
pub use metrics::Metrics;
pub use types::*;

#[derive(Debug, thiserror::Error)]
pub enum ProtoError {
    #[error("frame too large: {0}")]
    FrameTooLarge(usize),
    #[error("malformed message: {0}")]
    Malformed(&'static str),
}

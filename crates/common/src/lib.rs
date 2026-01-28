pub mod types;
pub use types::*;

#[derive(Debug, thiserror::Error)]
pub enum ProtoError {
    #[error("frame too large: {0}")]
    FrameTooLarge(usize),
    #[error("malformed message: {0}")]
    Malformed(&'static str),
}

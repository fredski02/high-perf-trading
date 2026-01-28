use bytes::{Bytes, BytesMut};
use common::{Command, Event, ProtoError};

pub trait Codec: Send + Sync + 'static {
    fn name(&self) -> &'static str;
    fn encode_event(&self, ev: &Event, out: &mut BytesMut) -> Result<(), ProtoError>;
    fn decode_command(&self, input: &Bytes) -> Result<Command, ProtoError>;
}

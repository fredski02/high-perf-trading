use crate::codec::Codec;
use bytes::{Bytes, BytesMut};
use common::{Command, Event, ProtoError};

#[derive(Clone, Default)]
pub struct JsonCodec;

impl Codec for JsonCodec {
    fn name(&self) -> &'static str {
        "json"
    }

    fn encode_event(&self, ev: &Event, out: &mut BytesMut) -> Result<(), ProtoError> {
        serde_json::to_writer(BytesMutWriter(out), ev)
            .map_err(|_| ProtoError::Malformed("json encode"))?;
        Ok(())
    }

    fn decode_command(&self, input: &Bytes) -> Result<Command, ProtoError> {
        serde_json::from_slice::<Command>(input).map_err(|_| ProtoError::Malformed("json decode"))
    }
}

struct BytesMutWriter<'a>(&'a mut BytesMut);

impl<'a> std::io::Write for BytesMutWriter<'a> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

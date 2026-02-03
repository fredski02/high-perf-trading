use bytes::{Buf, BufMut, Bytes, BytesMut};
use common::ProtoError;

/// Try to read a length-prefixed frame from the buffer
/// Returns Some(frame_bytes) if complete, None if need more data
pub fn try_read_frame(buf: &mut BytesMut, max_frame: usize) -> Result<Option<Bytes>, ProtoError> {
    if buf.len() < 4 {
        return Ok(None);
    }

    let len = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;

    if len > max_frame {
        return Err(ProtoError::FrameTooLarge(len));
    }

    if buf.len() < 4 + len {
        return Ok(None);
    }

    buf.advance(4);
    Ok(Some(buf.split_to(len).freeze()))
}

/// Frame a payload with length prefix
pub fn frame_payload(payload: Bytes) -> Bytes {
    let len = payload.len() as u32;
    let mut out = BytesMut::with_capacity(4 + payload.len());
    out.put_u32_le(len);
    out.put(payload);
    out.freeze()
}

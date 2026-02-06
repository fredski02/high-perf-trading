use bytes::{Buf, BufMut, Bytes, BytesMut};
use common::{Command, Event, ProtoError};

use crate::codec::Codec;

const MT_NEW_ORDER: u16 = 1;
const MT_CANCEL: u16 = 2;
const MT_REPLACE: u16 = 3;
const MT_SET_RISK_LIMITS: u16 = 4;
const MT_QUERY_ACCOUNT: u16 = 5;

const MT_ACK: u16 = 101;
const MT_REJECT: u16 = 102;
const MT_FILL: u16 = 103;
const MT_BOOK_TOP: u16 = 104;
const MT_ACCOUNT_STATE: u16 = 105;

#[derive(Clone, Default)]
pub struct BinaryCodec;

impl Codec for BinaryCodec {
    fn name(&self) -> &'static str {
        "binary"
    }

    fn encode_event(&self, ev: &Event, out: &mut BytesMut) -> Result<(), ProtoError> {
        match ev {
            Event::Ack(a) => {
                out.put_u16_le(MT_ACK);
                out.put_u64_le(a.server_seq);
                out.put_u64_le(a.client_seq);
                out.put_u64_le(a.order_id);
            }
            Event::Reject(r) => {
                out.put_u16_le(MT_REJECT);
                out.put_u64_le(r.server_seq);
                out.put_u64_le(r.client_seq);
                out.put_u8(match r.reason {
                    common::RejectReason::Invalid => 1,
                    common::RejectReason::Risk => 2,
                    common::RejectReason::Overloaded => 3,
                    common::RejectReason::NotFound => 4,
                    common::RejectReason::PostOnlyWouldCross => 5,
                });
                match r.order_id {
                    Some(id) => {
                        out.put_u8(1);
                        out.put_u64_le(id);
                    }
                    None => {
                        out.put_u8(0);
                    }
                }
            }
            Event::Fill(f) => {
                out.put_u16_le(MT_FILL);
                out.put_u64_le(f.server_seq);
                out.put_u64_le(f.client_seq);
                out.put_u32_le(f.symbol_id);
                out.put_u64_le(f.taker_order_id);
                out.put_u64_le(f.maker_order_id);
                out.put_i64_le(f.price);
                out.put_i64_le(f.qty);
            }
            Event::BookTop(b) => {
                out.put_u16_le(MT_BOOK_TOP);
                out.put_u64_le(b.server_seq);
                out.put_u32_le(b.symbol_id);
                put_opt_i64(out, b.best_bid_px);
                put_opt_i64(out, b.best_bid_qty);
                put_opt_i64(out, b.best_ask_px);
                put_opt_i64(out, b.best_ask_qty);
            }
            Event::AccountState(a) => {
                out.put_u16_le(MT_ACCOUNT_STATE);
                out.put_u64_le(a.server_seq);
                out.put_u64_le(a.client_seq);
                out.put_u32_le(a.account_id);
                out.put_u32_le(a.symbol_id);
                // Position
                out.put_i64_le(a.position.net_position);
                out.put_i64_le(a.position.avg_price);
                out.put_i64_le(a.position.realized_pnl);
                // RiskLimits
                out.put_i64_le(a.risk_limits.max_long_position);
                out.put_i64_le(a.risk_limits.max_short_position);
                out.put_i64_le(a.risk_limits.max_order_size);
            }
        }
        Ok(())
    }

    fn decode_command(&self, input: &Bytes) -> Result<Command, ProtoError> {
        let mut b = input.clone();
        if b.remaining() < 2 {
            return Err(ProtoError::Malformed("binary: missing msg_type"));
        }
        let mt = b.get_u16_le();

        match mt {
            MT_NEW_ORDER => {
                let client_seq = get_u64(&mut b)?;
                let order_id = get_u64(&mut b)?;
                let account_id = get_u32(&mut b)?;
                let symbol_id = get_u32(&mut b)?;
                let side = match get_u8(&mut b)? {
                    0 => common::Side::Buy,
                    1 => common::Side::Sell,
                    _ => return Err(ProtoError::Malformed("binary: bad side")),
                };
                let price = get_i64(&mut b)?;
                let qty = get_i64(&mut b)?;
                let tif = match get_u8(&mut b)? {
                    0 => common::TimeInForce::Gtc,
                    1 => common::TimeInForce::Ioc,
                    _ => return Err(ProtoError::Malformed("binary: bad tif")),
                };
                let post_only = match get_u8(&mut b)? {
                    0 => false,
                    1 => true,
                    _ => return Err(ProtoError::Malformed("binary: bad post_only")),
                };
                Ok(Command::NewOrder(common::NewOrder {
                    client_seq,
                    order_id,
                    account_id,
                    symbol_id,
                    side,
                    price,
                    qty,
                    tif,
                    flags: common::OrderFlags { post_only },
                }))
            }
            MT_CANCEL => {
                let client_seq = get_u64(&mut b)?;
                let order_id = get_u64(&mut b)?;
                let account_id = get_u32(&mut b)?;
                let symbol_id = get_u32(&mut b)?;
                Ok(Command::Cancel(common::Cancel {
                    client_seq,
                    order_id,
                    account_id,
                    symbol_id,
                }))
            }
            MT_REPLACE => {
                let client_seq = get_u64(&mut b)?;
                let order_id = get_u64(&mut b)?;
                let account_id = get_u32(&mut b)?;
                let symbol_id = get_u32(&mut b)?;
                let new_price = get_i64(&mut b)?;
                let new_qty = get_i64(&mut b)?;
                Ok(Command::Replace(common::Replace {
                    client_seq,
                    order_id,
                    account_id,
                    symbol_id,
                    new_price,
                    new_qty,
                }))
            }
            MT_SET_RISK_LIMITS => {
                let client_seq = get_u64(&mut b)?;
                let account_id = get_u32(&mut b)?;
                let symbol_id = get_u32(&mut b)?;
                let max_long_position = get_i64(&mut b)?;
                let max_short_position = get_i64(&mut b)?;
                let max_order_size = get_i64(&mut b)?;
                Ok(Command::SetRiskLimits(common::SetRiskLimits {
                    client_seq,
                    account_id,
                    symbol_id,
                    limits: common::RiskLimits {
                        max_long_position,
                        max_short_position,
                        max_order_size,
                    },
                }))
            }
            MT_QUERY_ACCOUNT => {
                let client_seq = get_u64(&mut b)?;
                let account_id = get_u32(&mut b)?;
                let symbol_id = get_u32(&mut b)?;
                Ok(Command::QueryAccount(common::QueryAccount {
                    client_seq,
                    account_id,
                    symbol_id,
                }))
            }
            _ => Err(ProtoError::Malformed("binary: unknown msg_type")),
        }
    }
}
fn put_opt_i64(out: &mut BytesMut, v: Option<i64>) {
    match v {
        Some(x) => {
            out.put_u8(1);
            out.put_i64_le(x);
        }
        None => out.put_u8(0),
    }
}
fn get_u8(b: &mut Bytes) -> Result<u8, ProtoError> {
    if b.remaining() < 1 {
        return Err(ProtoError::Malformed("binary: underrun"));
    }
    Ok(b.get_u8())
}
fn get_u32(b: &mut Bytes) -> Result<u32, ProtoError> {
    if b.remaining() < 4 {
        return Err(ProtoError::Malformed("binary: underrun"));
    }
    Ok(b.get_u32_le())
}
fn get_u64(b: &mut Bytes) -> Result<u64, ProtoError> {
    if b.remaining() < 8 {
        return Err(ProtoError::Malformed("binary: underrun"));
    }
    Ok(b.get_u64_le())
}
fn get_i64(b: &mut Bytes) -> Result<i64, ProtoError> {
    if b.remaining() < 8 {
        return Err(ProtoError::Malformed("binary: underrun"));
    }
    Ok(b.get_i64_le())
}

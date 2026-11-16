//! Original user mark-read DATA. This is NOT automatic received setRead and
//! grants no IO from a scalar, status value, caller UID/token or notification.
//! The verified native UID path is5B312C->40C014; non-UID resolver is separate.
use super::{im_data::{ImBox,BoxKind,SetReadResult,decode_set_read_result},im_status::StatusLabel,
    rpc_stream::{CodecError,RpcBytes,Writer,MAX_FIELD_BYTES}};
pub(super) const MARK_READ_METHOD:&str="setStatus.GroupServer.Group";
pub(super) const MARK_READ_PROPERTY:&[u8]=b"P:P_UDMR-N";
/// Parameters must later come from an explicit original-owned user action and
/// the current authenticated SDK. This pure encoder is not a write permit.
pub(super) fn encode_mark_read(label:StatusLabel<'_>,current_uid:&[u8],index:i64,epoch_ms:i64)->Result<RpcBytes,CodecError>{
    let box_id=label.box_bytes();if ImBox::parse(&box_id)?.kind()!=BoxKind::P2P{return Err(CodecError::InvalidField);}
    if index<0||epoch_ms<0||current_uid.is_empty()||current_uid.len()>MAX_FIELD_BYTES||current_uid.iter().any(u8::is_ascii_control){return Err(CodecError::InvalidField);}
    // The two native writer calls1 then0 are BE16 count/version words: one
    // entry[0], not a different[1,0] version pair or a major/minor header.
    let mut w=Writer::versioned()?;w.bytes(label.bytes())?;w.bytes(current_uid)?;w.bytes(MARK_READ_PROPERTY)?;
    w.bytes(index.to_string().as_bytes())?;w.i64(epoch_ms)?;Ok(w.finish())
}
pub(super) fn decode_mark_read_result(code:u32,bytes:&[u8])->Result<SetReadResult<'_>,CodecError>{
    //5A91F8 primary16E7068 slot0->5BDF50->4051B8: bool/Commonfooter, no
    // write retry on40C484 native version-error response is reproduced here.
    decode_set_read_result(code,bytes)
}

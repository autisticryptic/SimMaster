//! Normalized IM consumption DATA for a current original binding. No SQL/IO,
//! SDK/receipt/WRITE/channel permission is created by selecting a body kind.
use super::{im_data::ImMessage,im_delivery::ImBinding,im_info::ImInfo,im_text::ImText};
use std::fmt;
pub(super) enum ImContent{Text(ImText),Info(ImInfo)}
impl fmt::Debug for ImContent{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImContent(<redacted>)")}}
impl ImContent{
    pub(super) fn capture(binding:&ImBinding,message:&ImMessage<'_>)->Result<Option<Self>,&'static str>{
        if let Some(text)=ImText::decode(message).map_err(|_|"app_account_im_message_invalid")?{return Ok(Some(Self::Text(text)));}
        if !binding.receives_info(){return Ok(None);}
        ImInfo::decode(message).map(|value|value.map(Self::Info)).map_err(|_|"app_account_im_message_invalid")
    }
    pub(super) fn value(&self)->&str{match self{Self::Text(text)=>text.text(),Self::Info(info)=>info.content()}}
    pub(super) fn body_kind(&self)->&'static str{match self{Self::Text(_)=>"text",Self::Info(_)=>"info"}}
    pub(super) fn info_type(&self)->Option<&[u8]>{match self{Self::Info(info)=>Some(info.kind()),_=>None}}
    pub(super) fn system_info(&self)->bool{matches!(self,Self::Info(info)if info.system())}
}

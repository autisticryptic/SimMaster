//! Original Info.<type>/System.Info.<type> JSON DATA, not text/SMS/readiness.
//! Mapper5AB718 copies base recognized fields and content->MtcImInfoContentKey;
//! 5C76E4/5C6BE8 select the original Info class and retain its subtype. This
//! decoder has no owner/ticket/SQL/IO/notification or write permission.
use super::{im_data::ImMessage,im_text::{bounded_json,TextField},rpc_stream::{CodecError,MAX_FIELD_BYTES,MAX_MAP_ENTRIES}};
use serde::{de::{self,MapAccess,Visitor},Deserialize,Deserializer};
use std::fmt;
use zeroize::Zeroizing;

pub(super) struct ImInfo{kind:Zeroizing<Vec<u8>>,system:bool,body:InfoBody}
struct InfoBody{content:Zeroizing<String>,common:[Option<Zeroizing<String>>;5]}
impl fmt::Debug for ImInfo{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImInfo(<redacted>)")}}
impl fmt::Debug for InfoBody{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImInfoBody(<redacted>)")}}
impl ImInfo{
    pub(super) fn decode(message:&ImMessage<'_>)->Result<Option<Self>,CodecError>{
        let(kind,system)=if let Some(v)=message.kind.strip_prefix(b"System.Info."){(v,true)}else if let Some(v)=message.kind.strip_prefix(b"Info."){(v,false)}else{return Ok(None);};
        // Explicit bounded string subset; not a transport, authority or command.
        if kind.len()>128{return Err(CodecError::TooLarge);}
        if kind.is_empty()||!kind.iter().all(|v|v.is_ascii_alphanumeric()||matches!(*v,b'_'|b'-'|b'.')){return Err(CodecError::InvalidField);}
        if message.content.len()>MAX_FIELD_BYTES{return Err(CodecError::TooLarge);}
        if !bounded_json(message.content){return Err(CodecError::InvalidField);}
        let body:InfoBody=serde_json::from_slice(message.content).map_err(|_|CodecError::InvalidField)?;
        Ok(Some(Self{kind:Zeroizing::new(kind.to_vec()),system,body}))
    }
    pub(super) fn kind(&self)->&[u8]{&self.kind}
    pub(super) fn system(&self)->bool{self.system}
    pub(super) fn content(&self)->&str{&self.body.content}
    pub(super) fn field(&self,field:TextField)->Option<&str>{let index=match field{TextField::Text=>return None,TextField::ImdnId=>0,TextField::ConversationId=>1,TextField::DisplayName=>2,TextField::UserData=>3,TextField::OnlineMessage=>4};self.body.common[index].as_deref().map(String::as_str)}
}
impl<'de> Deserialize<'de> for InfoBody{
    fn deserialize<D:Deserializer<'de>>(deserializer:D)->Result<Self,D::Error>{
        struct Fields;
        impl<'de> Visitor<'de> for Fields{
            type Value=InfoBody;
            fn expecting(&self,f:&mut fmt::Formatter)->fmt::Result{f.write_str("bounded original IM Info object")}
            fn visit_map<M:MapAccess<'de>>(self,mut map:M)->Result<Self::Value,M::Error>{
                let mut common=std::array::from_fn(|_|None);let mut content=None;let mut keys:Vec<Zeroizing<String>>=Vec::new();
                while let Some(key)=map.next_key::<String>()?{
                    let key=Zeroizing::new(key);
                    if keys.len()>=MAX_MAP_ENTRIES||key.len()>MAX_FIELD_BYTES{return Err(de::Error::custom("too many IM Info fields"));}
                    if keys.iter().any(|v|v.as_str()==key.as_str()){return Err(de::Error::custom("duplicate IM Info field"));}
                    let field=match key.as_str(){"imdnId"=>Some(0),"csnId"=>Some(1),"displayName"=>Some(2),"userData"=>Some(3),"OnlineMsg"=>Some(4),_=>None};let is_content=key.as_str()=="content";keys.push(key);
                    if is_content||field.is_some(){let value=Zeroizing::new(map.next_value::<String>()?);if value.len()>MAX_FIELD_BYTES||value.contains('\0'){return Err(de::Error::custom("invalid IM Info field"));}if is_content{content=Some(value);}else{common[field.expect("recognized common Info field")]=Some(value);}}
                    else{let _=map.next_value::<de::IgnoredAny>()?;}
                }
                Ok(InfoBody{content:content.ok_or_else(||de::Error::custom("missing IM Info content"))?,common})
            }
        }
        deserializer.deserialize_map(Fields)
    }
}

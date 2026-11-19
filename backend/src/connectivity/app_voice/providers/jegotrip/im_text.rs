//! Structured Text/System.Text content from the original IM record, not SMS.
//!
//! 5C3AFC parses JSON; 5C39FC accepts only keys recognized by the message
//! class's virtual mapper. Text mapper5AB768 adds text->MtcImTextKey to the
//! five base keys at5AB5F0. Do not display the raw content JSON as user text,
//! derive an identity from metadata or use OnlineMsg as a live-session grant.
use super::{im_data::ImMessage,rpc_stream::{CodecError,MAX_FIELD_BYTES,MAX_MAP_ENTRIES}};
use serde::{de::{self,MapAccess,Visitor},Deserialize,Deserializer};
use std::fmt;
use zeroize::Zeroizing;
const MAX_JSON_DEPTH:usize=32;
pub(super) fn bounded_json(bytes:&[u8])->bool{
    let(mut depth,mut quoted,mut escaped)=(0usize,false,false);
    for &byte in bytes{
        if quoted{if escaped{escaped=false;}else if byte==b'\\'{escaped=true;}else if byte==b'"'{quoted=false;}continue;}
        match byte{b'"'=>quoted=true,b'{'|b'['=>{depth+=1;if depth>MAX_JSON_DEPTH{return false;}},b'}'|b']'=>{let Some(next)=depth.checked_sub(1)else{return false;};depth=next;},_=>{}}
    }
    !quoted&&!escaped&&depth==0
}

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(super) enum TextField{Text,ImdnId,ConversationId,DisplayName,UserData,OnlineMessage}
impl TextField{fn index(self)->usize{match self{Self::Text=>0,Self::ImdnId=>1,Self::ConversationId=>2,Self::DisplayName=>3,Self::UserData=>4,Self::OnlineMessage=>5}}}
pub(super) struct ImText{fields:[Option<Zeroizing<String>>;6]}
impl fmt::Debug for ImText{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImText(<redacted>)")}}
impl ImText{
    pub(super) fn decode(message:&ImMessage<'_>)->Result<Option<Self>,CodecError>{
        if !matches!(message.kind,b"Text"|b"System.Text"){return Ok(None);}
        if message.content.len()>MAX_FIELD_BYTES{return Err(CodecError::TooLarge);}
        if !bounded_json(message.content){return Err(CodecError::InvalidField);}
        let text:Self=serde_json::from_slice(message.content).map_err(|_|CodecError::InvalidField)?;
        Ok(Some(text))
    }
    pub(super) fn field(&self,field:TextField)->Option<&str>{self.fields[field.index()].as_deref().map(String::as_str)}
    pub(super) fn text(&self)->&str{self.field(TextField::Text).expect("decoded required original IM text")}
}
impl<'de> Deserialize<'de> for ImText{
    fn deserialize<D:Deserializer<'de>>(deserializer:D)->Result<Self,D::Error>{
        struct Fields;
        impl<'de> Visitor<'de> for Fields{
            type Value=ImText;
            fn expecting(&self,f:&mut fmt::Formatter)->fmt::Result{f.write_str("bounded original IM text object")}
            fn visit_map<M:MapAccess<'de>>(self,mut map:M)->Result<Self::Value,M::Error>{
                let mut result=ImText{fields:std::array::from_fn(|_|None)};
                let mut keys:Vec<Zeroizing<String>>=Vec::new();
                while let Some(key)=map.next_key::<String>()?{
                    let key=Zeroizing::new(key);
                    if keys.len()>=MAX_MAP_ENTRIES||key.len()>MAX_FIELD_BYTES{return Err(de::Error::custom("too many IM text fields"));}
                    if keys.iter().any(|prior|prior.as_str()==key.as_str()){return Err(de::Error::custom("duplicate IM text field"));}
                    let field=match key.as_str(){"text"=>Some(TextField::Text),"imdnId"=>Some(TextField::ImdnId),"csnId"=>Some(TextField::ConversationId),
                        "displayName"=>Some(TextField::DisplayName),"userData"=>Some(TextField::UserData),"OnlineMsg"=>Some(TextField::OnlineMessage),_=>None};
                    keys.push(key);
                    if let Some(field)=field{
                        let value=Zeroizing::new(map.next_value::<String>()?);
                        if value.len()>MAX_FIELD_BYTES||value.contains('\0'){return Err(de::Error::custom("invalid IM text field"));}
                        result.fields[field.index()]=Some(value);
                    }else{
                        // Native mapper ignores unrecognized content fields.
                        // Skip bounded JSON DATA without rendering or granting
                        // semantics to it; the preflight limits nesting before
                        // even this allocation-free skipping path is entered.
                        let _=map.next_value::<de::IgnoredAny>()?;
                    }
                }
                if result.fields[TextField::Text.index()].is_none(){return Err(de::Error::custom("missing IM text field"));}
                Ok(result)
            }
        }
        deserializer.deserialize_map(Fields)
    }
}

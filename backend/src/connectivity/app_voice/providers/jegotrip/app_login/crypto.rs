//! Recovered App HTTP primitives. These are compatibility codecs, not a new
//! authentication grant; only the authenticated HTTPS response decides outcome.
use super::{Error, config::{BodyProtection, HttpDeployment}};
use super::super::credentials::Secret;
use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit, generic_array::GenericArray};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::Value;
use zeroize::{Zeroize, Zeroizing};

pub(super) const MAX_BODY: usize = 256 * 1024;
pub(super) struct PhoneMaterial { pub token: Secret, pub sip_password: Secret }
pub(super) struct JsonDoc(pub Value);
impl Drop for JsonDoc { fn drop(&mut self) { wipe(&mut self.0); } }
fn wipe(v: &mut Value) {
    match v { Value::String(s) => s.zeroize(), Value::Array(a) => a.iter_mut().for_each(wipe),
        Value::Object(o) => o.values_mut().for_each(wipe), _ => {} }
}
fn bad() -> Error { Error::new("app_login_response_invalid") }
pub(super) fn parse(bytes: &[u8]) -> Result<JsonDoc, Error> {
    if bytes.is_empty() || bytes.len()>MAX_BODY { return Err(bad()); }
    let value:Value=serde_json::from_slice(bytes).map_err(|_|bad())?;
    if !value.is_object() { return Err(bad()); } Ok(JsonDoc(value))
}
pub(super) fn success(doc: &JsonDoc) -> Result<(),Error> {
    let code=doc.0.get("code").and_then(|v| {
        if let Some(text)=v.as_str() {
            if text.is_empty() || text.len()>6 || !text.bytes().all(|b|b.is_ascii_digit()) { return None; }
            let value:u32=text.parse().ok()?;
            (value.to_string()==text).then_some(value)
        } else { v.as_u64().filter(|n|*n<=999999).map(|n|n as u32) }
    });
    if code==Some(0) { return Ok(()); }
    let text=match code { Some(88001)=>"app_login_verification_required",Some(2044)=>"app_login_migration_required",
        Some(2006)=>"app_login_existing_business_action_required",Some(429)=>"app_login_session_revoked",
        Some(9999)=>"app_login_service_maintenance",None=>"app_login_response_invalid",_=>"app_login_server_rejected" };
    Err(Error { code:text,server_code:code })
}
pub(super) fn body(doc: &JsonDoc) -> Result<&serde_json::Map<String,Value>,Error> {
    doc.0.get("body").and_then(Value::as_object).ok_or_else(bad)
}
pub(super) fn secret_field(body: &serde_json::Map<String,Value>, name: &str) -> Result<Secret,Error> {
    let value=body.get(name).and_then(Value::as_str).ok_or_else(bad)?;
    if value.is_empty() || value.len()>16_384 || value.chars().any(char::is_control) { return Err(bad()); }
    Ok(Secret::from_owned(value.to_owned()))
}
pub(super) fn password_hash(salt:&Secret,password:&Secret)->Secret {
    let input=Zeroizing::new(format!("{}{}",salt.expose_to_backend(),password.expose_to_backend()));
    let mut digest=md5::compute(input.as_bytes());
    let value=format!("{digest:x}"); digest.0.zeroize(); Secret::from_owned(value)
}
pub(super) fn url_signature(salt:&Secret,millis:u64)->String {
    let source=salt.expose_to_backend().as_bytes();
    let mut data=Zeroizing::new([0u8;16]);
    for i in 0..8 { data[i]=source[i].wrapping_add(i as u8); }
    data[8..].copy_from_slice(&millis.to_be_bytes());
    ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY,&*data).as_ref().iter().map(|b|format!("{b:02x}")).collect()
}
fn derived_key(protection:&BodyProtection,nonce:&str)->Zeroizing<[u8;16]> {
    let input=Zeroizing::new(format!("{}{}",protection.secret_value.expose_to_backend(),nonce));
    let mut digest=md5::compute(input.as_bytes());let text=Zeroizing::new(format!("{digest:x}"));digest.0.zeroize();
    let mut key=Zeroizing::new([0;16]);key.copy_from_slice(&text.as_bytes()[8..24]);key
}
fn pad(input:&[u8],block:usize)->Zeroizing<Vec<u8>> {
    let mut out=Zeroizing::new(input.to_vec());let n=block-input.len()%block;out.resize(input.len()+n,n as u8);out
}
fn unpad(mut bytes:Zeroizing<Vec<u8>>,block:usize)->Result<Zeroizing<Vec<u8>>,Error> {
    let n=*bytes.last().ok_or_else(bad)? as usize;
    if n==0 || n>block || n>bytes.len() || bytes[bytes.len()-n..].iter().any(|b|*b as usize!=n) { return Err(bad()); }
    let length=bytes.len()-n;bytes.truncate(length);Ok(bytes)
}
fn decode64(input:&str)->Result<Zeroizing<Vec<u8>>,Error> {
    if input.len()>MAX_BODY { return Err(bad()); }
    let clean=Zeroizing::new(input.bytes().filter(|b|!b.is_ascii_whitespace()).collect::<Vec<_>>());
    STANDARD.decode(&*clean).map(Zeroizing::new).map_err(|_|bad())
}
fn aes_encrypt(input:&[u8],key:&[u8;16])->Result<Zeroizing<Vec<u8>>,Error> {
    let cipher=aes::Aes128::new_from_slice(key).map_err(|_|bad())?;let mut bytes=pad(input,16);
    for block in bytes.chunks_exact_mut(16) { cipher.encrypt_block(GenericArray::from_mut_slice(block)); } Ok(bytes)
}
fn aes_decrypt(input:&str,key:&[u8;16])->Result<Zeroizing<Vec<u8>>,Error> {
    let mut bytes=decode64(input)?;if bytes.is_empty() || bytes.len()%16!=0 { return Err(bad()); }
    let cipher=aes::Aes128::new_from_slice(key).map_err(|_|bad())?;
    for block in bytes.chunks_exact_mut(16) { cipher.decrypt_block(GenericArray::from_mut_slice(block)); } unpad(bytes,16)
}
pub(super) fn request_body(input:&[u8],protection:Option<&BodyProtection>,nonce:&str)->Result<Zeroizing<Vec<u8>>,Error> {
    if input.len()>MAX_BODY/2 { return Err(Error::new("app_login_request_invalid")); }
    let Some(p)=protection else { return Ok(Zeroizing::new(input.to_vec())); };
    if nonce.is_empty() || nonce.len()>128 || nonce.contains(';') || nonce.chars().any(char::is_control) { return Err(Error::new("app_login_request_invalid")); }
    let sec=Zeroizing::new(format!("{};{nonce};01",p.secret_id.expose_to_backend()));
    let key=derived_key(p,nonce);let cipher=aes_encrypt(input,&key)?;
    let json=JsonDoc(serde_json::json!({"sec":STANDARD.encode(sec.as_bytes()),"body":STANDARD.encode(&*cipher)}));
    serde_json::to_vec(&json.0).map(Zeroizing::new).map_err(|_|bad())
}
pub(super) fn decrypt_body(doc:&mut JsonDoc,protection:Option<&BodyProtection>)->Result<(),Error> {
    let Some(p)=protection else { return Ok(()); };
    let Some(body)=doc.0.get("body") else { return Ok(()); };
    if body.is_null() || body.as_str()==Some("") { return Ok(()); }
    let body=body.as_str().ok_or_else(bad)?;
    let sec=doc.0.get("sec").and_then(Value::as_str).ok_or_else(bad)?;
    let decoded=decode64(sec)?;let text=std::str::from_utf8(&decoded).map_err(|_|bad())?;
    let parts:Vec<&str>=text.split(';').collect();
    if parts.len()!=3 || parts[1].is_empty() || parts[1].len()>128 || parts[1].chars().any(char::is_control) { return Err(bad()); }
    let key=derived_key(p,parts[1]);let bytes=aes_decrypt(body,&key)?;
    let text=std::str::from_utf8(&bytes).map_err(|_|bad())?;
    let value=if (text.starts_with('{') && text.ends_with('}')) || (text.starts_with('[') && text.ends_with(']')) {
        serde_json::from_str(text).map_err(|_|bad())?
    } else { Value::String(text.to_owned()) };
    if let Some(old)=doc.0.get_mut("body") { wipe(old);*old=value; } Ok(())
}
pub(super) fn sip_password(input:&Secret,config:&HttpDeployment)->Result<Secret,Error> {
    let mut bytes=decode64(input.expose_to_backend())?;
    if bytes.is_empty() || bytes.len()%8!=0 { return Err(bad()); }
    let cipher=des::TdesEde3::new_from_slice(config.sip_password_key.expose_to_backend().as_bytes()).map_err(|_|bad())?;
    for block in bytes.chunks_exact_mut(8) { cipher.decrypt_block(GenericArray::from_mut_slice(block)); }
    let clear=unpad(bytes,8)?;let value=std::str::from_utf8(&clear).map_err(|_|bad())?;
    if value.is_empty() || value.len()>16_384 || value.chars().any(char::is_control) { return Err(bad()); }
    Ok(Secret::from_owned(value.to_owned()))
}

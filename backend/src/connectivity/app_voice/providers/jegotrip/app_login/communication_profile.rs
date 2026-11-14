//! getProfile is a bounded first-list entry, not getCallStatus or a SDK grant.
use super::Error;
use serde::{Deserialize,Serialize};
#[derive(Clone,Debug,Default,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub struct CommunicationProfile{
    pub allow_mo_voice:Option<i32>,pub allow_mt_voice:Option<i32>,pub mo_voice_limit:Option<i32>,pub mt_voice_limit:Option<i32>,
    pub forbidden_countries:Vec<ProfileCountry>,pub answer_forbidden_countries:Vec<ProfileCountry>,
}
#[derive(Clone,Debug,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileCountry{pub country_code:String,pub region_id:Option<String>}
pub(super) fn valid_region(value:&str)->bool{
    !value.is_empty()&&((value.len()<=6&&value.bytes().all(|b|b.is_ascii_digit()))||(value.len()<=3&&value.bytes().all(|b|b.is_ascii_uppercase())))
}
fn integer(value:&serde_json::Value)->Option<i32>{
    if let Some(n)=value.as_i64(){return i32::try_from(n).ok();}
    let text=value.as_str()?;if text.is_empty()||text.len()>11{return None;}
    let n=text.parse::<i32>().ok()?;(n.to_string()==text).then_some(n)
}
fn countries(body:&serde_json::Map<String,serde_json::Value>,key:&str)->Result<Vec<ProfileCountry>,Error>{
    let Some(value)=body.get(key)else{return Ok(Vec::new());};
    let rows=value.as_array().ok_or(Error::new("app_login_response_invalid"))?;
    if rows.len()>256{return Err(Error::new("app_login_response_invalid"));}
    let mut result=Vec::with_capacity(rows.len());
    for row in rows{
        let row=row.as_object().ok_or(Error::new("app_login_response_invalid"))?;
        let code=row.get("country_code").and_then(|v|v.as_str()).ok_or(Error::new("app_login_response_invalid"))?;
        if !valid_region(code){return Err(Error::new("app_login_response_invalid"));}
        let region=match row.get("region_id"){None=>None,Some(v)=>Some(v.as_i64().filter(|n|*n>=0).ok_or(Error::new("app_login_response_invalid"))?)};
        // Never reflect arbitrary country_cname/server text; the typed code
        // and region ID are sufficient for local country-table display.
            result.push(ProfileCountry{country_code:code.to_owned(),region_id:region.map(|value|value.to_string())});
    }Ok(result)
}
impl CommunicationProfile{
    pub(super) fn from_body(body:&serde_json::Map<String,serde_json::Value>)->Result<Self,Error>{
        let mut result=Self::default();
        for (key,slot) in [("allow_mo_voice",&mut result.allow_mo_voice),("allow_mt_voice",&mut result.allow_mt_voice),("mo_voice_limit",&mut result.mo_voice_limit),("mt_voice_limit",&mut result.mt_voice_limit)]{
            if let Some(v)=body.get(key){*slot=Some(integer(v).ok_or(Error::new("app_login_response_invalid"))?);}
        }
        result.forbidden_countries=countries(body,"forbidden_country")?;result.answer_forbidden_countries=countries(body,"answer_forbidden_country")?;Ok(result)
    }
    pub(super) fn valid(&self)->bool{
        [&self.forbidden_countries,&self.answer_forbidden_countries].into_iter().all(|rows|rows.len()<=256&&rows.iter().all(|r|valid_region(&r.country_code)&&r.region_id.as_deref().is_none_or(|n|n.parse::<i64>().ok().is_some_and(|value|value>=0&&value.to_string()==n))))
    }
}

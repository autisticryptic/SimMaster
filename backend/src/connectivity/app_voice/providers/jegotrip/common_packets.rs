//! Native Common call/control packets AFTER ordered NetStream records.
//! D70C9C fragments the (possibly compressed) payload; DDAA3C reassembles it
//! BEFORE DDA530/D76E40 decompression. The old strict mode3 codec is unchanged.
use super::{common_compression::{self,CompressionError},rpc_stream::MAX_STREAM_BYTES,
    transport_packets::{CallKind,TransportBytes}};
use std::{fmt,time::{Duration,Instant}};
use zeroize::{Zeroize,Zeroizing};

pub(super) const NATIVE_FRAGMENT_THRESHOLD:usize=65_536;
const MAX_PACKETS:usize=256;
const MAX_GROUP_BYTES:usize=1024*1024;
const FRAGMENTED:u8=0x20;
const COMPRESSED:u8=0x10;
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(super) enum CommonError{Configuration,Bound,Packet,Fragment,Compression(CompressionError),Timeout,ClockRollback,Closed}
impl From<CompressionError> for CommonError{fn from(error:CompressionError)->Self{Self::Compression(error)}}
pub(super) struct CommonMessage{pub kind:CallKind,pub payload:TransportBytes,pub deadline:Instant}
impl fmt::Debug for CommonMessage{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("CommonMessage(<redacted>)")}}
fn kind(header:u8)->Result<CallKind,CommonError>{match header&15{
    0=>Ok(CallKind::Request),1=>Ok(CallKind::Reply),2=>Ok(CallKind::Ping),4=>Ok(CallKind::Release),14=>Ok(CallKind::Pong),_=>Err(CommonError::Packet),
}}
fn validate_payload(kind:CallKind,payload:&[u8])->Result<(),CommonError>{
    if payload.len()>MAX_STREAM_BYTES{return Err(CommonError::Bound);}
    match kind{
        CallKind::Request|CallKind::Reply if payload.is_empty()=>Err(CommonError::Packet),
        CallKind::Ping|CallKind::Pong|CallKind::Release if !payload.is_empty()=>Err(CommonError::Packet),
        _=>Ok(()),
    }
}

/// An explicit outgoing compression threshold is optional; absence means do
///not compress, not a guessed SDK/global default. Native compression falls back
///unless the level1 container fits strictly inside3/4 of the original payload.
pub(super) fn encode(kind:CallKind,payload:&[u8],threshold:usize,compress_above:Option<usize>)->Result<Vec<TransportBytes>,CommonError>{
    if !(8..=NATIVE_FRAGMENT_THRESHOLD).contains(&threshold)||compress_above.is_some_and(|value|value==0||value>MAX_STREAM_BYTES){return Err(CommonError::Configuration);}
    validate_payload(kind,payload)?;
    let compressed=if matches!(kind,CallKind::Request|CallKind::Reply)&&compress_above.is_some_and(|minimum|payload.len()>minimum){common_compression::try_compress(payload)?}else{None};
    let mut header=kind as u8;if compressed.is_some(){header|=COMPRESSED;}
    let body=compressed.as_ref().map_or(payload,|bytes|bytes.expose_to_transport());
    if body.len()<=threshold{
        let mut bytes=Vec::with_capacity(body.len()+1);bytes.push(header);bytes.extend_from_slice(body);return Ok(vec![TransportBytes::from_bytes(bytes)]);
    }
    let mut full=Zeroizing::new(Vec::with_capacity(body.len()+4));full.extend_from_slice(&(body.len() as u32).to_be_bytes());full.extend_from_slice(body);
    let mut rest=full.as_slice();let mut first=true;let mut packets=Vec::new();
    while !rest.is_empty(){
        if packets.len()>=MAX_PACKETS{return Err(CommonError::Bound);}
        let count=if rest.len()>=2*threshold{threshold}else if rest.len()>threshold{rest.len()/2}else{rest.len()};
        let mut packet=Vec::with_capacity(count+1);packet.push(if first{header|FRAGMENTED}else{3});packet.extend_from_slice(&rest[..count]);
        packets.push(TransportBytes::from_bytes(packet));rest=&rest[count..];first=false;
    }Ok(packets)
}
struct Partial{kind:CallKind,compressed:bool,expected:usize,bytes:Vec<u8>,deadline:Instant}
impl Drop for Partial{fn drop(&mut self){self.bytes.zeroize();}}
pub(super) struct CommonAssembler{partial:Option<Partial>,timeout:Duration,last_now:Instant,closed:bool}
impl fmt::Debug for CommonAssembler{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.debug_struct("CommonAssembler").field("closed",&self.closed).field("partial",&self.partial.is_some()).field("data",&"<redacted>").finish()}}
impl Drop for CommonAssembler{fn drop(&mut self){self.close();}}
impl CommonAssembler{
    pub fn new(timeout:Duration,now:Instant)->Result<Self,CommonError>{
        if timeout.is_zero()||timeout>Duration::from_secs(30){return Err(CommonError::Configuration);}
        Ok(Self{partial:None,timeout,last_now:now,closed:false})
    }
    pub fn close(&mut self){self.closed=true;self.partial=None;}
    pub fn deadline(&self)->Option<Instant>{self.partial.as_ref().map(|part|part.deadline)}
    pub fn retained_bytes(&self)->usize{self.partial.as_ref().map_or(0,|part|part.bytes.len())}
    pub fn tick(&mut self,now:Instant)->Result<(),CommonError>{
        let result=if self.closed{Err(CommonError::Closed)}else if now<self.last_now{Err(CommonError::ClockRollback)}
            else if self.deadline().is_some_and(|end|now>=end){Err(CommonError::Timeout)}else{self.last_now=now;Ok(())};
        if result.is_err(){self.close();}result
    }
    fn complete(kind:CallKind,compressed:bool,bytes:TransportBytes,deadline:Instant)->Result<CommonMessage,CommonError>{
        let payload=if compressed{common_compression::decompress(bytes.expose_to_transport())?}else{bytes};
        validate_payload(kind,payload.expose_to_transport())?;Ok(CommonMessage{kind,payload,deadline})
    }
    pub fn on_packet(&mut self,packet:&[u8],caller_end:Instant,now:Instant)->Result<Option<CommonMessage>,CommonError>{
        let result=self.consume(packet,caller_end,now);if result.is_err(){self.close();}result
    }
    fn consume(&mut self,packet:&[u8],caller_end:Instant,now:Instant)->Result<Option<CommonMessage>,CommonError>{
        self.tick(now)?;if now>=caller_end{return Err(CommonError::Timeout);}
        if packet.len()>MAX_STREAM_BYTES+1{return Err(CommonError::Bound);}
        let(&header,body)=packet.split_first().ok_or(CommonError::Packet)?;
        if header&0xc0!=0{return Err(CommonError::Packet);}
        if header==3{
            let part=self.partial.as_mut().ok_or(CommonError::Fragment)?;
            // A shorter current caller budget may tighten, never lengthen,
            //the original captured fragment lifetime.
            part.deadline=part.deadline.min(caller_end);
            if body.is_empty()||body.len()>part.expected-part.bytes.len(){return Err(CommonError::Fragment);}
            part.bytes.extend_from_slice(body);
            if part.bytes.len()!=part.expected{return Ok(None);}
            let mut part=self.partial.take().ok_or(CommonError::Fragment)?;
            return Self::complete(part.kind,part.compressed,TransportBytes::from_bytes(std::mem::take(&mut part.bytes)),part.deadline).map(Some);
        }
        // Never silently discard/replace an original partial when a new call
        //or control interrupts it. Only the encoder's exact continuation3 is
        //accepted; flags on a continuation cannot alter captured compression.
        if self.partial.is_some(){return Err(CommonError::Fragment);}
        let kind=kind(header)?;let compressed=header&COMPRESSED!=0;
        if (compressed||header&FRAGMENTED!=0)&&!matches!(kind,CallKind::Request|CallKind::Reply){return Err(CommonError::Packet);}
        if header&FRAGMENTED==0{return Self::complete(kind,compressed,TransportBytes::from_bytes(body.to_vec()),caller_end).map(Some);}
        let prefix=body.get(..4).ok_or(CommonError::Fragment)?;
        let expected=u32::from_be_bytes(prefix.try_into().map_err(|_|CommonError::Fragment)?) as usize;
        if expected==0||expected>MAX_STREAM_BYTES{return Err(CommonError::Bound);}
        let initial=&body[4..];if initial.is_empty()||initial.len()>=expected{return Err(CommonError::Fragment);}
        let deadline=caller_end.min(now.checked_add(self.timeout).ok_or(CommonError::Configuration)?);
        let mut bytes=Vec::with_capacity(expected);bytes.extend_from_slice(initial);
        self.partial=Some(Partial{kind,compressed,expected,bytes,deadline});Ok(None)
    }
    pub fn on_group(&mut self,packets:&[TransportBytes],caller_end:Instant,now:Instant)->Result<Vec<CommonMessage>,CommonError>{
        self.tick(now)?;if now>=caller_end{self.close();return Err(CommonError::Timeout);}
        let input_bytes=packets.iter().try_fold(0usize,|total,packet|total.checked_add(packet.expose_to_transport().len()));
        if packets.len()>MAX_PACKETS||input_bytes.is_none_or(|bytes|bytes>MAX_GROUP_BYTES){
            self.close();return Err(CommonError::Bound);
        }
        let mut messages=Vec::new();let mut expanded=0usize;
        for packet in packets{
            if let Some(message)=self.on_packet(packet.expose_to_transport(),caller_end,now)?{
                let Some(total)=expanded.checked_add(message.payload.expose_to_transport().len()) else{self.close();return Err(CommonError::Bound);};
                expanded=total;messages.push(message);
            }
            if expanded>MAX_GROUP_BYTES.saturating_sub(self.retained_bytes()){self.close();return Err(CommonError::Bound);}
        }
        Ok(messages)
    }
    pub fn finish_eof(&mut self)->Result<(),CommonError>{
        if self.closed{return Err(CommonError::Closed);}let partial=self.partial.is_some();self.close();if partial{Err(CommonError::Fragment)}else{Ok(())}
    }
}

#[cfg(test)]
#[path="common_packets_tests.rs"]
mod tests;

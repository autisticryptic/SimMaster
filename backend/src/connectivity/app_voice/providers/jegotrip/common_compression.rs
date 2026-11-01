//! Bounded native Common bzip2 containers (D70E44/D76E40), not SDP zlib.
//! BE32 is the uncompressed payload length. Decode only AFTER complete Common
//! reassembly under the original scope. No concatenated/trailing streams.
use super::{rpc_stream::MAX_STREAM_BYTES,transport_packets::TransportBytes};
use bzip2::{Action,Compress,Compression,Decompress,Status};
use zeroize::Zeroizing;

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(super) enum CompressionError{TooLarge,Length,Stream}

/// Native compression uses level1/work-factor0 and a total output budget of
///floor(3*input/4), including the four-byte prefix. Failure to fit falls back
///to the ORIGINAL uncompressed payload; it never sends a truncated stream.
pub(super) fn try_compress(payload:&[u8])->Result<Option<TransportBytes>,CompressionError>{
    if payload.is_empty(){return Err(CompressionError::Length);}
    if payload.len()>MAX_STREAM_BYTES{return Err(CompressionError::TooLarge);}
    let budget=payload.len()*3/4;
    if budget<=4{return Ok(None);}
    let mut out=Zeroizing::new(vec![0u8;budget]);out[..4].copy_from_slice(&(payload.len() as u32).to_be_bytes());
    let mut compressor=Compress::new(Compression::fast(),0);
    let status=compressor.compress(payload,&mut out[4..],Action::Finish).map_err(|_|CompressionError::Stream)?;
    let written=compressor.total_out() as usize;
    if status!=Status::StreamEnd||compressor.total_in()!=payload.len() as u64||written>=budget-4{return Ok(None);}
    out.truncate(4+written);Ok(Some(TransportBytes::from_bytes(std::mem::take(&mut *out))))
}

pub(super) fn uncompressed_length(container:&[u8])->Result<usize,CompressionError>{
    if container.len()>MAX_STREAM_BYTES{return Err(CompressionError::TooLarge);}
    let prefix=container.get(..4).ok_or(CompressionError::Length)?;
    let expected=u32::from_be_bytes(prefix.try_into().map_err(|_|CompressionError::Length)?) as usize;
    if expected==0{return Err(CompressionError::Length);}
    if expected>MAX_STREAM_BYTES{return Err(CompressionError::TooLarge);}
    Ok(expected)
}
pub(super) fn decompress(container:&[u8])->Result<TransportBytes,CompressionError>{
    let expected=uncompressed_length(container)?;
    let input=&container[4..];
    if input.get(..3)!=Some(b"BZh".as_slice())||!input.get(3).is_some_and(|level|(b'1'..=b'9').contains(level)){return Err(CompressionError::Stream);}
    // Exactly the bounded declared output is allocated. One separate stack
    //byte detects excess output while consuming the final terminator/CRC.
    let mut out=Zeroizing::new(vec![0u8;expected]);let mut overflow=Zeroizing::new([0u8;1]);
    let mut decoder=Decompress::new(true); // bounded small-memory native algorithm
    let mut consumed=0usize;let mut produced=0usize;
    for _ in 0..4{
        let old=(consumed,produced);
        let destination=if produced==expected{overflow.as_mut_slice()}else{&mut out[produced..]};
        let status=decoder.decompress(&input[consumed..],destination).map_err(|_|CompressionError::Stream)?;
        consumed=usize::try_from(decoder.total_in()).map_err(|_|CompressionError::TooLarge)?;
        produced=usize::try_from(decoder.total_out()).map_err(|_|CompressionError::TooLarge)?;
        if consumed>input.len()||produced>expected{return Err(CompressionError::Length);}
        if status==Status::StreamEnd{
            if consumed!=input.len()||produced!=expected{return Err(CompressionError::Length);}
            out.truncate(expected);return Ok(TransportBytes::from_bytes(std::mem::take(&mut *out)));
        }
        if status!=Status::Ok||(consumed,produced)==old{return Err(CompressionError::Stream);}
    }
    Err(CompressionError::Stream)
}

#[cfg(test)]
#[path="common_compression_tests.rs"]
pub(super) mod tests;

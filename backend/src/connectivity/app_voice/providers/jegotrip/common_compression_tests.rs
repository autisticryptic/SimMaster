//! Independent reference compression uses the pre-existing Actions Python3
//! standard-library libbzip2, never local protocol/test execution.
use super::*;
use std::{io::Write,process::{Command,Stdio}};

fn reference(operation:&str,level:u8,input:&[u8])->Vec<u8>{
 let script="import bz2,sys; d=sys.stdin.buffer.read(); sys.stdout.buffer.write(bz2.compress(d,compresslevel=int(sys.argv[2])) if sys.argv[1]=='compress' else bz2.decompress(d))";
 let mut child=Command::new("python3").args(["-c",script,operation,&level.to_string()]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
 {let mut stdin=child.stdin.take().unwrap();stdin.write_all(input).unwrap();}
 let output=child.wait_with_output().unwrap();assert!(output.status.success(),"independent bzip2 reference failed");assert!(output.stdout.len()<=MAX_STREAM_BYTES+2048);output.stdout
}
pub(crate) fn reference_container(payload:&[u8],level:u8)->Vec<u8>{
 let mut bytes=(payload.len() as u32).to_be_bytes().to_vec();bytes.extend_from_slice(&reference("compress",level,payload));bytes
}

#[test]
fn independent_native_level_one_binary_container_decodes_exact_be32_length(){
 let plain=[0,1,0xff,0xfe,0,42,13,10,b'x'];let bytes=reference_container(&plain,1);assert_eq!(&bytes[..4],&9u32.to_be_bytes());assert_eq!(&bytes[4..8],b"BZh1");
 assert_eq!(decompress(&bytes).unwrap().expose_to_transport(),plain);
}
#[test]
fn every_valid_block_size_is_bounded_without_assuming_outgoing_compression_policy(){
 for level in 1..=9{let bytes=reference_container(b"bounded native Common data",level);assert_eq!(decompress(&bytes).unwrap().expose_to_transport(),b"bounded native Common data");}
}
#[test]
fn zero_oversized_or_truncated_length_is_rejected_before_decompressor_allocation(){
 for bytes in [vec![],vec![0],vec![0,0,0],vec![0,0,0,0]]{assert_eq!(decompress(&bytes).err(),Some(CompressionError::Length));}
 let mut bytes=((MAX_STREAM_BYTES+1) as u32).to_be_bytes().to_vec();bytes.extend_from_slice(b"BZh1");assert_eq!(decompress(&bytes).err(),Some(CompressionError::TooLarge));
}
#[test]
fn declared_output_size_cannot_be_larger_or_smaller_than_actual_data(){
 let plain=b"size must be exact";let bytes=reference_container(plain,1);
 for size in [plain.len()-1,plain.len()+1]{let mut bad=bytes.clone();bad[..4].copy_from_slice(&(size as u32).to_be_bytes());assert!(decompress(&bad).is_err());}
}
#[test]
fn truncated_bzip2_header_data_or_crc_never_publishes_partial_plaintext(){
 let bytes=reference_container(b"a complete native stream is required",1);
 for cut in 4..bytes.len(){assert!(decompress(&bytes[..cut]).is_err(),"truncated stream accepted at {cut}");}
}
#[test]
fn invalid_crc_is_not_accepted_even_when_announced_output_length_matches(){
 let mut bytes=reference_container(b"do not ignore the stream checksum",1);let position=bytes.len()-4;bytes[position]^=0x80;
 assert!(decompress(&bytes).is_err());
}
#[test]
fn trailing_bytes_and_concatenated_streams_are_not_silently_ignored(){
 let first=reference_container(b"first",1);
 for tail in [vec![0],reference("compress",1,b"second")]{let mut bytes=first.clone();bytes.extend_from_slice(&tail);assert!(decompress(&bytes).is_err());}
}
#[test]
fn native_compression_uses_level_one_and_prefix_inside_strict_three_quarter_budget(){
 let data=vec![b'x';4096];let compressed=try_compress(&data).unwrap().unwrap();let bytes=compressed.expose_to_transport();
 assert_eq!(&bytes[..4],&4096u32.to_be_bytes());assert_eq!(&bytes[4..8],b"BZh1");assert!(bytes.len()<data.len()*3/4);
 assert_eq!(reference("decompress",1,&bytes[4..]),data);
}
#[test]
fn incompressible_input_falls_back_without_returning_a_truncated_container(){
 let mut state=0x12345678u32;let data:Vec<_>=(0..65536).map(|_|{state^=state<<13;state^=state>>17;state^=state<<5;state as u8}).collect();
 assert!(try_compress(&data).unwrap().is_none());
}
#[test]
fn too_small_compression_budget_retains_original_uncompressed_data(){
 for size in 1..=6{assert!(try_compress(&vec![b'x';size]).unwrap().is_none());}
 assert_eq!(try_compress(&[]).err(),Some(CompressionError::Length));
}
#[test]
fn maximum_plaintext_is_exact_and_larger_input_is_rejected_without_large_allocations(){
 let data=vec![b'a';MAX_STREAM_BYTES];let independent=reference_container(&data,1);assert_eq!(decompress(&independent).unwrap().expose_to_transport(),data);
 let encoded=try_compress(&data).unwrap().unwrap();assert_eq!(reference("decompress",1,&encoded.expose_to_transport()[4..]),data);
 assert_eq!(try_compress(&vec![0;MAX_STREAM_BYTES+1]).err(),Some(CompressionError::TooLarge));
 assert_eq!(decompress(&vec![0;MAX_STREAM_BYTES+1]).err(),Some(CompressionError::TooLarge));
}
#[test]
fn invalid_magic_or_block_size_cannot_select_another_compression_format(){
 let base=reference_container(b"not zlib, gzip or a raw stream",1);
 for header in [*b"BZh0",*b"BZh:",*b"BZx1",[0x78,0x9c,0,0]]{let mut bytes=base.clone();bytes[4..8].copy_from_slice(&header);assert_eq!(decompress(&bytes).err(),Some(CompressionError::Stream));}
}
#[test]
fn separate_decoder_instances_do_not_carry_previous_stream_state(){
 let first=reference_container(b"first payload",1);let second=reference_container(b"second payload",9);
 assert_eq!(decompress(&first).unwrap().expose_to_transport(),b"first payload");assert_eq!(decompress(&second).unwrap().expose_to_transport(),b"second payload");
 let mut joined=first;joined.extend_from_slice(&second[4..]);assert!(decompress(&joined).is_err());
}
#[test]
fn decompressed_transport_debug_does_not_disclose_private_call_bytes(){
 let bytes=reference_container(b"fixture-private-call-material",1);let result=decompress(&bytes).unwrap();assert!(!format!("{result:?}").contains("private-call-material"));
}

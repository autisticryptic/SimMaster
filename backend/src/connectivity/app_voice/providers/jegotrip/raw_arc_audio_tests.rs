//! Actions-only paired native raw-ARC audio framing tests, no socket/SDK.
use super::*;
fn rtp()->Vec<u8>{vec![0x80,0xe0,0,9,0,0,0,10,0x12,0x34,0x56,0x78,0x80,0,0xff]}
#[test]
fn native_channel_zero_prefix_preserves_every_rtp_marker_header_and_payload_byte(){
    let payload=rtp();let wire=encode_audio(&payload).unwrap();let mut oracle=vec![0];oracle.extend_from_slice(&payload);
    assert_eq!(wire.expose_to_transport(),oracle);assert_eq!(decode_audio(&oracle).unwrap(),Some(payload.as_slice()));
    assert_eq!(audio_kind(&payload).unwrap(),AudioPacketKind::Rtp);
}
#[test]
fn rtcp_compound_audio_payload_is_one_unchanged_channel_zero_datagram(){
    // SR with zero report blocks is 28 bytes; RR with zero blocks is 8.
    let mut payload=vec![0x80,200,0,6];payload.extend_from_slice(&[0;24]);
    payload.extend_from_slice(&[0x80,201,0,1,0x12,0x34,0x56,0x78]);
    let wire=encode_audio(&payload).unwrap();assert_eq!(wire.expose_to_transport()[0],0);assert_eq!(&wire.expose_to_transport()[1..],payload.as_slice());
    assert_eq!(decode_audio(wire.expose_to_transport()).unwrap(),Some(payload.as_slice()));assert_eq!(audio_kind(&payload).unwrap(),AudioPacketKind::Rtcp);
}
#[test]
fn empty_truncated_and_other_media_channels_never_become_audio(){
    assert_eq!(decode_audio(&[]),Err(RawMediaError::Truncated));assert_eq!(decode_audio(&[0]),Err(RawMediaError::Truncated));
    for channel in [1,2,3,6,255]{let mut other=vec![channel];other.extend_from_slice(&rtp());assert_eq!(decode_audio(&other).unwrap(),None);}
    assert!(encode_audio(&[]).is_err());assert!(decode_audio(&[0,0x80,0,0]).is_err());
}
#[test]
fn rpc_tls_and_raw_rse_endpoint_parameters_are_not_audio_packet_formats(){
    for other in [&[0,0,1,0,0][..],b"connect.Endpoint.Endpoint",b"RseEx.SDP",&[0xff,0xff,6,22,3,3,0,1,0]]{
        assert!(encode_audio(other).is_err());let mut prefixed=vec![0];prefixed.extend_from_slice(other);assert!(decode_audio(&prefixed).is_err());
    }
}
#[test]
fn malformed_rtp_version_csrc_and_rtcp_lengths_fail_without_slicing_panics(){
    let mut packet=rtp();packet[0]=0x40;assert!(audio_kind(&packet).is_err());packet[0]=0x8f;assert!(audio_kind(&packet).is_err());
    for bad in [&[0x80,200,0,2,1,2,3,4][..],&[0x80,200,0,1,1,2,3,4,0],&[0x80,200,0,1,1,2,3,4,0x40,201,0,0]]{assert!(audio_kind(bad).is_err());}
}
#[test]
fn raw_media_size_limit_counts_native_prefix_and_is_not_tls_fragment_threshold(){
    let mut payload=rtp();payload.resize(MAX_RAW_MEDIA_BYTES-1,0);let wire=encode_audio(&payload).unwrap();assert_eq!(wire.expose_to_transport().len(),MAX_RAW_MEDIA_BYTES);
    assert!(decode_audio(wire.expose_to_transport()).unwrap().is_some());payload.push(0);assert!(matches!(encode_audio(&payload),Err(RawMediaError::ResourceLimit)));
    assert_eq!(decode_audio(&vec![0;MAX_RAW_MEDIA_BYTES+1]),Err(RawMediaError::ResourceLimit));
}
#[test]
fn raw_media_wrapper_debug_never_contains_audio_header_or_payload_bytes(){
    let wire=encode_audio(&rtp()).unwrap();assert_eq!(format!("{wire:?}"),"RawMediaDatagram(<redacted>)");
}
#[test]
fn all_small_datagrams_are_bounded_and_unknown_channels_do_not_poison_later_audio(){
    for size in 0..20{let mut bytes=vec![0;size];for first in [0,1,255]{if let Some(byte)=bytes.first_mut(){*byte=first;}let _=decode_audio(&bytes);}}
    let mut video=vec![1];video.extend_from_slice(&rtp());assert_eq!(decode_audio(&video).unwrap(),None);
    let audio=encode_audio(&rtp()).unwrap();assert!(decode_audio(audio.expose_to_transport()).unwrap().is_some());
}
#[test]
fn complete_rtcp_report_bodies_and_report_counts_are_required_not_just_length_words(){
    for bad in [vec![0x80,200,0,0],vec![0x80,200,0,1,0,0,0,0],vec![0x81,201,0,1,0,0,0,0],vec![0x80,203,0,0]]{
        assert!(audio_kind(&bad).is_err());assert!(encode_audio(&bad).is_err());
    }
    let rr=[0x80,201,0,1,1,2,3,4];assert_eq!(audio_kind(&rr),Ok(AudioPacketKind::Rtcp));
    let mut with_report=vec![0x81,201,0,7];with_report.extend_from_slice(&[0;28]);
    assert_eq!(audio_kind(&with_report),Ok(AudioPacketKind::Rtcp));
}
#[test]
fn rtcp_padding_only_on_final_packet_and_sdes_end_alignment_are_checked(){
    let sdes=[0x81,202,0,2,1,2,3,4,1,1,b'x',0];
    assert_eq!(audio_kind(&sdes),Ok(AudioPacketKind::Rtcp));
    let mut missing_end=sdes;missing_end[11]=1;assert!(audio_kind(&missing_end).is_err());
    let mut padded=vec![0xa0,201,0,2,1,2,3,4,0,0,0,4];
    assert_eq!(audio_kind(&padded),Ok(AudioPacketKind::Rtcp));
    padded.extend_from_slice(&[0x80,201,0,1,1,2,3,4]);assert!(audio_kind(&padded).is_err());
    for count in [0,1,12,255]{let bytes=[0xa0,201,0,2,1,2,3,4,0,0,0,count];assert!(audio_kind(&bytes).is_err());}
}
#[test]
fn rtp_padding_and_mux_payload_type_collisions_cannot_be_silently_relayed(){
    let mut payload=rtp();payload[0]|=0x20;
    for count in [0,4,255]{*payload.last_mut().unwrap()=count;assert!(audio_kind(&payload).is_err());}
    *payload.last_mut().unwrap()=1;assert_eq!(audio_kind(&payload),Ok(AudioPacketKind::Rtp));
    for payload_type in 64..=95{for marker in [0,0x80]{let mut bytes=rtp();bytes[1]=marker|payload_type;assert!(audio_kind(&bytes).is_err());}}
}

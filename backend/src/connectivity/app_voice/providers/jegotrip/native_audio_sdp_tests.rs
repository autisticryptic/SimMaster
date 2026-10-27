//! Actions-only strict native/trunk raw and zlib+Base64 SDP oracles. No sockets
//! or service readiness; fixed wire fixture is standard zlib level 6 output.
use super::*;
use super::super::{media_test_support::{accepted_context, CONTENT, ENDPOINT}, rpc_calls::{INVITED_METHOD, NativeCallId},
    rpc_envelope::RpcRequest, rpc_stream::{RpcProperty, Writer}, call_binding::CallIncomingBinding};
const SDP: &[u8] = b"v=0\r\no=fixture 10 11 IN IP4 127.0.0.1\r\ns=fixture\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio 40000 RTP/AVP 0 8\r\na=rtpmap:0 PCMU/8000\r\na=rtpmap:8 PCMA/8000\r\na=sendrecv\r\na=rtcp-mux\r\n";
const COMPRESSED_SDP: &[u8] = b"eJwrszXg5cq3TcusKCktSlUwNFAwNFTw9FPwDDBRMDQy1zMAQkNermKYCl6uZFtM6RJbAwWgObm2iaUpmfkKJgZAoBAUEqDvGBagYKBgwcuVaFtUUpCbWGBloBDg7BuqbwFUgSRqARJ1hIsWp+alFKUml0FUJBfo5pZW8HIBAMNyLRg=";
const URL_SAFE_COMPRESSED_SDP: &[u8] = b"eJwrszXg5cq3TcusKCktSlUwNFAwNFTw9FPwDDBRMDQy1zMAQkNermKYCl6uZFtM6RJbAwWgObm2iaUpmfkKJgZAoBAUEqDvGBagYKBgwcuVaFtUUpCbWGBloBDg7BuqbwFUgSRqARJ1hIsWp-alFKUml0FUJBfo5pZW8HIBAMNyLRg=";
fn inflate_wire(body: &[u8]) -> Vec<u8> {
    let compressed = STANDARD.decode(body).unwrap(); let mut decoder = ZlibDecoder::new(compressed.as_slice()); let mut plain = Vec::new();
    decoder.read_to_end(&mut plain).unwrap(); assert_eq!(decoder.total_in() as usize, compressed.len()); plain
}
fn notice(rpc: &super::super::arc_session::ArcRpcSession, ticket: &CallControlTicket, sdp: &[u8]) -> OwnedCallNotice {
    let dispatcher = rpc.incoming_call_dispatcher(CallIncomingBinding { object_id: b"#fixture-call-client", domain: b"fixture-domain" }).unwrap();
    let mut data = Writer::versioned().unwrap(); data.u64(ticket.native_id().expose_to_backend()).unwrap(); data.bytes(b"fixture-peer").unwrap();
    data.properties(&[RpcProperty { key: b"RseEx.SDP", value: sdp }]).unwrap(); let data = data.finish();
    let request = RpcRequest { call_id: 99, method: INVITED_METHOD.as_bytes(), arguments: data.expose_to_transport(), metadata: vec![
        RpcProperty { key: b"id", value: b"#fixture-call-client" }, RpcProperty { key: b"domain", value: b"fixture-domain" }] };
    dispatcher.on_direct_request(ticket.owner(), &ticket.connection_scope(), &request).unwrap().unwrap()
}
#[test]
fn original_native_rse_sdp_only_is_accepted_without_using_its_network_address_as_sdk_authority() {
    let (rpc, _, _, ticket, _) = accepted_context(ENDPOINT, CONTENT); let notice = notice(&rpc, &ticket, SDP);
    let offer = PlainAudioOffer::native(&notice, &ticket, true).unwrap();
    assert_eq!(offer.endpoint().unwrap(), "127.0.0.1:40000".parse().unwrap());
    assert_eq!(offer.description.codecs.len(), 2);
    assert_eq!(offer.description.codecs[0].payload_type, 0); assert_eq!(offer.description.codecs[1].payload_type, 8);
    let answer = PlainAudioOffer::trunk(SDP, true).unwrap(); let raw = offer.native_answer(&answer, &ticket, "127.0.0.1:49151".parse().unwrap()).unwrap();
    assert!(raw.starts_with(b"v=0\r\n"), "raw invitation keeps raw native reply");
    assert_eq!(ticket.native_id(), NativeCallId::from_bits(super::super::media_test_support::NATIVE_ID));
}
#[test]
fn native_zlib_base64_invitation_is_bounded_inflated_and_answer_mirrors_standard_padded_wire() {
    let (rpc, _, _, ticket, _) = accepted_context(ENDPOINT, CONTENT); let notice = notice(&rpc, &ticket, COMPRESSED_SDP);
    let offer = PlainAudioOffer::native(&notice, &ticket, true).unwrap();
    assert_eq!(offer.endpoint().unwrap(), "127.0.0.1:40000".parse().unwrap());
    let local: SocketAddr = "127.0.0.1:49152".parse().unwrap();
    let sip = offer.with_local_endpoint(&ticket, local).unwrap(); assert!(sip.starts_with(b"v=0\r\n"), "Trunk/UI see plain SDP");
    let answer = PlainAudioOffer::trunk(&sip, true).unwrap();
    let native = offer.native_answer(&answer, &ticket, local).unwrap();
    assert!(!native.starts_with(b"v=0")); assert_eq!(native.len() % 4, 0); assert!(!native.iter().any(|byte| matches!(byte, b'-' | b'_')));
    let decoded = inflate_wire(&native); let text = std::str::from_utf8(&decoded).unwrap();
    assert!(text.contains("m=audio 49152 RTP/AVP 0 8")); assert!(text.contains("a=rtcp-mux\r\n"));
}
#[test]
fn native_url_safe_base64_is_accepted_but_reply_uses_original_standard_alphabet_policy() {
    let (rpc, _, _, ticket, _) = accepted_context(ENDPOINT, CONTENT); let notice = notice(&rpc, &ticket, URL_SAFE_COMPRESSED_SDP);
    let offer = PlainAudioOffer::native(&notice, &ticket, true).unwrap(); let answer = PlainAudioOffer::trunk(SDP, true).unwrap();
    let native = offer.native_answer(&answer, &ticket, "127.0.0.1:49153".parse().unwrap()).unwrap();
    assert!(!native.iter().any(|byte| matches!(byte, b'-' | b'_'))); assert!(inflate_wire(&native).starts_with(b"v=0\r\n"));
}
#[test]
fn malformed_mixed_trailing_and_expanding_compressed_sdp_fail_closed_before_call_admission() {
    let (rpc, _, _, ticket, _) = accepted_context(ENDPOINT, CONTENT);
    let mut mixed = URL_SAFE_COMPRESSED_SDP.to_vec(); let at = mixed.iter().position(|byte| *byte == b'-').unwrap(); mixed[at + 1] = b'/';
    let not_zlib = STANDARD.encode(SDP); let mut trailing = STANDARD.decode(COMPRESSED_SDP).unwrap(); trailing.push(0); let trailing = STANDARD.encode(trailing);
    for wire in [mixed, not_zlib.into_bytes(), trailing.into_bytes()] {
        let notice = notice(&rpc, &ticket, &wire); assert_eq!(PlainAudioOffer::native(&notice, &ticket, true).err(), Some(MediaError::Protocol));
    }
    let oversized_plain = vec![b'x'; MAX_SDP + 1]; let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&oversized_plain).unwrap(); let bomb = STANDARD.encode(encoder.finish().unwrap());
    let bomb_notice = notice(&rpc, &ticket, bomb.as_bytes()); assert_eq!(PlainAudioOffer::native(&bomb_notice, &ticket, true).err(), Some(MediaError::Protocol));
    let too_large = vec![b'A'; MAX_NATIVE_SDP_WIRE + (4 - MAX_NATIVE_SDP_WIRE % 4)];
    let large_notice = notice(&rpc, &ticket, &too_large); assert_eq!(PlainAudioOffer::native(&large_notice, &ticket, true).err(), Some(MediaError::Configuration));
}
#[test]
fn security_attributes_remain_rejected_after_native_decompression() {
    let (rpc, _, _, ticket, _) = accepted_context(ENDPOINT, CONTENT); let mut secure = SDP.to_vec(); secure.extend_from_slice(b"a=crypto:1 fixture\r\n");
    let raw = Zeroizing::new(secure); let wire = encode_native_sdp(raw, NativeSdpWire::ZlibBase64).unwrap(); let notice = notice(&rpc, &ticket, &wire);
    assert_eq!(PlainAudioOffer::native(&notice, &ticket, true).err(), Some(MediaError::Configuration));
}
#[test]
fn caller_supplied_other_owner_same_numeric_call_cannot_borrow_original_offer_or_restore_revoked_ticket() {
    let (rpc, _, account, ticket, _) = accepted_context(ENDPOINT, CONTENT); let notice = notice(&rpc, &ticket, SDP);
    let (_other_rpc, _, _, other_ticket, _) = accepted_context(ENDPOINT, CONTENT);
    assert_eq!(PlainAudioOffer::native(&notice, &other_ticket, true).err(), Some(MediaError::Scope));
    ticket.revoke(); assert_eq!(PlainAudioOffer::native(&notice, &ticket, true).err(), Some(MediaError::Scope)); assert!(account.is_open());
}
#[test]
fn basic_profile_must_explicitly_enable_plain_media_instead_of_downgrading_sdk_tls_or_srtp() {
    assert_eq!(PlainAudioOffer::trunk(SDP, false).err(), Some(MediaError::Configuration));
    for crypto in ["a=crypto:1 AES_CM_128_HMAC_SHA1_80 inline:fixture\r\n", "a=fingerprint:sha-256 fixture\r\n", "a=setup:active\r\n",
        "a=ice-ufrag:fixture\r\n", "a=candidate:fixture\r\n", "a=group:BUNDLE audio\r\n"] {
        let mut body = SDP.to_vec(); body.extend_from_slice(crypto.as_bytes());
        assert_eq!(PlainAudioOffer::trunk(&body, true).err(), Some(MediaError::Configuration));
    }
    let secure = String::from_utf8(SDP.to_vec()).unwrap().replace("RTP/AVP", "RTP/SAVP");
    assert_eq!(PlainAudioOffer::trunk(secure.as_bytes(), true).err(), Some(MediaError::Configuration));
}
#[test]
fn compression_rpc_bytes_invalid_utf8_nul_and_lf_only_are_not_guessed_into_plain_sdp() {
    for body in [b"fixture-compressed-base64".as_slice(), b"\0\0\x01\0\0", b"v=0\r\n\xff", b"v=0\r\n\0", b"v=0\nc=IN IP4 127.0.0.1\n"] {
        assert!(PlainAudioOffer::trunk(body, true).is_err());
    }
}
#[test]
fn only_one_sendrecv_audio_media_and_no_rtcp_mux_inference_are_allowed() {
    let text = String::from_utf8(SDP.to_vec()).unwrap();
    for body in [text.replace("a=sendrecv", "a=recvonly"), text.replace("a=sendrecv", "a=inactive"),
        text.replace("a=rtcp-mux\r\n", ""), format!("{text}m=video 40002 RTP/AVP 99\r\n"),
        text.replace("RTP/AVP", "unknown") ] { assert!(PlainAudioOffer::trunk(body.as_bytes(), true).is_err()); }
}
#[test]
fn identical_negotiated_payload_codec_clock_channel_and_fmtp_are_required_for_passthrough() {
    let offer = PlainAudioOffer::trunk(SDP, true).unwrap(); let text = String::from_utf8(SDP.to_vec()).unwrap();
    let answer = PlainAudioOffer::trunk(text.replace("RTP/AVP 0 8", "RTP/AVP 8").replace("a=rtpmap:0 PCMU/8000\r\n", "").as_bytes(), true).unwrap();
    assert_eq!(offer.matching_payloads(&answer).unwrap(), vec![8]);
    let wrong_clock = text.replace("a=rtpmap:8 PCMA/8000", "a=rtpmap:8 PCMA/16000");
    assert_eq!(PlainAudioOffer::trunk(wrong_clock.as_bytes(), true).err(), Some(MediaError::Configuration));
    for replacement in ["PCMU/8000", "PCMA/8000/2"] {
        let wrong = text.replace("a=rtpmap:8 PCMA/8000", &format!("a=rtpmap:8 {replacement}"));
        let wrong = PlainAudioOffer::trunk(wrong.as_bytes(), true).unwrap(); assert!(offer.matching_payloads(&wrong).is_err());
    }
    let mut different = text.into_bytes(); different.extend_from_slice(b"a=fmtp:8 fixture=1\r\n");
    assert!(offer.matching_payloads(&PlainAudioOffer::trunk(&different, true).unwrap()).is_err());
}
#[test]
fn local_sdp_uses_actual_supplied_bound_port_and_original_native_session_identity_not_remote_endpoint() {
    let (_rpc, _, _, ticket, _) = accepted_context(ENDPOINT, CONTENT); let offer = PlainAudioOffer::trunk(SDP, true).unwrap();
    let local = "127.0.0.1:49151".parse().unwrap(); let body = offer.with_local_endpoint(&ticket, local).unwrap();
    let text = std::str::from_utf8(&body).unwrap(); assert!(text.contains("m=audio 49151 RTP/AVP 0 8"));
    assert!(text.contains(&format!("o=app-account {} 1", ticket.native_id().expose_to_backend())));
    assert!(text.contains("s=SimAdmin application call")); assert!(text.contains("a=rtcp-mux\r\n"));
    assert!(!text.contains("40000")); assert!(!text.contains("VoWiFi"));
}
#[test]
fn unknown_zero_multicast_and_non_ip_asterisk_peers_cannot_become_audio_endpoints() {
    let text = String::from_utf8(SDP.to_vec()).unwrap();
    for address in ["0.0.0.0", "224.0.0.1", "fixture.invalid"] {
        let body = text.replace("127.0.0.1", address); let offer = PlainAudioOffer::trunk(body.as_bytes(), true).unwrap();
        assert_eq!(offer.endpoint().err(), Some(MediaError::Configuration));
    }
    let zero = PlainAudioOffer::trunk(text.replace("audio 40000", "audio 0").as_bytes(), true).unwrap(); assert!(zero.endpoint().is_err());
}
#[test]
fn duplicate_and_rtp_rtcp_mux_collision_payloads_are_rejected_before_answer() {
    let text = String::from_utf8(SDP.to_vec()).unwrap();
    for payloads in ["0 0", "64", "95", "128", "255"] {
        assert!(PlainAudioOffer::trunk(text.replace("RTP/AVP 0 8", &format!("RTP/AVP {payloads}")).as_bytes(), true).is_err());
    }
}
#[test]
fn sdp_resource_limits_and_debug_never_reveal_owner_peer_or_native_data() {
    assert!(PlainAudioOffer::trunk(&vec![b'x'; MAX_SDP + 1], true).is_err());
    let mut body = SDP.to_vec(); for _ in 0..129 { body.extend_from_slice(b"a=fixture\r\n"); }
    assert!(PlainAudioOffer::trunk(&body, true).is_err());
    let offer = PlainAudioOffer::trunk(SDP, true).unwrap(); assert_eq!(format!("{offer:?}"), "PlainAudioOffer(<redacted>)");
}
#[test]
fn opus_passthrough_preserves_rfc7587_clock_channels_fmtp_and_dynamic_payload_without_transcoding() {
    let body = b"v=0\r\no=fixture 10 11 IN IP4 127.0.0.1\r\ns=fixture\r\nc=IN IP4 127.0.0.1\r\nt=0 0\r\nm=audio 40000 RTP/AVP 106\r\na=rtpmap:106 opus/48000/2\r\na=fmtp:106 minptime=10;useinbandfec=1\r\na=sendrecv\r\na=rtcp-mux\r\n";
    let offer = PlainAudioOffer::trunk(body, true).unwrap();
    assert_eq!(offer.description.codecs[0].codec, crate::connectivity::core::voice::AudioCodec::Opus);
    assert_eq!(offer.description.codecs[0].clock_rate, 48000); assert_eq!(offer.description.codecs[0].channels, Some(2));
    assert_eq!(offer.matching_payloads(&offer).unwrap(), vec![106]);
    let invalid = String::from_utf8(body.to_vec()).unwrap().replace("opus/48000", "opus/16000");
    assert!(PlainAudioOffer::trunk(invalid.as_bytes(), true).is_err());
}
#[test]
fn telephone_event_mapping_is_separate_from_voice_and_survives_original_answer_passthrough() {
    let text = String::from_utf8(SDP.to_vec()).unwrap().replace("RTP/AVP 0 8", "RTP/AVP 0 8 101");
    let body = format!("{text}a=rtpmap:101 telephone-event/8000\r\na=fmtp:101 0-16\r\n");
    let offer = PlainAudioOffer::trunk(body.as_bytes(), true).unwrap();
    assert_eq!(offer.description.codecs.len(), 2); assert_eq!(offer.events.len(), 1);
    assert_eq!(offer.matching_payloads(&offer).unwrap(), vec![0, 8, 101]);
    let (_rpc, _, _, ticket, _) = accepted_context(ENDPOINT, CONTENT);
    let rendered = offer.with_local_endpoint(&ticket, "127.0.0.1:49152".parse().unwrap()).unwrap();
    let rendered = std::str::from_utf8(&rendered).unwrap(); assert!(rendered.contains("RTP/AVP 0 8 101"));
    assert!(rendered.contains("a=rtpmap:101 telephone-event/8000")); assert!(rendered.contains("a=fmtp:101 0-16"));
    let event_only = body.replace("RTP/AVP 0 8 101", "RTP/AVP 101"); assert!(PlainAudioOffer::trunk(event_only.as_bytes(), true).is_err());
}
#[test]
fn explicit_trunk_codec_allow_filters_native_offer_without_relabeling_payload_or_transcoding() {
    let offer = PlainAudioOffer::trunk(SDP, true).unwrap().restrict(&["pcmu".to_owned()]).unwrap();
    assert_eq!(offer.matching_payloads(&offer).unwrap(),vec![0]);
    let (_rpc, _, _, ticket, _) = accepted_context(ENDPOINT, CONTENT);
    let rendered=offer.with_local_endpoint(&ticket,"127.0.0.1:49152".parse().unwrap()).unwrap();
    let rendered=std::str::from_utf8(&rendered).unwrap();assert!(rendered.contains("RTP/AVP 0\r\n"));assert!(!rendered.contains("rtpmap:8"));
    assert!(PlainAudioOffer::trunk(SDP,true).unwrap().restrict(&["not-implemented".to_owned()]).is_err());
}

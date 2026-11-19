//! Source-only original IM Info DATA tests, intended for im_data_tests::info.
//! No transport, permission, readiness, notification or Text semantics follow.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::{
    im_info::ImInfo,
    im_text::{ImText, TextField},
    rpc_stream::{MAX_FIELD_BYTES, MAX_MAP_ENTRIES},
};

const COMMON: [(TextField, &str); 5] = [
    (TextField::ImdnId, "imdnId"),
    (TextField::ConversationId, "csnId"),
    (TextField::DisplayName, "displayName"),
    (TextField::UserData, "userData"),
    (TextField::OnlineMessage, "OnlineMsg"),
];

fn message<'a>(kind: &'a [u8], content: &'a [u8]) -> ImMessage<'a> {
    ImMessage {
        index: 1, time: 2, sender_uid: b"1_2", kind, content,
        attachments: vec![], parameters: vec![],
    }
}

fn decode(kind: &[u8], content: &[u8]) -> Result<Option<ImInfo>, CodecError> {
    ImInfo::decode(&message(kind, content))
}

#[test]
fn recognize_kinds() {
    for (kind, subtype, system) in [
        (b"Info.Text".as_slice(), b"Text".as_slice(), false),
        (b"System.Info.Text".as_slice(), b"Text".as_slice(), true),
        (b"Info.A0_z-9.more".as_slice(), b"A0_z-9.more".as_slice(), false),
        (b"System.Info.a0_Z-9.More".as_slice(), b"a0_Z-9.More".as_slice(), true),
    ] {
        let body = br#"{"content":"fixture info"}"#;
        let info = decode(kind, body).unwrap().unwrap();
        assert_eq!(info.kind(), subtype);
        assert_eq!(info.system(), system);
        assert_eq!(info.content(), "fixture info");
        assert!(info.field(TextField::Text).is_none());
        assert!(ImText::decode(&message(kind, body)).unwrap().is_none());
        assert!(ImText::decode(&message(kind, b"not-json")).unwrap().is_none());
    }
}

#[test]
fn retain_common() {
    let body = br#"{"content":"opaque info","text":{"ignored":true},"imdnId":"fixture/id:01","csnId":"not-a-box","displayName":"fixture name","userData":"{opaque}","OnlineMsg":"true"}"#;
    for kind in [b"Info.Common".as_slice(), b"System.Info.Common"] {
        let info = decode(kind, body).unwrap().unwrap();
        assert_eq!(info.content(), "opaque info");
        for ((field, _), value) in COMMON.into_iter().zip([
            "fixture/id:01", "not-a-box", "fixture name", "{opaque}", "true",
        ]) {
            assert_eq!(info.field(field), Some(value));
        }
        assert!(info.field(TextField::Text).is_none());
        let empty = decode(kind, br#"{"content":"","text":false,"imdnId":"","csnId":"","displayName":"","userData":"","OnlineMsg":""}"#).unwrap().unwrap();
        assert_eq!(empty.content(), "");
        for (field, _) in COMMON {
            assert_eq!(empty.field(field), Some(""));
        }
        assert!(empty.field(TextField::Text).is_none());
    }
}

#[test]
fn skip_other_kinds() {
    let oversized = vec![b'x'; MAX_FIELD_BYTES + 1];
    for kind in [
        b"".as_slice(), b"Info", b"System.Info", b"info.Text", b"InfoX.Text",
        b"System.info.Text", b"system.Info.Text", b"SystemInfo.Text",
        b"Other.Info.Text", b"System.System.Info.Text", b" Info.Text",
        b"Text", b"System.Text", b"Text.extra", b"Image", b"Unknown",
    ] {
        for body in [b"not-json".as_slice(), b"{", &[0xff, 0], oversized.as_slice()] {
            assert!(decode(kind, body).unwrap().is_none());
        }
    }
}

#[test]
fn bound_subtypes() {
    for prefix in [b"Info.".as_slice(), b"System.Info."] {
        for suffix in [b"a".as_slice(), b"Z", b"0", b"_", b"-", b".", b"aZ09_.-"] {
            let kind = [prefix, suffix].concat();
            assert_eq!(decode(&kind, br#"{"content":""}"#).unwrap().unwrap().kind(), suffix);
        }
        let suffix = vec![b'A'; 128];
        let kind = [prefix, suffix.as_slice()].concat();
        assert_eq!(decode(&kind, br#"{"content":""}"#).unwrap().unwrap().kind(), suffix.as_slice());
        let too_long = [prefix, vec![b'A'; 129].as_slice()].concat();
        assert_eq!(decode(&too_long, b"not-json").err(), Some(CodecError::TooLarge));
        for suffix in [
            b"".as_slice(), b" ", b"a b", b"a/b", b"a:b", b"a@b", b"a+b",
            b"a\n", b"a\0", &[0xff], "文".as_bytes(),
        ] {
            let kind = [prefix, suffix].concat();
            assert_eq!(decode(&kind, br#"{"content":""}"#).err(), Some(CodecError::InvalidField));
            assert_eq!(decode(&kind, b"not-json").err(), Some(CodecError::InvalidField));
        }
    }
}

#[test]
fn require_strings() {
    let empty = decode(b"Info.Empty", br#"{"content":""}"#).unwrap().unwrap();
    assert_eq!(empty.content(), "");
    for (field, _) in COMMON {
        assert!(empty.field(field).is_none());
    }
    for body in [
        b"".as_slice(), br#"{}"#, br#"[]"#, br#""raw info""#, b"null", b"true", b"1",
        br#"{"Content":"wrong key"}"#, br#"{"text":"not content"}"#,
        br#"{"content":null}"#, br#"{"content":false}"#, br#"{"content":1}"#,
        br#"{"content":[]}"#, br#"{"content":{}}"#, br#"{"content":"ok","OnlineMsg":true}"#,
    ] {
        assert!(decode(b"Info.Empty", body).is_err());
    }
    for (_, key) in COMMON {
        for value in ["null", "true", "false", "0", "[]", "{}"] {
            let body = format!(r#"{{"content":"ok","{key}":{value}}}"#);
            assert!(decode(b"System.Info.Common", body.as_bytes()).is_err());
        }
    }
}

#[test]
fn unescape_content() {
    let body = r#"{"con\u0074ent":"中文\n\"[{}]\" \\ \u4E2D \uD83E\uDD58","imdn\u0049d":"fixture\u002Did","csn\u0049d":"\u4E2D","display\u004Eame":"line\nname","user\u0044ata":"\\opaque","Online\u004Dsg":"\u0074rue"}"#;
    let info = decode(b"System.Info.Unicode", body.as_bytes()).unwrap().unwrap();
    assert_eq!(info.content(), "中文\n\"[{}]\" \\ 中 🥘");
    for ((field, _), value) in COMMON.into_iter().zip([
        "fixture-id", "中", "line\nname", "\\opaque", "true",
    ]) {
        assert_eq!(info.field(field), Some(value));
    }
    assert!(info.field(TextField::Text).is_none());
}

#[test]
fn reject_duplicates() {
    for body in [
        br#"{"content":"one","content":"two"}"#.as_slice(),
        br#"{"content":"one","con\u0074ent":"two"}"#,
        br#"{"con\u0074ent":"one","content":"two"}"#,
        br#"{"content":"ok","opaque":1,"opaque":2}"#,
        br#"{"content":"ok","opaque":1,"op\u0061que":{}}"#,
        br#"{"content":"ok","text":1,"te\u0078t":2}"#,
        r#"{"content":"ok","文":1,"\u6587":2}"#.as_bytes(),
    ] {
        assert!(decode(b"Info.Duplicate", body).is_err());
    }
    for (_, key) in COMMON {
        let body = format!(r#"{{"content":"ok","{key}":"one","{key}":"two"}}"#);
        assert!(decode(b"Info.Duplicate", body.as_bytes()).is_err());
    }
    for (key, escaped) in [
        ("imdnId", r"imdn\u0049d"), ("csnId", r"csn\u0049d"),
        ("displayName", r"display\u004Eame"), ("userData", r"user\u0044ata"),
        ("OnlineMsg", r"Online\u004Dsg"),
    ] {
        let body = format!(r#"{{"content":"ok","{key}":"one","{escaped}":"two"}}"#);
        assert!(decode(b"System.Info.Duplicate", body.as_bytes()).is_err());
    }
}

#[test]
fn ignore_unknowns() {
    let body = br#"{"content":"fixture-visible-marker","text":{"value":"fixture-text-marker"},"token":"fixture-token-marker","endpoint":{"host":"fixture-route-marker","port":1},"grant":true,"nested":[1,null,false,{"OnlineMsg":true,"content":"fixture-nested-marker"}],"Content":"fixture-alias-marker","onlinemsg":true}"#;
    let info = decode(b"System.Info.FixtureSubtype", body).unwrap().unwrap();
    assert_eq!(info.kind(), b"FixtureSubtype");
    assert!(info.system());
    assert_eq!(info.content(), "fixture-visible-marker");
    assert!(info.field(TextField::Text).is_none());
    for (field, _) in COMMON {
        assert!(info.field(field).is_none());
    }
    let debug = format!("{info:?}");
    assert_eq!(debug, "ImInfo(<redacted>)");
    for marker in [
        "FixtureSubtype", "fixture-visible-marker", "fixture-text-marker",
        "fixture-token-marker", "fixture-route-marker", "fixture-nested-marker",
        "fixture-alias-marker", "grant", "endpoint",
    ] {
        assert!(!debug.contains(marker));
    }
    let common = decode(b"Info.FixtureSubtype", br#"{"content":"fixture-body-marker","imdnId":"fixture-id-marker","csnId":"fixture-csn-marker","displayName":"fixture-name-marker","userData":"fixture-user-marker","OnlineMsg":"fixture-online-marker"}"#).unwrap().unwrap();
    assert_eq!(format!("{common:?}"), "ImInfo(<redacted>)");
}

#[test]
fn reject_invalid_data() {
    let complete = br#"{"content":"fixture","imdnId":"id","csnId":"csn","displayName":"name","userData":"data","OnlineMsg":"1","unknown":[null,{"x":1}]}"#;
    for end in 0..complete.len() {
        assert!(decode(b"Info.Invalid", &complete[..end]).is_err());
    }
    for body in [
        br#"{"content":"a\u0000b"}"#.as_slice(), b"{\"content\":\"a\0b\"}",
        br#"{"content":"\uD800"}"#, br#"{"content":"\uDC00"}"#,
        br#"{"content":"\uD800\u0041"}"#, br#"{"content":"\uDC00\uD800"}"#,
        br#"{"content":"\uZZZZ"}"#, br#"{"content":"\x41"}"#,
        b"{\"content\":\"line\nbreak\"}", b"{\"content\":\"\xff\"}",
        b"{\"content\":\"\xc0\xaf\"}", b"{\"content\":\"\xed\xa0\x80\"}",
        b"{\"\xff\":1,\"content\":\"ok\"}",
        br#"{"content":"ok","\uD800":1}"#,
    ] {
        assert!(decode(b"Info.Invalid", body).is_err());
    }
    for (_, key) in COMMON {
        let nul = format!(r#"{{"content":"ok","{key}":"a\u0000b"}}"#);
        assert!(decode(b"Info.Invalid", nul.as_bytes()).is_err());
        let invalid = [format!(r#"{{"content":"ok","{key}":""#).as_bytes(), &[0xff], b"\"}"].concat();
        assert!(decode(b"Info.Invalid", &invalid).is_err());
    }
    for tail in [b"{}".as_slice(), b"[]", b"null", b"true", b"1", b"\"x\"", b"x", b"\0", &[0xff]] {
        let trailing = [complete.as_slice(), tail].concat();
        assert!(decode(b"Info.Invalid", &trailing).is_err());
    }
    let whitespace = [complete.as_slice(), b" \r\n\t"].concat();
    assert_eq!(decode(b"Info.Invalid", &whitespace).unwrap().unwrap().content(), "fixture");
}

#[test]
fn bound_field_bytes() {
    assert_eq!(MAX_FIELD_BYTES, 65_536);
    let overhead = br#"{"content":""}"#.len();
    for raw_len in [MAX_FIELD_BYTES - 1, MAX_FIELD_BYTES] {
        let value = "a".repeat(raw_len - overhead);
        let body = format!(r#"{{"content":"{value}"}}"#);
        assert_eq!(body.len(), raw_len);
        let info = decode(b"Info.Bytes", body.as_bytes()).unwrap().unwrap();
        assert_eq!(info.content().len(), raw_len - overhead);
        assert_eq!(info.content(), value);
    }
    // A decoded 65,536-byte string cannot fit the same raw JSON budget:
    // its required object/key/quotes add overhead, even without escapes.
    for decoded_len in [MAX_FIELD_BYTES - overhead + 1, MAX_FIELD_BYTES, MAX_FIELD_BYTES + 1] {
        let body = format!(r#"{{"content":"{}"}}"#, "a".repeat(decoded_len));
        assert_eq!(decode(b"Info.Bytes", body.as_bytes()).err(), Some(CodecError::TooLarge));
    }
    for (field, key) in COMMON {
        let empty = format!(r#"{{"content":"","{key}":""}}"#);
        let value = "a".repeat(MAX_FIELD_BYTES - empty.len());
        let mut body = format!(r#"{{"content":"","{key}":"{value}"}}"#);
        assert_eq!(body.len(), MAX_FIELD_BYTES);
        let info = decode(b"Info.Bytes", body.as_bytes()).unwrap().unwrap();
        assert_eq!(info.field(field), Some(value.as_str()));
        body.push(' ');
        assert_eq!(decode(b"Info.Bytes", body.as_bytes()).err(), Some(CodecError::TooLarge));
    }
    for (encoded, decoded) in [(r"\u0061", "a"), (r"\u4E2D", "中"), (r"\uD83E\uDD58", "🥘")] {
        let budget = MAX_FIELD_BYTES - overhead;
        let count = budget / encoded.len();
        let remainder = budget % encoded.len();
        let wire = encoded.repeat(count) + &"a".repeat(remainder);
        let expected = decoded.repeat(count) + &"a".repeat(remainder);
        let mut body = format!(r#"{{"content":"{wire}"}}"#);
        assert_eq!(body.len(), MAX_FIELD_BYTES);
        let info = decode(b"Info.Bytes", body.as_bytes()).unwrap().unwrap();
        assert_eq!(info.content(), expected);
        assert_eq!(info.content().len(), count * decoded.len() + remainder);
        assert!(info.content().len() <= wire.len());
        body.push(' ');
        assert_eq!(decode(b"Info.Bytes", body.as_bytes()).err(), Some(CodecError::TooLarge));
    }
}

#[test]
fn bound_map_entries() {
    assert_eq!(MAX_MAP_ENTRIES, 128);
    for include_common in [false, true] {
        let mut base = String::from(r#"{"content":"bounded""#);
        if include_common {
            for (_, key) in COMMON {
                base.push_str(&format!(r#", "{key}":"""#));
            }
        }
        let known = if include_common { 6 } else { 1 };
        for total in [MAX_MAP_ENTRIES, MAX_MAP_ENTRIES + 1] {
            let mut body = base.clone();
            for n in 0..total - known {
                body.push_str(&format!(r#", "unknown{n}":{{"nested":[0,null,false]}}"#));
            }
            body.push('}');
            let result = decode(b"Info.Map", body.as_bytes());
            if total == MAX_MAP_ENTRIES {
                let info = result.unwrap().unwrap();
                assert_eq!(info.content(), "bounded");
                for (field, _) in COMMON {
                    assert_eq!(info.field(field), if include_common { Some("") } else { None });
                }
            } else {
                assert_eq!(result.err(), Some(CodecError::InvalidField));
            }
        }
    }
}

#[test]
fn bound_depth() {
    // The root object contributes one level: 31 unknown containers fit,
    // while 32 (or 1,000) must be rejected before IgnoredAny traverses them.
    for (depth, accepted) in [(31, true), (32, false), (1000, false)] {
        for (open, close) in [("[", "]"), (r#"{"x":"#, "}")] {
            let body = format!(r#"{{"content":"bounded","unknown":{}0{}}}"#, open.repeat(depth), close.repeat(depth));
            let result = decode(b"Info.Depth", body.as_bytes());
            if accepted {
                assert_eq!(result.unwrap().unwrap().content(), "bounded");
            } else {
                assert_eq!(result.err(), Some(CodecError::InvalidField));
            }
        }
    }
    let encoded = r#"[{}]\"quoted\"\\[]"#.repeat(1000);
    let expected = "[{}]\"quoted\"\\[]".repeat(1000);
    let ignored = "}][{{".repeat(1000);
    let body = format!(r#"{{"content":"{encoded}","unknown":"{ignored}"}}"#);
    assert_eq!(decode(b"Info.Depth", body.as_bytes()).unwrap().unwrap().content(), expected);
}

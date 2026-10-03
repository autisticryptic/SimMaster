// Offline only. Real derivation/builders/retry driver; fake SIM and byte channel.
// No sockets, D-Bus, modem, namespaces or XFRM operations are used here.
use super::{
    access::ImsChannel,
    context::{ImsIdentity, ImsRoute, SipTransport},
    digest_aka as digest,
    register::{run_register_observed, RegisterAuthenticator, RegisterFailure},
    sip_frame, ImsError,
};
use crate::connectivity::modems::ims::{cellular_ims, vowifi};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    time::Duration,
};
use vowifi::profiles::{derive_standard_3gpp_profile, CarrierProfile, Standard3gppAccess};

pub(crate) const SIM_SECURITY: &str = "ipsec-3gpp;alg=hmac-sha-1-96;ealg=aes-cbc;spi-c=10001;spi-s=10002;port-c=5062;port-s=5063;prot=esp;mod=trans";
const NONCE: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
const USER: &str = "001010000000001@ims.mnc001.mcc001.3gppnetwork.org";
const REALM: &str = "ims.mnc001.mcc001.3gppnetwork.org";
const URI: &str = "sip:ims.mnc001.mcc001.3gppnetwork.org";
const RES: &[u8] = &[1, 2, 3, 4, 5, 6, 7, 8];
const CK: &[u8] = &[0x11; 16];
const IK: &[u8] = &[0x22; 16];

pub(crate) trait WireBuilder: Send {
    fn build(&self, cseq: u32, expires: u32, authorization: Option<&str>) -> Vec<u8>;
    fn advance(&mut self, failure: &RegisterFailure) -> bool;
    fn label(&self) -> &'static str;
}

// Independent fixture registrar: raw MD5/SHA-256 formulas, not the client's
// compute_aka_response. It rejects altered proof/URI/realm/identity/nonce count.
fn md5hex(bytes: &[u8]) -> String {
    format!("{:x}", md5::compute(bytes))
}
fn server_proof(algorithm: &str, fields: &HashMap<String, String>) -> String {
    let mut key = RES.to_vec();
    key.extend_from_slice(IK);
    key.extend_from_slice(CK);
    let password = if algorithm == "AKAv1-MD5" {
        RES.to_vec()
    } else {
        let bytes = if algorithm == "AKAv2-SHA-256" {
            ring::hmac::sign(
                &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, &key),
                b"http-digest-akav2-password",
            )
            .as_ref()
            .to_vec()
        } else {
            let mut block = [0u8; 64];
            block[..key.len()].copy_from_slice(&key);
            let mut inner = block.iter().map(|x| x ^ 0x36).collect::<Vec<_>>();
            inner.extend_from_slice(b"http-digest-akav2-password");
            let mut outer = block.iter().map(|x| x ^ 0x5c).collect::<Vec<_>>();
            outer.extend_from_slice(&md5::compute(inner).0);
            md5::compute(outer).0.to_vec()
        };
        B64.encode(bytes).into_bytes()
    };
    let hash = |bytes: &[u8]| {
        if algorithm == "AKAv2-SHA-256" {
            ring::digest::digest(&ring::digest::SHA256, bytes)
                .as_ref()
                .iter()
                .map(|x| format!("{x:02x}"))
                .collect::<String>()
        } else {
            md5hex(bytes)
        }
    };
    let mut a1 = format!("{USER}:{REALM}:").into_bytes();
    a1.extend_from_slice(&password);
    let a2 = format!("REGISTER:{URI}");
    hash(
        format!(
            "{}:{NONCE}:{}:{}:auth:{}",
            hash(&a1),
            fields["nc"],
            fields["cnonce"],
            hash(a2.as_bytes())
        )
        .as_bytes(),
    )
}
fn parameters(header: &str) -> HashMap<String, String> {
    header
        .trim_start_matches("Digest ")
        .split(',')
        .filter_map(|part| part.trim().split_once('='))
        .map(|(k, v)| (k.to_string(), v.trim_matches('"').to_string()))
        .collect()
}
fn header(frame: &[u8], name: &str) -> Option<String> {
    sip_frame::header_value(frame, name)
}
fn response(request: &[u8], status: u16, extra: &str) -> Vec<u8> {
    format!(
        "SIP/2.0 {status} Fixture\r\nCall-ID: {}\r\nCSeq: {}\r\n{extra}Content-Length: 0\r\n\r\n",
        header(request, "Call-ID").unwrap(),
        header(request, "CSeq").unwrap()
    )
    .into_bytes()
}
#[derive(Clone, Copy)]
struct Scenario {
    id: &'static str,
    wifi: bool,
    mode: &'static str,
    expected: bool,
    fallbacks: bool,
}
struct Peer {
    scenario: Scenario,
    route: ImsRoute,
    queue: VecDeque<Vec<u8>>,
    sent: Vec<Vec<u8>>,
    statuses: Vec<u16>,
    requeued: usize,
    digest_verified: usize,
    nc: u32,
}
impl Peer {
    fn algorithm(&self) -> &str {
        match self.scenario.mode {
            "akav2" => "AKAv2-MD5",
            "sha256" => "AKAv2-SHA-256",
            "plain" => "MD5",
            _ => "AKAv1-MD5",
        }
    }
    fn reply(&mut self, request: &[u8], status: u16, extra: &str) {
        self.statuses.push(status);
        self.queue.push_back(response(request, status, extra));
    }
    fn challenge(&mut self, request: &[u8]) {
        let proxy = self.scenario.mode == "proxy";
        let nonce = if self.scenario.mode == "bad_nonce" {
            "abcd"
        } else {
            NONCE
        };
        let extra = format!(
            "{}: Digest realm=\"{REALM}\",nonce=\"{nonce}\",algorithm={},qop=\"auth\"\r\n",
            if proxy {
                "Proxy-Authenticate"
            } else {
                "WWW-Authenticate"
            },
            self.algorithm()
        );
        self.reply(request, if proxy { 407 } else { 401 }, &extra);
    }
}
impl ImsChannel for Peer {
    async fn send_sip(&mut self, frame: &[u8]) -> Result<(), ImsError> {
        assert!(sip_frame::is_request(frame, "REGISTER"));
        assert!(header(frame, "Call-ID").is_some());
        assert!(header(frame, "CSeq").unwrap().ends_with(" REGISTER"));
        self.sent.push(frame.to_vec());
        if self.scenario.mode == "drop" {
            if self.sent.len() == 1 {
                return Ok(());
            }
            if self.sent.len() == 2 {
                assert_eq!(
                    self.sent[0], self.sent[1],
                    "UDP retransmission changed bytes"
                );
            }
        }
        let auth = header(
            frame,
            if self.scenario.mode == "proxy" {
                "Proxy-Authorization"
            } else {
                "Authorization"
            },
        );
        let fields = auth.as_deref().map(parameters).unwrap_or_default();
        let authenticated = fields.get("response").is_some_and(|v| !v.is_empty());
        let expires = header(frame, "Expires").unwrap().parse::<u32>().unwrap();
        if self.scenario.mode == "custom_domain" {
            assert_ne!(
                std::str::from_utf8(frame)
                    .unwrap()
                    .split_whitespace()
                    .nth(1),
                Some("sip:operator.private.example")
            );
            self.reply(frame, 403, "");
            return Ok(());
        }
        if self.scenario.mode == "403" {
            self.reply(frame, 403, "");
            return Ok(());
        }
        if self.scenario.mode == "omit" {
            assert!(header(frame, "P-Access-Network-Info").is_none());
            assert!(header(frame, "Require").is_none());
            assert!(header(frame, "Security-Client").is_none());
            assert!(!header(frame, "Contact")
                .unwrap_or_default()
                .contains("icsi.mmtel"));
        }
        if matches!(self.scenario.mode, "421" | "494" | "no_fallback") && !authenticated {
            if !header(frame, "Require").is_some_and(|v| v.contains("sec-agree")) {
                self.reply(
                    frame,
                    if self.scenario.mode == "494" {
                        494
                    } else {
                        421
                    },
                    "Require: sec-agree\r\n",
                );
                return Ok(());
            }
            assert!(header(frame, "Proxy-Require").is_some_and(|v| v.contains("sec-agree")));
            assert!(header(frame, "Security-Client").is_some());
            if auth.is_none() {
                self.reply(frame, 400, "");
                return Ok(());
            }
        }
        if !self.scenario.wifi && self.scenario.mode != "omit" && !authenticated {
            assert!(
                header(frame, "Authorization").is_some() || auth.is_some(),
                "derived LTE must identify AKA before challenge"
            );
            assert!(header(frame, "Require").is_some_and(|v| v.contains("sec-agree")));
        }
        if self.scenario.mode == "min_pre" && expires < 7200 {
            self.reply(frame, 423, "Min-Expires: 7200\r\n");
            return Ok(());
        }
        if authenticated {
            let valid = fields.get("username").map(String::as_str) == Some(USER)
                && fields.get("realm").map(String::as_str) == Some(REALM)
                && fields.get("uri").map(String::as_str) == Some(URI)
                && fields.get("nonce").map(String::as_str) == Some(NONCE)
                && fields.get("algorithm").map(String::as_str) == Some(self.algorithm())
                && fields.get("qop").map(String::as_str) == Some("auth")
                && fields.contains_key("nc")
                && fields.contains_key("cnonce");
            if !valid || fields["response"] != server_proof(self.algorithm(), &fields) {
                self.reply(frame, 403, "");
                return Ok(());
            }
            let nc = u32::from_str_radix(&fields["nc"], 16).unwrap();
            assert!(nc > self.nc);
            self.nc = nc;
            self.digest_verified += 1;
            if self.scenario.mode == "auth_exhaust" {
                self.challenge(frame);
                return Ok(());
            }
            if self.scenario.mode == "min_post" && expires < 7200 {
                self.reply(frame, 423, "Min-Expires: 7200\r\n");
                return Ok(());
            }
            self.reply(frame, 200, &format!("Expires: {expires}\r\n"));
        } else {
            if self.scenario.mode == "noise" {
                self.queue.push_back(b"NOTIFY sip:fixture SIP/2.0\r\nCall-ID: notification\r\nCSeq: 1 NOTIFY\r\nContent-Length: 0\r\n\r\n".to_vec());
                self.queue.push_back(b"SIP/2.0 200 stale\r\nCall-ID: stale\r\nCSeq: 99 REGISTER\r\nContent-Length: 0\r\n\r\n".to_vec());
                self.reply(frame, 100, "");
            }
            self.challenge(frame);
        }
        Ok(())
    }
    async fn recv_sip(&mut self, _timeout: Duration) -> Result<Vec<u8>, ImsError> {
        self.queue
            .pop_front()
            .ok_or(ImsError::new("ims_channel_read_timeout"))
    }
    fn requeue(&mut self, _frame: Vec<u8>) {
        self.requeued += 1;
    }
    fn route(&self) -> ImsRoute {
        self.route
    }
    fn security_verify(&self) -> Option<&str> {
        None
    }
}
struct Auth {
    wire: Box<dyn WireBuilder>,
    saved: Option<digest::DigestChallenge>,
    expires: u32,
    nc: u32,
    corrupt: bool,
}
impl Auth {
    fn authorized(&mut self, cseq: u32) -> Result<Vec<u8>, ImsError> {
        let challenge = self
            .saved
            .as_ref()
            .ok_or(ImsError::new("simulation_missing_challenge"))?;
        self.nc += 1;
        let nc = format!("{:08x}", self.nc);
        let cnonce = "offline-fixture-cnonce";
        let proof = if self.corrupt {
            "incorrect-proof".to_string()
        } else {
            digest::compute_aka_response(
                USER,
                &challenge.realm,
                &digest::AkaMaterial {
                    res: RES,
                    ck: CK,
                    ik: IK,
                },
                &challenge.algorithm,
                "REGISTER",
                URI,
                &challenge.nonce,
                challenge.qop.as_deref(),
                cnonce,
                &nc,
            )?
        };
        let authorization =
            digest::build_authorization_header(challenge, USER, URI, &proof, cnonce, &nc);
        Ok(self.wire.build(cseq, self.expires, Some(&authorization)))
    }
}
impl RegisterAuthenticator<Peer> for Auth {
    async fn authenticated_request(
        &mut self,
        frame: &[u8],
        cseq: u32,
    ) -> Result<Vec<u8>, ImsError> {
        let challenge = digest::select_digest_challenge(
            &sip_frame::header_values(frame, "WWW-Authenticate"),
            &sip_frame::header_values(frame, "Proxy-Authenticate"),
            false,
        )?;
        digest::decode_aka_nonce(&challenge.nonce)?;
        self.saved = Some(challenge);
        self.authorized(cseq)
    }
    async fn rebuild_register_with_min_expires(
        &mut self,
        _frame: &[u8],
        cseq: u32,
        min: u32,
        authenticated: bool,
    ) -> Result<Vec<u8>, ImsError> {
        self.expires = min;
        if authenticated {
            self.authorized(cseq)
        } else {
            Ok(self.wire.build(cseq, min, None))
        }
    }
}
async fn run_case(scenario: Scenario) -> Value {
    let base = derive_standard_3gpp_profile(
        "001",
        "01",
        if scenario.wifi {
            Standard3gppAccess::WifiEpdg
        } else {
            Standard3gppAccess::LteEpc
        },
    )
    .unwrap();
    let profile: &'static CarrierProfile = if scenario.mode == "omit" {
        let mut p = *base;
        p.ims.register.sec_agree_mode = "disabled";
        p.ims.register.require_sec_agree_headers = false;
        p.ims.register.proxy_require_sec_agree_headers = false;
        p.ims.register.include_pani_initial = false;
        p.ims.register.include_pani_authenticated = false;
        p.ims.register.include_mmtel_features = false;
        p.ims.register.always_add_sip_instance = false;
        Box::leak(Box::new(p))
    } else {
        base
    };
    let identity = ImsIdentity {
        private_user: USER.into(),
        public_uri: format!("sip:{USER}"),
        contact_user: "001010000000001".into(),
        home_domain: REALM.into(),
        contact_user_phone: false,
    };
    let route = ImsRoute {
        local_addr: "127.0.0.1:5062".parse().unwrap(),
        pcscf_addr: "127.0.0.1:5060".parse().unwrap(),
        transport: SipTransport::Udp,
    };
    let wire = if scenario.wifi {
        vowifi::live::offline_sim_adapter::builder(profile, identity, route)
    } else {
        cellular_ims::live::offline_sim_adapter::builder(profile, identity, route)
    };
    let mut auth = Auth {
        wire,
        saved: None,
        expires: 3600,
        nc: 0,
        corrupt: scenario.mode == "bad_proof",
    };
    let mut peer = Peer {
        scenario,
        route,
        queue: VecDeque::new(),
        sent: vec![],
        statuses: vec![],
        requeued: 0,
        digest_verified: 0,
        nc: 0,
    };
    let mut success = false;
    let mut rounds = 0;
    let mut failure_code = None;
    let mut candidates = vec![];
    for _ in 0..if scenario.wifi { 8 } else { 24 } {
        candidates.push(auth.wire.label());
        let request = auth.wire.build(1, auth.expires, None);
        match run_register_observed(&mut peer, &request, &mut auth).await {
            Ok(result) => {
                success = true;
                rounds = result.auth_rounds;
                assert!(result.authenticated);
                break;
            }
            Err(failure) => {
                rounds = failure.auth_rounds;
                failure_code = Some(failure.error.code());
                if !scenario.fallbacks || !auth.wire.advance(&failure) {
                    break;
                }
            }
        }
    }
    if success {
        assert!(peer.digest_verified > 0);
    }
    if scenario.mode == "noise" {
        assert!(peer.requeued >= 1);
    }
    if scenario.mode == "auth_exhaust" {
        assert_eq!(rounds, 2);
        assert_eq!(candidates.len(), 1);
    }
    if matches!(scenario.mode, "403" | "bad_proof" | "custom_domain") {
        assert_eq!(candidates.len(), 1);
    }
    json!({"id":scenario.id,"access":if scenario.wifi{"vowifi"}else{"lte"},"expected_success":scenario.expected,
        "observed_success":success,"passed":success==scenario.expected,"status_trace":peer.statuses,
        "request_count":peer.sent.len(),"auth_rounds":rounds,"digest_verified_count":peer.digest_verified,
        "candidate_trace":candidates,"failure_code":failure_code,
        "limitations":["SIM output is synthetic; security transport metadata is not encrypted XFRM/IKE","No radio, NAS, entitlement or operator availability proof"]})
}

#[tokio::test]
async fn offline_derivation_registration_matrix() {
    let cases = [
        ("lte_aka_baseline", false, "baseline", true, true),
        ("wifi_aka_baseline", true, "baseline", true, true),
        ("wifi_421_cumulative", true, "421", true, true),
        ("wifi_494_cumulative", true, "494", true, true),
        (
            "wifi_without_fallback_counterexample",
            true,
            "no_fallback",
            false,
            false,
        ),
        ("lte_proxy_407", false, "proxy", true, true),
        ("lte_akav2_md5", false, "akav2", true, true),
        ("wifi_akav2_sha256", true, "sha256", true, true),
        ("lte_423_before_aka", false, "min_pre", true, true),
        ("wifi_423_after_aka", true, "min_post", true, true),
        ("lte_interleaved_frames", false, "noise", true, true),
        ("lte_udp_retransmission", false, "drop", true, true),
        ("lte_auth_round_bound", false, "auth_exhaust", false, true),
        ("wifi_terminal_403", true, "403", false, true),
        (
            "lte_custom_domain_counterexample",
            false,
            "custom_domain",
            false,
            true,
        ),
        (
            "lte_wrong_digest_counterexample",
            false,
            "bad_proof",
            false,
            true,
        ),
        ("wifi_malformed_nonce", true, "bad_nonce", false, true),
        ("lte_plain_md5_not_authorized", false, "plain", false, true),
        ("lte_explicit_omit_sms_only", false, "omit", true, true),
    ];
    let mut results = vec![];
    for (id, wifi, mode, expected, fallbacks) in cases {
        let result = run_case(Scenario {
            id,
            wifi,
            mode,
            expected,
            fallbacks,
        })
        .await;
        eprintln!(
            "SIMULATION {} passed={} registered={} requests={}",
            id, result["passed"], result["observed_success"], result["request_count"]
        );
        results.push(result);
    }
    assert!(derive_standard_3gpp_profile("999", "99", Standard3gppAccess::LteEpc).is_none());
    assert_eq!(
        vowifi::profiles::standard_ims_home_domain("310", "26"),
        vowifi::profiles::standard_ims_home_domain("310", "026")
    );
    let nr_name = vowifi::profiles::standard_tai_epdg_fqdn("001", "01", 0x123456, "nr").unwrap();
    assert!(nr_name.starts_with("tac-lb56.tac-mb34.tac-hb12.5gstac"));
    let passed = results.iter().all(|r| r["passed"] == true);
    let report = json!({"suite_id":"simadmin-offline-derived-registration-v1","evidence_kind":"offline_simulation",
        "live_network_verified":false,"hardware_used":false,"passed":passed,"scenarios":results,
        "naming_checks":{"private_plmn_rejected":true,"mnc_padding":true,"nr_tai_format":true},
        "nr_registration":"not_implemented_or_tested; NR naming metadata only",
        "scope":"real derivation, request builders, candidate transitions, shared REGISTER and Digest-AKA; in-memory peer and synthetic USIM material"});
    if let Ok(path) = std::env::var("SIMADMIN_DERIVATION_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    assert!(passed, "one or more offline registration scenarios failed");
}

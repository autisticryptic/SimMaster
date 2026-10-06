use super::*;

#[test]
fn cmcc_restores_september_initial_declaration_without_weakening_algorithms() {
    // Source baseline: 7c6cf86/09edc03/33d16f3 profiles blob
    // cc05393f657ca36747682fe3c6291b33e374129a had these two flags false.
    for mnc in ["00", "02"] {
        let p = derive_standard_3gpp_profile("460", mnc, Standard3gppAccess::LteEpc).unwrap();
        let r = p.ims.register;
        assert!(!r.require_sec_agree_headers && !r.proxy_require_sec_agree_headers);
        assert_eq!(r.initial_authorization, "aka_empty");
        assert_eq!(r.sec_agree_mode, "auto");
        assert!(r.supported_header.split(',').any(|x| x == "sec-agree"));
        assert!(r.strict_security_server_offer);
        assert_eq!(r.security_client_mechanisms, ["hmac-sha-1-96/aes-cbc/esp/trans", "hmac-sha-1-96/null/esp/trans"]);
        assert_eq!(p.ims.domain, format!("ims.mnc{:03}.mcc460.3gppnetwork.org", mnc.parse::<u16>().unwrap()));
        assert_eq!(p.ims.realm, p.ims.domain);
        assert_eq!(p.ims.transport, "udp");
        assert!(!r.include_visited_network);
    }
}

#[test]
fn other_home_networks_and_three_digit_mncs_keep_current_policy() {
    for (mcc, mnc) in [("515", "02"), ("204", "08"), ("460", "11"), ("460", "01"), ("001", "01"), ("460", "002")] {
        let p = derive_standard_3gpp_profile(mcc, mnc, Standard3gppAccess::LteEpc).unwrap();
        assert!(p.ims.register.require_sec_agree_headers && p.ims.register.proxy_require_sec_agree_headers);
    }
    for mnc in ["00", "02"] {
        let p = derive_standard_3gpp_profile("460", mnc, Standard3gppAccess::WifiEpdg).unwrap();
        assert!(!p.ims.register.require_sec_agree_headers && !p.ims.register.proxy_require_sec_agree_headers);
        assert_eq!(p.ims.register.initial_authorization, "none");
    }
}

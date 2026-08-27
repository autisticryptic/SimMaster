//! Shared IMS REGISTER Contact feature completion.
//!
//! Carrier databases are overlays, not complete wire templates.  Only an
//! explicit `custom` mode suppresses the access baseline; all other modes keep
//! the database order and fill fields required by the selected access shape.

pub struct ContactCompletion<'a> {
    pub mode: &'a str,
    pub explicit: &'a [&'a str],
    pub access_network_info: &'a str,
    pub include_mmtel: bool,
    pub include_video: bool,
    pub include_sip_instance: bool,
    pub always_add_sip_instance: bool,
    pub sip_instance: &'a str,
    pub reg_id: u32,
    pub expires: Option<u32>,
}

pub fn complete_contact_parameters(input: ContactCompletion<'_>) -> Vec<String> {
    let mut parameters = Vec::new();
    for parameter in input.explicit {
        let name = parameter_name(parameter);
        if name.eq_ignore_ascii_case("video") && !input.include_video {
            continue;
        }
        if !input.include_sip_instance
            && (name.eq_ignore_ascii_case("+sip.instance") || name.eq_ignore_ascii_case("reg-id"))
        {
            continue;
        }
        push_once(&mut parameters, parameter);
    }

    if input.mode.eq_ignore_ascii_case("custom") {
        return parameters;
    }

    push_once(
        &mut parameters,
        &format!(
            "+g.3gpp.accesstype=\"{}\"",
            input.access_network_info.trim()
        ),
    );
    if input.include_mmtel {
        push_once(&mut parameters, "audio");
        push_once(
            &mut parameters,
            "+g.3gpp.icsi-ref=\"urn%3Aurn-7%3A3gpp-service.ims.icsi.mmtel\"",
        );
    }
    push_once(&mut parameters, "+g.3gpp.smsip");
    if input.include_sip_instance {
        push_once(
            &mut parameters,
            &format!("+sip.instance=\"<{}>\"", input.sip_instance),
        );
        if input.always_add_sip_instance {
            push_once(&mut parameters, &format!("reg-id={}", input.reg_id));
        }
    }
    if let Some(expires) = input.expires {
        push_once(&mut parameters, &format!("expires={expires}"));
    }
    parameters
}

fn parameter_name(parameter: &str) -> &str {
    parameter
        .split_once('=')
        .map_or(parameter, |(name, _)| name)
        .trim()
}

fn push_once(parameters: &mut Vec<String>, parameter: &str) {
    let name = parameter_name(parameter);
    if parameters
        .iter()
        .any(|existing| parameter_name(existing).eq_ignore_ascii_case(name))
    {
        return;
    }
    parameters.push(parameter.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn standard<'a>(explicit: &'a [&'a str]) -> ContactCompletion<'a> {
        ContactCompletion {
            mode: "standard",
            explicit,
            access_network_info: "IEEE-802.11",
            include_mmtel: false,
            include_video: false,
            include_sip_instance: true,
            always_add_sip_instance: true,
            sip_instance: "urn:uuid:test",
            reg_id: 2,
            expires: None,
        }
    }

    #[test]
    fn wifi_baseline_is_compact_and_stably_bound() {
        let parameters = complete_contact_parameters(standard(&[]));
        assert!(parameters.iter().any(|value| value == "+g.3gpp.smsip"));
        assert!(!parameters.iter().any(|value| value == "audio"));
        assert!(!parameters
            .iter()
            .any(|value| value.starts_with("+g.3gpp.icsi-ref=")));
        assert!(parameters
            .iter()
            .any(|value| value == "+sip.instance=\"<urn:uuid:test>\""));
        assert!(parameters.iter().any(|value| value == "reg-id=2"));
    }

    #[test]
    fn standard_overlay_keeps_order_and_fills_lte_voice_fields_once() {
        let mut input = standard(&["+g.3gpp.mid-call", "AUDIO", "+G.3GPP.SMSIP"]);
        input.include_mmtel = true;
        input.expires = Some(3600);
        let parameters = complete_contact_parameters(input);
        assert_eq!(parameters[0], "+g.3gpp.mid-call");
        assert_eq!(
            parameters
                .iter()
                .filter(|value| value.eq_ignore_ascii_case("audio"))
                .count(),
            1
        );
        assert_eq!(
            parameters
                .iter()
                .filter(|value| value.eq_ignore_ascii_case("+g.3gpp.smsip"))
                .count(),
            1
        );
        assert!(parameters.iter().any(|value| value == "expires=3600"));
    }

    #[test]
    fn custom_mode_does_not_invent_carrier_fields() {
        let mut input = standard(&["+g.3gpp.mid-call", "video"]);
        input.mode = "custom";
        let parameters = complete_contact_parameters(input);
        assert_eq!(parameters, ["+g.3gpp.mid-call"]);
    }
}

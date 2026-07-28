//! Importers that turn public carrier data into VoWiFi profile records.
//!
//! ## What these sources can and cannot give you
//!
//! The genuinely carrier-specific parts of a VoWiFi profile are the IMS
//! REGISTER quirks (sec-agree strictness, Contact shape, retry policy). No
//! public database publishes those — they are found by testing. What public
//! sources *do* provide is the surrounding configuration:
//!
//! - **AOSP `apns-conf.xml`** (Apache-2.0): APN names per PLMN, including the
//!   `ims` APN, plus MVNO matching rules.
//! - **AOSP CarrierConfig** (Apache-2.0): whether the carrier supports VoWiFi
//!   at all, the default WFC mode, and IMS-related toggles.
//! - **Apple IPCC carrier bundles**: APNs, VoLTE/VoWiFi enablement flags, E911
//!   entitlement URLs. These are Apple/carrier property, so SimAdmin never
//!   ships or downloads them — an operator points the importer at a bundle they
//!   already have.
//!
//! Notably absent from *all* of them is the ePDG hostname, because it is not
//! configuration: 3GPP TS 23.003 §19.4.2.4 defines it as a function of the
//! IMSI. The derivation layer computes it instead.

use std::collections::BTreeMap;

use serde::Serialize;

use super::profile_record::CarrierProfileRecord;
use super::profiles::generate_standard_3gpp_profile;

/// The subset of a profile a public source can actually populate. Everything
/// else keeps the derived or existing value, so importing never silently
/// downgrades a hand-verified profile.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ImportedCarrierFacts {
    pub mcc: String,
    pub mnc: String,
    /// Operator display name, when the source carries one.
    pub brand: Option<String>,
    /// The IMS APN, when the source names one explicitly.
    pub ims_apn: Option<String>,
    /// Whether the source says this carrier supports VoWiFi at all.
    pub vowifi_supported: Option<bool>,
    /// E911 / emergency entitlement endpoint, when published.
    pub entitlement_url: Option<String>,
}

impl ImportedCarrierFacts {
    pub fn plmn(&self) -> String {
        format!("{}{}", self.mcc, self.mnc)
    }

    /// Build a full profile record: start from the 3GPP-derived defaults for
    /// this PLMN, then overlay whatever the source actually told us.
    pub fn to_record(&self) -> Option<CarrierProfileRecord> {
        if self.mcc.len() != 3 || self.mnc.is_empty() || self.mnc.len() > 3 {
            return None;
        }
        if !self.mcc.chars().all(|c| c.is_ascii_digit())
            || !self.mnc.chars().all(|c| c.is_ascii_digit())
        {
            return None;
        }
        let derived = generate_standard_3gpp_profile(&self.mcc, &self.mnc, self.mnc.len() as u8);
        let mut record = CarrierProfileRecord::from_profile(derived);
        record.meta.profile_id = format!("imported_{}", self.plmn());
        if let Some(brand) = self.brand.as_ref().filter(|value| !value.trim().is_empty()) {
            record.meta.brand = brand.trim().to_string();
            record.meta.operator_legal_name = brand.trim().to_string();
        }
        if let Some(apn) = self
            .ims_apn
            .as_ref()
            .filter(|value| !value.trim().is_empty())
        {
            record.epdg.apn = Some(apn.trim().to_string());
        }
        if let Some(url) = self
            .entitlement_url
            .as_ref()
            .filter(|value| !value.trim().is_empty())
        {
            record.e911.entitlement_url = Some(url.trim().to_string());
            record.e911.enabled = true;
            // Their reference implementation auto-fills this when E911 is on;
            // an entitlement URL without a host policy is not actionable.
            if record.e911.websheet_host_policy.is_none() {
                record.e911.websheet_host_policy = Some("public_https".to_string());
            }
        }
        record.meta.source_refs = vec![format!("imported:{}", self.plmn())];
        Some(record)
    }
}

/// Parse an AOSP `apns-conf.xml`.
///
/// Only `<apn>` entries whose `type` mentions `ims` are interesting here; the
/// rest describe internet/MMS bearers that the per-line APN setting already
/// covers.
pub fn parse_aosp_apns(xml: &str) -> Vec<ImportedCarrierFacts> {
    let mut by_plmn: BTreeMap<String, ImportedCarrierFacts> = BTreeMap::new();
    for element in xml_elements(xml, "apn") {
        let Some(mcc) = element.get("mcc") else {
            continue;
        };
        let Some(mnc) = element.get("mnc") else {
            continue;
        };
        let types = element
            .get("type")
            .map(|value| value.to_ascii_lowercase())
            .unwrap_or_default();
        if !types.split(',').any(|kind| kind.trim() == "ims") {
            continue;
        }
        let entry = by_plmn
            .entry(format!("{mcc}{mnc}"))
            .or_insert_with(|| ImportedCarrierFacts {
                mcc: mcc.clone(),
                mnc: mnc.clone(),
                ..ImportedCarrierFacts::default()
            });
        if entry.ims_apn.is_none() {
            entry.ims_apn = element.get("apn").cloned().filter(|v| !v.is_empty());
        }
        if entry.brand.is_none() {
            entry.brand = element.get("carrier").cloned().filter(|v| !v.is_empty());
        }
    }
    by_plmn.into_values().collect()
}

/// Parse an AOSP CarrierConfig XML fragment for one carrier.
///
/// `mcc`/`mnc` are supplied by the caller because CarrierConfig files are named
/// by PLMN rather than carrying it inside the document.
pub fn parse_aosp_carrier_config(xml: &str, mcc: &str, mnc: &str) -> ImportedCarrierFacts {
    let mut facts = ImportedCarrierFacts {
        mcc: mcc.to_string(),
        mnc: mnc.to_string(),
        ..ImportedCarrierFacts::default()
    };
    for element in xml_elements(xml, "boolean") {
        let Some(name) = element.get("name") else {
            continue;
        };
        let value = element
            .get("value")
            .map(|value| value.eq_ignore_ascii_case("true"));
        if name == "carrier_wfc_supported_bool" || name == "carrier_volte_available_bool" {
            if let Some(value) = value {
                // Either key being true means the carrier does IMS over WLAN in
                // at least one mode; only overwrite with a positive answer so
                // two keys disagreeing cannot flip it back off.
                facts.vowifi_supported = Some(facts.vowifi_supported.unwrap_or(false) || value);
            }
        }
    }
    for element in xml_elements(xml, "string") {
        let Some(name) = element.get("name") else {
            continue;
        };
        if name.contains("entitlement") || name.contains("wfc_emergency_address") {
            if let Some(value) = element
                .get("value")
                .filter(|value| value.starts_with("http"))
            {
                facts.entitlement_url = Some(value.clone());
            }
        }
    }
    facts
}

/// Parse the `carrier.plist` of an Apple IPCC bundle (XML plist form).
///
/// IPCC bundles are the operator's own copy; SimAdmin neither ships nor fetches
/// them. Only the fields that are actually usable are read.
pub fn parse_ipcc_carrier_plist(plist_xml: &str, mcc: &str, mnc: &str) -> ImportedCarrierFacts {
    let mut facts = ImportedCarrierFacts {
        mcc: mcc.to_string(),
        mnc: mnc.to_string(),
        ..ImportedCarrierFacts::default()
    };
    if let Some(name) = plist_string_for_key(plist_xml, "CarrierName") {
        facts.brand = Some(name);
    }
    if let Some(url) = plist_string_for_key(plist_xml, "EntitlementServerURL") {
        facts.entitlement_url = Some(url);
    }
    for key in ["WiFiCallingEnabled", "WifiCallingEnabled"] {
        if let Some(enabled) = plist_bool_for_key(plist_xml, key) {
            facts.vowifi_supported = Some(enabled);
            break;
        }
    }
    if facts.ims_apn.is_none() {
        if let Some(apn) = plist_string_for_key(plist_xml, "ims") {
            facts.ims_apn = Some(apn);
        }
    }
    facts
}

/// Value of `<key>NAME</key><string>VALUE</string>` in an XML plist.
fn plist_string_for_key(xml: &str, key: &str) -> Option<String> {
    let needle = format!("<key>{key}</key>");
    let after = xml.split_once(&needle)?.1;
    let start = after.find("<string>")? + "<string>".len();
    let end = after[start..].find("</string>")?;
    let value = after[start..start + end].trim();
    if value.is_empty() {
        None
    } else {
        Some(decode_xml_entities(value))
    }
}

/// Value of `<key>NAME</key><true/>` or `<false/>` in an XML plist.
fn plist_bool_for_key(xml: &str, key: &str) -> Option<bool> {
    let needle = format!("<key>{key}</key>");
    let after = xml.split_once(&needle)?.1;
    let trimmed = after.trim_start();
    if trimmed.starts_with("<true/>") {
        Some(true)
    } else if trimmed.starts_with("<false/>") {
        Some(false)
    } else {
        None
    }
}

/// Minimal attribute scraper for flat XML elements.
///
/// A full XML parser is unnecessary here: both AOSP formats are machine
/// generated, flat, and attribute-only. This deliberately does not try to be a
/// general parser — it extracts `<tag a="1" b="2"/>` attribute maps and nothing
/// else.
fn xml_elements(xml: &str, tag: &str) -> Vec<BTreeMap<String, String>> {
    let mut out = Vec::new();
    let open = format!("<{tag}");
    let mut rest = xml;
    while let Some(index) = rest.find(&open) {
        let after = &rest[index + open.len()..];
        // Guard against `<apnX` matching when looking for `<apn`.
        if !after
            .chars()
            .next()
            .is_some_and(|c| c.is_whitespace() || c == '>' || c == '/')
        {
            rest = after;
            continue;
        }
        let Some(end) = after.find('>') else { break };
        out.push(parse_attributes(&after[..end]));
        rest = &after[end..];
    }
    out
}

fn parse_attributes(fragment: &str) -> BTreeMap<String, String> {
    let mut attributes = BTreeMap::new();
    let bytes = fragment.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        while index < bytes.len() && (bytes[index] as char).is_whitespace() {
            index += 1;
        }
        let name_start = index;
        while index < bytes.len() && bytes[index] != b'=' && !(bytes[index] as char).is_whitespace()
        {
            index += 1;
        }
        if name_start == index {
            break;
        }
        let name = fragment[name_start..index].trim().to_string();
        while index < bytes.len() && (bytes[index] as char).is_whitespace() {
            index += 1;
        }
        if index >= bytes.len() || bytes[index] != b'=' {
            continue;
        }
        index += 1;
        while index < bytes.len() && (bytes[index] as char).is_whitespace() {
            index += 1;
        }
        if index >= bytes.len() || (bytes[index] != b'"' && bytes[index] != b'\'') {
            continue;
        }
        let quote = bytes[index];
        index += 1;
        let value_start = index;
        while index < bytes.len() && bytes[index] != quote {
            index += 1;
        }
        let value = fragment[value_start..index.min(fragment.len())].to_string();
        index += 1;
        if !name.is_empty() {
            attributes.insert(name, decode_xml_entities(&value));
        }
    }
    attributes
}

fn decode_xml_entities(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

#[cfg(test)]
mod tests {
    use super::*;

    const APNS_XML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<apns version="8">
  <apn carrier="EE Internet" mcc="234" mnc="33" apn="everywhere" type="default,supl" />
  <apn carrier="EE IMS" mcc="234" mnc="33" apn="ims" type="ims" />
  <apn carrier="Unicom IMS" mcc="460" mnc="01" apn="cmims" type="ims,emergency" />
  <apn carrier="Other" mcc="310" mnc="260" apn="fast.t-mobile.com" type="default" />
</apns>"#;

    #[test]
    fn aosp_apns_import_keeps_only_ims_bearers() {
        let facts = parse_aosp_apns(APNS_XML);
        let plmns = facts.iter().map(|f| f.plmn()).collect::<Vec<_>>();
        assert_eq!(plmns, vec!["23433", "46001"], "310260 has no ims apn");
        let ee = facts.iter().find(|f| f.plmn() == "23433").unwrap();
        assert_eq!(ee.ims_apn.as_deref(), Some("ims"));
        assert_eq!(ee.brand.as_deref(), Some("EE IMS"));
    }

    #[test]
    fn imported_facts_start_from_the_derived_profile() {
        let facts = ImportedCarrierFacts {
            mcc: "460".to_string(),
            mnc: "01".to_string(),
            brand: Some("China Unicom".to_string()),
            ims_apn: Some("cmims".to_string()),
            vowifi_supported: Some(true),
            entitlement_url: None,
        };
        let record = facts.to_record().expect("record");
        record.validate().expect("valid");
        // ePDG and IMS names come from derivation, not from the import.
        assert_eq!(
            record.epdg.host,
            "epdg.epc.mnc001.mcc460.pub.3gppnetwork.org"
        );
        assert_eq!(record.ims.domain, "ims.mnc001.mcc460.3gppnetwork.org");
        // The APN and brand come from the import.
        assert_eq!(record.epdg.apn.as_deref(), Some("cmims"));
        assert_eq!(record.meta.brand, "China Unicom");
        assert_eq!(record.meta.plmn, "46001");
    }

    #[test]
    fn malformed_plmn_is_rejected_rather_than_producing_a_bad_profile() {
        let facts = ImportedCarrierFacts {
            mcc: "46".to_string(),
            mnc: "01".to_string(),
            ..ImportedCarrierFacts::default()
        };
        assert!(facts.to_record().is_none());
    }

    #[test]
    fn carrier_config_reads_wfc_support_and_entitlement_url() {
        let xml = r#"
        <carrier_config>
          <boolean name="carrier_wfc_supported_bool" value="true" />
          <boolean name="carrier_volte_available_bool" value="false" />
          <string name="wfc_emergency_address_country_codes_string">us</string>
          <string name="imsservice_entitlement_url_string" value="https://ent.example.net/" />
        </carrier_config>"#;
        let facts = parse_aosp_carrier_config(xml, "310", "260");
        assert_eq!(facts.vowifi_supported, Some(true));
        assert_eq!(
            facts.entitlement_url.as_deref(),
            Some("https://ent.example.net/")
        );
    }

    #[test]
    fn ipcc_plist_reads_name_entitlement_and_wifi_calling_flag() {
        let plist = r#"<?xml version="1.0"?>
        <plist version="1.0"><dict>
          <key>CarrierName</key><string>T-Mobile</string>
          <key>EntitlementServerURL</key><string>https://ent.t-mobile.example/</string>
          <key>WiFiCallingEnabled</key><true/>
        </dict></plist>"#;
        let facts = parse_ipcc_carrier_plist(plist, "310", "260");
        assert_eq!(facts.brand.as_deref(), Some("T-Mobile"));
        assert_eq!(
            facts.entitlement_url.as_deref(),
            Some("https://ent.t-mobile.example/")
        );
        assert_eq!(facts.vowifi_supported, Some(true));

        // An entitlement URL implies E911 is in play, and the host policy is
        // auto-filled so the value is actionable.
        let record = facts.to_record().expect("record");
        assert!(record.e911.enabled);
        assert_eq!(
            record.e911.websheet_host_policy.as_deref(),
            Some("public_https")
        );
    }

    #[test]
    fn attribute_scraper_handles_entities_and_similar_tag_names() {
        let xml = r#"<apnsettings mcc="1" /><apn mcc="234" mnc="33" apn="a&amp;b" type="ims" />"#;
        let facts = parse_aosp_apns(xml);
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].ims_apn.as_deref(), Some("a&b"));
    }
}

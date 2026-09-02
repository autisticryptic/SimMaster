# VoWiFi registration, refresh, and no-database fallback audit

Updated: 2026-09-02

This document records behavior that can be verified in SimAdmin, constraints
from public specifications, and items that still require confirmation on the
QCM410 device. A standards-derived value is not proof that an operator accepts
that value.

## Vodafone Germany test boundary

Vodafone Germany must be tested with UDP only:

- IKE_SA_INIT: UDP/500.
- NAT-T and IKE_AUTH: UDP/4500.
- IMS REGISTER, refresh REGISTER, OPTIONS, and keepalive: SIP over UDP.
- TCP must not be used as a probe or as evidence that this operator failed.

The runtime now forces SIP transport to UDP for a Vodafone profile whose country
is Germany. Other carriers may still use an explicitly configured TCP profile.

## Behavior implemented in code

### Address-family fallback

The default bearer order is `ipv4v6 -> ipv6 -> ipv4`. A single-family retry is
selected immediately only when the network reports an explicit family
requirement. A timeout, authentication failure, proposal failure, or generic
IKE Notify does not imply IPv4 or IPv6. The family that completes IKE is saved
and used for inner-address and P-CSCF selection.

### Fragmentation and reassembly

The full SIP REGISTER is preserved. Fragmentation is performed at the outer IP
layer. IPv4 and IPv6 fragments are checked for offset, identification,
continuity, overlap, and valid non-final payload alignment before reassembly.
This avoids using a compact REGISTER merely to hide an MTU problem.

### REGISTER refresh

Contact `expires` has priority over the response `Expires` header, followed by
the profile default. The shared lease schedules refresh at eleven twelfths of
the negotiated lifetime. VoWiFi refresh invalidates only IMS registration
readiness and keeps ePDG, IKE, Child-SA, ESP, and TUN state. Access rebuild is
deferred until the configured consecutive-refresh-failure threshold is met.
VoLTE refresh runs in its live loop using the same lease model. OPTIONS timeout
is advisory and does not by itself declare REGISTER expired.

## No-database derived profile boundary

When no usable database or catalog record exists, the fallback may use a
validated home PLMN to derive the standard `ims` APN, operator-identifier ePDG
FQDN, IMS home domain, EAP-AKA permanent NAI realm, UDP transport,
interoperable IKE/ESP proposal sets, and a conservative IMS REGISTER envelope.
Database search remains separate and must never display a guessed profile as a
stored operator record.

Private deployment values must not be invented from PLMN alone. Examples are a
real P-CSCF, private ePDG address, operator DNS, AKA identity template, private
IPsec ports, and non-standard security negotiation. Missing values should be
reported with a stage-specific log reason.

The runtime source ladder is shared by VoLTE and VoWiFi:

1. A source-bound custom database record, if it is valid for the requested
   access.
2. A source-bound read-only carrier-catalog projection.
3. A marked standard-derived profile for the validated HPLMN.

An IMS-only custom database record remains usable by VoLTE, but
`voice.vowifi_enabled = false` prevents it from entering the VoWiFi resolver,
the published live matcher, or an explicitly pinned Wi-Fi access. The database
row remains visible in stored-profile search as not VoWiFi-ready. After
ePDG/IKE/ESP succeeds, the selected Wi-Fi profile's IMS domain, realm,
registrar, P-CSCF and REGISTER policy feed the existing shared IMS REGISTER
driver. The per-SIM `ims_vowifi.profile_id` compatibility pin is passed through
the same source-bound resolver as the VoLTE pin; a database pin is considered
only during the database slot and cannot silently select a same-named catalog
row.

## Public specification checklist

- 3GPP TS 23.003: PLMN, IMSI, ePDG FQDN, and related identifiers.
- 3GPP TS 24.302: WLAN access, ePDG discovery, and selection.
- 3GPP TS 24.229: IMS SIP REGISTER, access-network information, and MMTEL.
- 3GPP TS 33.203: IMS IPsec and Security-Client, Security-Server, and
  Security-Verify negotiation.
- RFC 7296: IKEv2 exchange and NAT-T on UDP/4500.
- RFC 3261: SIP REGISTER, Expires, and 423/Min-Expires handling.
- RFC 3329: SIP security agreement.
- RFC 5626: instance identity, reg-id, flow, and keepalive extensions.

## Requirement-to-code review

| Source | Requirement applied | Implementation and remaining boundary |
|---|---|---|
| 3GPP TS 23.003 | Three-digit MNC labels in public 3GPP names; standard IMS home domain, operator ePDG FQDN and EAP-AKA realm | `profiles.rs` owns all standard naming helpers. MCC 999 and malformed or ambiguous HPLMNs are rejected rather than sent to public DNS. |
| 3GPP TS 24.302 | Prefer provisioned/UICC ePDG selection information and use operator-identifier discovery only as a fallback | `live.rs` orders explicit line, UICC selection/home identity, visited-country result, stored profile and derived candidates. Location-based names require an explicit UICC selection rule. |
| 3GPP TS 24.229 | Register over the selected access, preserve IMS identities and access information, and refresh the negotiated binding | Database/catalog and derived profiles enter the same REGISTER driver. PANI/CNI and MMTEL are policy-gated; Contact `expires` controls refresh. |
| 3GPP TS 33.203 Annex H | Negotiate IMS access security; the first protected request repeats Security-Client and carries Security-Verify | Security-Client/Server/Verify and protected REGISTER are implemented. SPI, port and algorithm values come from the negotiated offer; a server SA tuple or operator-specific mechanism is never synthesized. |
| [RFC 7296](https://www.rfc-editor.org/rfc/rfc7296) | IKEv2 retransmission and NAT traversal on UDP/4500 | The IKE exchange retransmits within bounds, switches to NAT-T when negotiated, and reuses the Child SA/TUN for REGISTER refresh. |
| [RFC 3261](https://www.rfc-editor.org/rfc/rfc3261) sections 10.2.1.1 and 21.4.17 | Per-Contact expiry and bounded 423 retry using Min-Expires | Shared `connectivity/core/register.rs` handles 423/Min-Expires; registration artifacts prefer Contact expiry over the response-wide Expires value. |
| [RFC 3329](https://www.rfc-editor.org/rfc/rfc3329) sections 2.2-2.3 | Security-Client offer, Security-Server selection and matching Security-Verify | The initial and authenticated REGISTER paths implement this negotiation; 494 may enable the bounded sec-agree variant without guessing SA values. |
| [RFC 5626](https://www.rfc-editor.org/rfc/rfc5626) sections 4.1-4.2 | Stable `+sip.instance`; `reg-id` only when requesting and maintaining an outbound flow | A configured stable instance is advertised. The derived profile does not claim `outbound` or invent `reg-id`, because the required outbound-proxy flow contract is absent. |

The safe implementation gap found in this review was not a new guessed wire
value; it was access qualification in profile selection. The duplicated
VoLTE/VoWiFi source ladders could diverge, and legacy generic lookup/publication
could expose an IMS-only custom row to VoWiFi. They now share one access-aware
candidate resolver and one custom-profile readiness predicate.

## QCM410 evidence still required

Initial Vodafone Germany VoWiFi and VoLTE registration, one VoWiFi refresh,
and one VoLTE refresh have returned 200 over UDP. The remaining device work is:

1. Observe another negotiated refresh or reconnect cycle to establish that the
   result is repeatable over time.
2. Force consecutive refresh failure, missing UE socket, and stale TUN/veth
   conditions and verify the bounded rebuild path.
3. Confirm that subscription, roaming, or private-profile rejection remains
   distinguishable from local fragmentation and address-family failures.

The final interoperability result must be based on QCM410 UDP logs and SIP
responses, not on a standards inference alone.

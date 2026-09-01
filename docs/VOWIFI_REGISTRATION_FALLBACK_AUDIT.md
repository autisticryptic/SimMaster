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

When no database record exists, the fallback may derive PLMN, APN, ePDG FQDN,
IMS domain, EAP-AKA NAI realm, UDP transport, conservative IKE and ESP
proposal sets, and standard IMS headers. Database search remains separate and
must never display a guessed profile as a stored operator record.

Private deployment values must not be invented from PLMN alone. Examples are a
real P-CSCF, private ePDG address, operator DNS, AKA identity template, private
IPsec ports, and non-standard security negotiation. Missing values should be
reported with a stage-specific log reason.

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

## QCM410 evidence still required

1. Vodafone Germany full UDP REGISTER returns 200.
2. Contact expiry is parsed and used for the next refresh.
3. VoWiFi refresh returns 200 over the existing ESP and TUN path.
4. VoLTE refresh succeeds before its negotiated lease expires.
5. Any subscription, roaming, or private-profile rejection is distinguished
   from local fragmentation and address-family failures.

The final interoperability result must be based on QCM410 UDP logs and SIP
responses, not on a standards inference alone.

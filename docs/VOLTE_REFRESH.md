# Protected VoLTE REGISTER refresh

## Failure investigated

The QCM410 trace on 2026-09-05 showed that the network did answer a protected
refresh with 401. The subsequent authenticated REGISTER, not the original
refresh, timed out:

1. SM1 repeated the active Security-Client ports and SPIs.
2. The 401 contained an unusable Security-Server (spi-c=1, spi-s=0).
3. The authenticator allocated different UE ports/SPIs **after** this response,
   installed an outbound ESP SA with SPI zero, and sent SM7 with an offer the
   P-CSCF had not seen in SM1.
4. Failure left the live channel using tentative sockets while the session
   still advertised its previous binding. Further retries mixed both states.
5. Two timeouts triggered an unprotected REGISTER. Its successful initial
   registration concealed the defective refresh path.

A separate sample had no response at all. A timeout alone is not proof that
headers should change or that the existing association should be discarded.

## Follow-up: registration identity versus originating identity

The first fix (89bd812 / v1.1.5) passed the hardware-free regressions, but the
2026-09-06 natural refresh still received no response to SM1. It must not be
reported as a successful device refresh just because initial registration works.

A complete initial exchange then exposed a separate request-construction bug:
REGISTER CSeq 1/2 used the USIM-derived temporary IMPU. The 200 returned an
MSISDN-based default in P-Associated-URI. The session overwrote its sole identity
with that default, so refresh CSeq 3 (and deregistration) changed the registered
From/To AoR while retaining the original Contact, Call-ID and security context.

TS 24.229 5.1.1.1A and 5.1.1.4.1 distinguish the derived registration identity
from the default public identity selected for originating services. The session
now stores them separately:

- `registration_identity` is fixed when initial registration succeeds. All
  refresh requests, challenged authentication rounds and deregistration use it.
- `identity` follows P-Associated-URI for calls, SMS, subscriptions and OPTIONS.
  A later 200 can update this default without changing the registered AoR.
- Contact user, Call-ID/CSeq and the protected transport retain their existing
  lifecycle. This change does not trigger or add IPsec renegotiation.

Regression coverage checks two consecutive refreshes with changing network
aliases, challenged REGISTER identity, original-AoR deregistration, and timeout
retries. Real-carrier validation must still observe a complete natural refresh;
a local mock 200 is not sufficient evidence that carrier timeouts are resolved.

## Device validation and the dual-access comparison

A bounded comparison on 2026-09-06 temporarily disabled VoWiFi, without changing
VoLTE's refresh schedule or the configured `concurrent` preference. With
89bd812 / v1.1.5 still deployed, the natural refresh at 06:31:36 received a
protected direct 200 at 06:31:37 (0.573 seconds). The original outbound and
inbound SAs were retained; the refresh counter advanced to one. VoWiFi was
then re-enabled and the restoration watchdog was cancelled.

That successful request still contained the old identity-mixing defect.
Consequently identity preservation is a protocol correctness fix, **not yet
proof of the root cause of the earlier dual-access timeouts**. OPTIONS silence
is also insufficient evidence of a dead SA: OPTIONS remained unanswered in
the same isolated session whose natural REGISTER refresh succeeded.

The tested 98f9459 / v1.1.6 ARM64 Release was deployed with the original
concurrent LTE/WLAN settings restored. LTE initial registration succeeded at
06:42:33, but its natural refresh at 07:24:54 received no response on the wire;
protected retries CSeq 4 through 12 also went unanswered. The registered AoR,
Contact and old SA were preserved. Only after the lease expired did a new
initial registration succeed at 07:36:11. This is expiration recovery, **not a
successful refresh**. The next natural refresh at 08:22:56 was also unanswered.
Passive metadata is scoped to the cellular interface because the decrypted
inner VoWiFi SIP traffic is not cellular ESP.

A bounded WLAN profile probe at 08:14 added `Supported: outbound`, retaining
the same instance ID and the distinct WLAN `reg-id`. Its 200 response contained
neither `Require: outbound` nor `Flow-Timer`. This did not establish outbound
negotiation or repair coexistence. The temporary profile was removed at 08:19;
normal profile selection, both enabled intents and the original `concurrent`
preference were restored, and the rollback timer was cancelled.

TS 24.229 5.1.1.2.1(f) requires a UE advertising multiple registration with
`reg-id`, `+sip.instance` and `Supported: outbound` to inspect the successful
response for `Require: outbound`. If absent, the UE must treat RFC 5626 multiple
registration as unsupported and refrain from registering additional IMS flows
for that private identity. The same clause describes replacement of the old
contact and network-initiated deregistration in the non-outbound case. RFC 5626
section 6 additionally requires first-hop/Path support. Distinct `reg-id` values
and two initial 200 responses alone are therefore insufficient evidence that
two independent registrations remain valid concurrently.

The current coexistence implementation lacks this negotiated-capability gate.
Binding replacement/old-SA removal is a plausible explanation of the device
comparison, but carrier-side binding/SA removal has not been observed directly;
a data-plane access interaction has not been excluded. Do not report this
hypothesis as the proven timeout cause, blindly add an outbound token, silently
change instance IDs, or permanently disable an access to claim refresh success.

## Correct transaction boundary

TS 33.203 sections 7.4, 7.4.1a and 7.4.2a distinguish a protected refresh from a
network-challenged authentication. The latter can establish replacement SAs;
this is not the same thing as a timeout-driven unprotected restart.

- SM1 always uses the current protected channel and Security-Verify. Its
  Security-Client already offers fresh client ports/SPIs; the UE server port
  stays fixed. Reserved sockets keep the advertised ports available.
- A direct 200 extends the lease and discards the unused offer. No SA changes.
- A usable AKA/Security-Server challenge uses exactly that offer for SM7.
  Server ports remain stable; client ports and SPIs change. Overlapping or
  zero-valued bindings are rejected before installing a replacement plan.
- Tentative channel changes retain the old sockets. Old-SA failure responses
  and unrelated SIP frames remain readable during authentication.
- Failure restores sockets, advertised route, Security-Verify and the original
  authentication context together. CSeq and consumed original nonce-counts
  still advance. Failed new challenge state is not committed.
- Only final success commits new SAs and credentials. One preceding protected
  association remains readable for delayed network traffic and is released
  before the next registration procedure; session teardown removes both plans.
- Repeated timeouts retry the same protected channel while the registration
  lease is valid. There is no timeout counter that switches refresh to plain
  UDP. An actually expired lease is handled as a new registration, not a
  successful refresh.

The carrier-provided lease scheduling is unchanged. Local SA installation is
still scoped to the UE worker; cleanup never globally flushes XFRM.

## Verification

Build-Release executes hardware-free regressions on GitHub Actions and gates
publication on their success. Only the final release job publishes assets; the
earlier ARM64-only publish shortcut was removed because it bypassed this gate. Coverage includes frozen offers, rollback of
sockets and authentication state, zero SPI rejection, disjoint XFRM plans,
direct-200 refresh, reception on old/new associations during rollover, and
three consecutive real Timer E/F timeouts without plaintext fallback.

The device must use the ARM64 Release artifact whose `meta.json` commit matches
the tested source. Device acceptance requires observing the normal carrier
refresh interval, not shortening it to 120 seconds. Inspect the entire
SM1 -> 401 (if any) -> SM7 -> 200 exchange, actual UDP ports/ESP SPIs, CSeq,
refresh counters and absence of timeout-triggered initial registration.
Do not persist or log AKA material, raw authorization secrets or XFRM keys.


## Coexistence admission correction (v1.1.7)

The implementation and its limits are documented in
[IMS_ACCESS_COEXISTENCE.md](IMS_ACCESS_COEXISTENCE.md). Both enable intents and
`concurrent` preference are retained, but the effective mode is one coordinated
registration until complete client/network outbound support exists. This avoids
starting an unconfirmed competing registration; it does not reinterpret a
protected refresh timeout as successful IPsec renegotiation. Device validation
of this release must still observe the natural refresh on the selected LTE leg.

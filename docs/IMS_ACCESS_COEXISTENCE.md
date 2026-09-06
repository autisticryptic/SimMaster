# IMS access coexistence and registration admission

## Scope of v1.1.7

This release corrects **unconfirmed parallel registrations**, not all causes of
REGISTER timeout, and does not implement full RFC 5626 outbound. VoLTE and
VoWiFi may both remain enabled, but an enabled intent is not a valid IMS binding.
The effective policy is observable instead of treating two cached `registered`
flags as proof of standards-compliant concurrency.

`concurrent` remains the stored/default user preference. The current client
reports `client_incomplete` because it lacks complete UDP outbound flow
maintenance (STUN, negotiated flow timers and flow recovery). It therefore
coordinates **one** IMS registration. No token-only `Supported: outbound` change
is made. This does not establish that the operator cannot support outbound with
a capable client.

## Standards basis

- [3GPP TS 24.229 / ETSI TS 124 229 V16.7.0](https://www.etsi.org/deliver/etsi_ts/124200_124299/124229/16.07.00_60/ts_124229v160700p.pdf),
  section 5.1.1.2.1(f): after REGISTER with `reg-id`, `+sip.instance` and
  `Supported: outbound`, check the successful response for `Require: outbound`.
  If it is absent, the registrar did not use the RFC 5626 procedure; the UE must
  refrain from registering additional IMS flows for the same private identity.
  An old contact may have been replaced (note 13 describes reg-event NOTIFY).
- [RFC 5626 section 4.2](https://www.rfc-editor.org/rfc/rfc5626#section-4.2):
  stable instance identity and distinct reg-id values distinguish flows; they
  are necessary, not sufficient evidence of negotiation.
- [RFC 5626 section 4.4.2](https://www.rfc-editor.org/rfc/rfc5626#section-4.4.2):
  UDP outbound requires STUN-based keepalives. Do not send STUN blindly before
  first-hop/network support has been established (section 4.3).
- [RFC 5626 section 6](https://www.rfc-editor.org/rfc/rfc5626#section-6):
  outbound registrar behavior depends on first-hop Path/ob support and successful
  negotiation, not simply receiving a Contact reg-id parameter.

This implementation is **break-before-make registration selection/fallback**.
It is not seamless IR.51 handover, does not transfer an active call between
accesses, and cannot promise instantaneous fallback or terminating-call delivery.

## Selection and lifecycle

1. Preserve both enable flags and the requested preference.
2. With unconfirmed concurrency, retain an existing valid single registration.
   At a cold start where both can be attempted, prefer cellular. A working WLAN
   fallback stays selected when LTE merely reappears, avoiding registration
   ping-pong. Explicit `cellular_preferred` / `wlan_preferred` can request a switch.
3. A modem merely being present is not enough: normal cellular eligibility
   requires network registration and a non-exhausted, safe recovery budget.
   WLAN gets a bounded attempt; exhausted recovery makes the fallback eligible.
   An explicit retry can retry a failed target, not bypass registration policy.
4. The proactive refresh deadline is **not** lease expiry. Valid registrations
   remain eligible during protected refresh retries. VoLTE refresh is not gated
   on the opposite access's enabled intent and retains the existing protected
   transaction/security behavior described in [VOLTE_REFRESH.md](VOLTE_REFRESH.md).
5. Serialize per-line LTE/WLAN registration transitions before bearer/access
   locks. Re-evaluate the policy after taking the lock, drain admitted REGISTER
   transactions, tear down the opposite path, then authorize the selected one.
   The low-level LTE/WLAN REGISTER entry points fail closed even if a diagnostic,
   SMS or voice caller bypasses the normal restore workflow. Different SIM lines
   have independent locks and admission decisions.
6. Reconcile already connected paths every scheduler pass and on preference
   updates. Calls in dialing, ringing, incoming, active, held or unknown
   nonterminal states defer policy teardown; check again after draining a
   transaction and acquiring the access lock. During deferral retain the old
   applied decision so the old leg can refresh, not the new leg register. These
   are repeated application call-state checks, not an atomic incoming-call
   barrier or a guarantee of seamless in-call access transfer.
7. A parked access is not a failed profile attempt and does not spend its
   recovery budget. Parking only tears down that IMS/ePDG path; it does not
   silently edit radio/data configuration or the other SIM's runtime. Resource
   cleanup preserves an exhausted primary's retry marker, so it cannot instantly
   steal admission back from the fallback.

## Observability

`GET /api/modem/lines/{line}/ims/status` adds `registration_policy`:

- `requested`: original user preference;
- `effective`: applied `single_registration`, `concurrent`, or `none`;
- `desired` / `applied`: per-access admission and stable reason codes;
- `concurrent_support`: `client_incomplete`, `not_negotiated`, or `negotiated`;
- `switch_deferred_for_call`;
- each access's last successful response `require_outbound` and `flow_timer_seconds`.

Response metadata is passive evidence only. `Supported: outbound`, a substring
like `x-outbound`, or reg-id alone never enables concurrency. The dashboard
shows the effective single-registration selection and explains the limitation.

## Evidence and acceptance

See [VOLTE_REFRESH.md](VOLTE_REFRESH.md) for the September 6 isolated-vs-concurrent
trace and the controlled outbound probe. They demonstrate an implementation
capability-gate defect and a coexistence correlation; they **do not prove** that
the network deleted a specific LTE binding or SA. APN/ePDG data-plane interaction
has not been excluded.

Build/test only through GitHub Actions. Added regressions cover the capability
gate, all boolean single-registration combinations, full lease versus refresh
deadline, protected-refresh eligibility, fallback stickiness, deferred calls,
per-line serialization and fail-closed admission, exact Require parsing, and
not exhausting a parked access's recovery budget.

Deploy only a successful Release artifact after checking there are no calls.
Retain `concurrent` and both enabled intents during validation, but report the
**effective single-registration mode honestly**. Acceptance requires a natural
LTE REGISTER refresh and a protected 200 response on the existing session; a new
initial registration after expiry or access teardown is not refresh success.
Real dual-registration support remains future work; do not label this test as
parallel VoLTE/VoWiFi registration success.

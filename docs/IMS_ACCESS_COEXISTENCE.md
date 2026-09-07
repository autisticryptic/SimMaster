# IMS access coexistence and registration admission

## Implementation and validation status

The client implements negotiated LTE/WLAN IMS flows: a shared `+sip.instance`,
distinct stable `reg-id` values, binding-specific leases, UDP STUN / TCP CRLF
keepalives, and per-flow recovery. VoLTE and VoWiFi may both remain enabled, but
enabled intent is not a valid IMS binding. Two cached `registered` flags are
not proof of concurrency or inbound reachability.

`concurrent` remains the stored/default user preference. The bootstrap state is
`not_negotiated`, not `client_incomplete`. An owned, unexpired flow must prove
outbound negotiation and transport health before admission opens for a **new**
second access. Those are separate facts: a temporarily expired keepalive proof
does not erase an existing binding's negotiated capability or authorize policy
teardown. Without negotiation, the client retains **one** IMS registration. A
legacy registration on the tested network does not establish that every access,
P-CSCF or carrier configuration lacks outbound support.

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
- [RFC 3261 section 7.3.1](https://www.rfc-editor.org/rfc/rfc3261#section-7.3.1):
  comma-list header fields may be combined without changing their meaning.
  Emit one `Supported` and one `Require`, preserving existing security/GRUU
  options, so a peer that reads only the first row cannot miss an appended
  outbound option. Do not combine digest authentication header fields.
- TS 24.229 K.2.1.5: positively observed no-NAT UDP paths may omit keepalives;
  an explicit `Flow-Timer` is still honored. An `ob` URI parameter by itself
  is not proof that an IMS P-CSCF implements STUN.

The non-negotiated fallback uses **break-before-make registration selection**.
Negotiated concurrency keeps both bindings and refresh owners; a failed refresh
does not itself authorize destroying the other access. Neither mode implements
seamless IR.51 handover or transfers an active call between accesses.

## Selection and lifecycle

1. Preserve both enable flags and the requested preference.
2. With unconfirmed concurrency, prefer WLAN whenever eligible, deferring a
   switch during a call. Exhausted WLAN recovery permits the cellular fallback.
   Explicit `cellular_preferred` / `wlan_preferred` can request a selection.
   Registration admission is distinct from the business path priority:
   VoWiFi, then 4G/5G IMS, then CS where configured/available.
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
   transactions, release only a path no longer admitted, then publish the
   selection. A negotiated second flow does not tear down the first.
   The low-level LTE/WLAN REGISTER entry points fail closed even if a diagnostic,
   SMS or voice caller bypasses the normal restore workflow. Different SIM lines
   have independent locks and admission decisions.
   A protected security rollover validates and publishes the new owned flow
   before retiring its predecessor, avoiding a transient capability gap.
   A staged refresh retains the original outbound requirement even when there
   is no second access; a challenged 200 cannot silently downgrade the binding.
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
- `concurrent_support`: `client_incomplete`, `not_negotiated`, `not_supported`,
  or `negotiated`. `not_supported` means a **current owned successful flow**
  offered outbound but its response did not accept it; it is not inferred from
  a timeout, and disappears when that flow is removed. `negotiated` alone is
  not proof of current transport health or that both accesses are registered;
- `switch_deferred_for_call`;
- each access's last successful response `require_outbound` and `flow_timer_seconds`.
- `cellular_flow` / `wlan_flow`: current owned binding lifetime, whether outbound
  was offered/negotiated, and separate `transport_validated` evidence. These
  fields contain no Contact, subscriber identity, authorization or SA keys.

Response metadata is passive evidence only. `Supported: outbound`, a substring
like `x-outbound`, or reg-id alone never enables concurrency. The dashboard
shows the applied selection and explains any negotiation limitation.

`IMS REGISTER outbound offer prepared` describes the final request after
capability completion, including authenticated retries, refresh and removal.
`IMS outbound registration flow accepted` distinguishes the offer, response
Require, first-hop Path, matching binding, NAT observation and flow timer. It
does not log identities, digest authorization, or IPsec key material.

## Evidence and acceptance

See [VOLTE_REFRESH.md](VOLTE_REFRESH.md) for the September 6 isolated-vs-concurrent
trace and the controlled outbound probe. They demonstrate an implementation
capability-gate defect and a coexistence correlation; they **do not prove** that
the network deleted a specific LTE binding or SA. APN/ePDG data-plane interaction
has not been excluded.

Build/test only through GitHub Actions. Added regressions cover the capability
gate, all boolean single-registration combinations, full lease versus refresh
deadline, protected-refresh eligibility, fallback stickiness, deferred calls,
per-line serialization and fail-closed admission, exact Require parsing,
independent live flows/keepalives, and not exhausting a parked access's recovery
budget. Cellular-first regressions then register WLAN and alternate protected
flow ownership through multiple refreshes, without borrowing the other lease.
Pending keepalive proof windows preserve existing dual admission but cannot
authorize a new second flow. Capability-field regressions cover initial/authenticated/refresh/remove
requests, retransmission byte stability, folded/compact/repeated header fields,
security-option preservation, and Contact echoes without outbound negotiation.

Deploy only a successful Release artifact after checking there are no calls.
Retain `concurrent` and both enabled intents during validation, but report the
**effective registration mode honestly**. Dual-flow acceptance requires both
negotiated bindings and a natural refresh on each existing flow, without
invalidating the other. A new initial registration after expiry or access
teardown is not refresh success. The fixed 120-second LTE test delay is disabled.

### September 6, 2026 device evidence (Asia/Shanghai)

On commit `0930fd9`, the 23:29:36 initial WLAN REGISTER offered `outbound` in
a second `Supported` row. The 23:29:38 successful response echoed the same
instance and `reg-id=2`, with a 3195-second Contact expiry, but lacked
`Require: outbound`; its Path also lacked `ob`. Therefore the current access
was registered but did not establish outbound concurrency.

Header coalescing is an interoperability change to test, **not a proven timeout
root cause**. The baseline metadata collector did not reassemble fragmented
authenticated requests, so absence of a field in those request snippets is
not evidence that the actual authenticated request omitted it. Subsequent
wire checks must reassemble IP fragments and accept only complete SIP headers.

### September 7 continuation

The requested acceptance remains **both registrations plus each flow's natural
refresh**, or an evidence-backed priority fallback where the current network
does not accept the required multi-registration procedure. Disabling cellular
IMS is not a dual-registration fix. Re-establishing security after failed
refresh is recovery, never counted as successful refresh.

The keepalive/admission and staged-flow publication fixes are client-side
correctness fixes. They are not yet a proven explanation for the older carrier
trace with no outbound negotiation. That trace must be tested with the corrected
final REGISTER before drawing a conclusion about this access.

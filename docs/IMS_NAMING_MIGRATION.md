# Cellular IMS naming migration (beta2)

The naming implementation shipped in beta2 candidate `2bb6099` after final
Actions run `34169025117`. It was deployed to 410 on 2026-09-08 at 07:34
(Asia/Shanghai). Implementation/contract tests and live business validation
remain distinct; the latter is not implied by a successful build.

## Implemented migration

- The access stack's `Volte*` type symbols now use `CellularIms*`. Shared
  profile source/candidate/selection/reference types used by **both** cellular
  and WLAN instead use `ImsProfile*`, not a misleading cellular-only name.
- Eight backend handler names and seven frontend client method names now use
  cellular IMS terminology. The frontend calls the new canonical endpoints.
- All seven canonical route groups have been added; the old routes still
  invoke exactly the same handlers. **JSON field names, enum wire values and
  persistent storage keys deliberately retain compatible serialized spellings.**
- A private-D-Bus HTTP test checks canonical/legacy response parity and ensures
  the new endpoints remain authenticated. It fails rather than silently
  skipping if its test bus is missing. It passed the final release workflow.
- The implementation directory/module is now `cellular_ims`; runtime
  fields/functions and frontend code identifiers have been renamed. New
  configuration aliases are accepted; externally stored/written keys remain
  compatible as described below.

## Wire/storage compatibility decision

This refactor does **not** require rewriting existing installations or breaking
old API clients. Rust fields/members use the canonical names, but serde retains
the old serialized key and accepts the new spelling as an input alias.
Examples:

- Rust `cellular_ims_connection_enabled`: reads either that key or
  `volte_connection_enabled`; serializes the established legacy key.
- Rust `ims_cellular`: accepts `ims_cellular` and `ims_volte`; stored SIM-bound
  envelopes likewise accept `ims.cellular_ims` and `ims.volte`.
- `AccessPathKind::CellularIms` accepts `"cellular_ims"` or `"volte"`; it retains
  `"volte"` / `"volte_ims"` as wire/history identifiers so existing SMS,
  notification and automation conditions continue to work.

Do not send both spellings of one setting: ambiguous duplicate keys are
rejected, not silently resolved. JSON DTO properties in the frontend and
historical error codes intentionally retain their compatibility spellings;
code variables, helpers, components and new route names use cellular IMS.
Actual VoLTE standard/vendor terms and protocol strings are not globally
rewritten. These intentional compatibility encodings are not missed renames.

## Meaning

`cellular_ims` means the 3GPP (4G/5G) IMS access/registration, which can carry
SMS, supplementary services and voice. VoLTE remains the correct name for
LTE voice capabilities in standards/vendor data. Visible access labels remain
the user's short **4G/5G**, not a longer internal identifier.

## Canonical names

| Current concept | Canonical name | Compatibility requirement |
| --- | --- | --- |
| IMS `volte` module | `cellular_ims` | Update internal imports and CI filters together; never rename vendor AT commands or protocol tokens |
| Registration/runtime `Volte*` types | `CellularIms*` | Restrict to this access stack; leave actual voice-specific standard/vendor terms alone |
| `LineRuntime.volte`, `volte_live`, locks | `cellular_ims`, `cellular_ims_live`, corresponding locks | Inventory/API compatibility must be deliberate, not silently broken |
| `volte_connection_enabled` | `cellular_ims_connection_enabled` | Read both spellings; keep established wire/storage key without disabling a line |
| `volte_auto_restore`, `volte_profile_selection` | corresponding `cellular_ims_*` | Preserve retries, ordered profile attempts and their sources |
| `volte_ip_families`, `volte_ip_families_auto` | corresponding `cellular_ims_*` | Preserve automatic/manual semantics and ordering |
| `AccessPathKind::Volte` | `AccessPathKind::CellularIms` | Accept old serialized `"volte"`; update voice/SMS/UI together |
| `ImsRegistrationAccess::Volte`, `EffectiveImsAccess::Volte` | corresponding `CellularIms` variants | Internal registration access, not LTE-only voice capability |
| `volte_ready` and equivalent access readiness | `cellular_ims_ready` | Do not conflate registration with voice/SMS capability |

Seven HTTP route groups provide canonical endpoints plus old endpoint aliases:

- `/api/volte/lines` → `/api/cellular-ims/lines`
- `/api/volte/lines/{line_id}` → `/api/cellular-ims/lines/{line_id}`
- `/profile-selection`, `/connection`, `/retry`, `/ip-families` under the above
- `/api/modem/lines/{line_id}/volte/call/status` →
  `/api/modem/lines/{line_id}/cellular-ims/call/status`

New and legacy routes share the same handlers and compatible response schema.
Tests check that contract rather than merely the presence of route aliases.

## Storage and safety

- Existing line profiles are JSON documents in `config_line_profiles`.
  Test aliases and unchanged re-serialization; do not gratuitously rewrite keys.
- Keep historical SQLite column names and counters unless an explicit,
  backward-tested migration is necessary. Rust field/method names can change
  independently from SQL column literals.
- Read old event/SMS transport labels; do not hide pre-upgrade history.
- Preserve compatibility for persisted error codes; UI helper names can change
  without rewriting every historical error string.
- Never replace `volte` in carrier database blobs, SIP feature tags, AT commands,
  LTE RAT data, arbitrary user strings or prior release history by substring.
- Keep VoWiFi → cellular IMS → CS ordering, enabled intents, binding ownership,
  leases and the beta1 protected-refresh behavior unchanged.

## Validation and remaining limits

- Final CI passed old/new alias load, compatible write/reload, duplicate-key
  rejection, API contracts, routing regressions, frontend lint/build/type-check
  and nonempty Rust test filters. No backend build/test ran locally.
- At 07:40/07:52 the live canonical/legacy APIs, profile selection and call
  status matched. Eight unauthenticated GET requests were rejected with 401.
  Complete stored line profiles and config.yaml matched the pre-upgrade backup.
- SMS/voice policy, enabled intent and IP-family settings were unchanged.
  Actual SMS, calls, supplementary-service operations and interactive history
  pages were **not** exercised. Readiness is not evidence of a completed call.
- The DNS-only correction `0b97b4f` passed final release workflow `34173433980`
  and was deployed at 08:43. At 08:54 the same live API/authentication/settings
  checks passed again, along with the actual application's HTTP DNS lookup.
  Naming/wire contracts were not changed by that correction.
- beta2 remains a pre-release, not latest. The first candidate's natural
  refresh passed at 08:25:52; the corrected candidate independently passed at
  09:31:28 (protected CSeq=3 → 200, 531 seconds left, renewed lease 3100
  seconds, original service/TUN). Field results belong in `plan.md`, never
  inferred from initial registration or another candidate's result.
  Current-network dual registration and unexercised live business paths remain
  unverified.
- The later registration-mode/cost-guard candidate `48e37fc` retains these
  compatibility contracts. Final CI `34197531701`, live API/settings checks
  at 16:23, and its own protected natural refresh at 16:47:58 passed after
  deployment. The current SMS/Trunk VoWiFi-only switches were not silently
  enabled or reset; operational details are in `docs/IMS_REGISTRATION_POLICY.md`.

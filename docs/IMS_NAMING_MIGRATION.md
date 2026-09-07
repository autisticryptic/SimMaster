# Cellular IMS naming migration (beta2 work list)

This is the implementation checklist, **not a claim that all renames below
have shipped**. Work remains isolated on `refactor/1.1.4-beta2`.

## Current incremental implementation

- DNS migration passed Actions run `34153925916` at `87eead1`.
- The access stack's `Volte*` type symbols now use `CellularIms*`. Shared
  profile source/candidate/selection/reference types used by **both** cellular
  and WLAN instead use `ImsProfile*`, not a misleading cellular-only name.
- Eight backend handler names and seven frontend client method names now use
  cellular IMS terminology. The frontend calls the new canonical endpoints.
- All seven canonical route groups have been added; the old routes still
  invoke exactly the same handlers. **JSON field names, enum wire values and
  persistent storage keys are deliberately unchanged in this first increment.**
- A private-D-Bus HTTP test checks canonical/legacy response parity and ensures
  the new endpoints remain authenticated. It fails rather than silently
  skipping if its test bus is missing. This increment still needs Actions.

The next increment moves the implementation directory/module to `cellular_ims`,
renames runtime fields/functions and frontend code identifiers, and accepts new
configuration aliases. The externally stored/written keys remain compatible as
described below. Full beta2 device validation is still required.

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

## Planned canonical names

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

Seven HTTP route groups need canonical endpoints plus old endpoint aliases:

- `/api/volte/lines` → `/api/cellular-ims/lines`
- `/api/volte/lines/{line_id}` → `/api/cellular-ims/lines/{line_id}`
- `/profile-selection`, `/connection`, `/retry`, `/ip-families` under the above
- `/api/modem/lines/{line_id}/volte/call/status` →
  `/api/modem/lines/{line_id}/cellular-ims/call/status`

New and legacy route responses must have an explicit compatibility contract.
Merely adding route aliases does not make a changed JSON schema compatible.

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

## Required validation

- Old/new alias fixture load, compatible write/reload, duplicate-key rejection.
- New and legacy HTTP routes, request keys and response schemas.
- SMS/voice/supplementary routing and old/new activity history display.
- Frontend lint/build/type-check and nonempty Rust regression filters on Actions.
- Only after implementation and 410 validation: `1.1.4-beta2`, pre-release and
  not latest. The running beta1 device is not upgraded by development CI.

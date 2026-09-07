# Cellular IMS naming migration (beta2 work list)

This is the implementation checklist, **not a claim that the renames below
have already shipped**. DNS migration is being validated separately first.

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
| `volte_connection_enabled` | `cellular_ims_connection_enabled` | Read old saved key; write canonical key without disabling an existing line |
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
  Add and test deserialization aliases before writing new canonical keys.
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

- Old config/DB fixture load, canonical write, reload, no duplicate alias keys.
- New and legacy HTTP routes, request keys and response schemas.
- SMS/voice/supplementary routing and old/new activity history display.
- Frontend lint/build/type-check and nonempty Rust regression filters on Actions.
- Only after implementation and 410 validation: `1.1.4-beta2`, pre-release and
  not latest. The running beta1 device is not upgraded by development CI.

# SimAdmin Quectel and USB SIM Reader Expansion TODO

## 1. Scope and baseline

This document is based on SimAdmin `codex/volte-beta8-fix` and VoCat commit
`a624c454bcf82bf82a850d83e915c82b57a12c5c` (reviewed on 2026-08-15).

SimAdmin already has a strong generic modem path:

- ModemManager discovery and stable per-physical-slot line identities.
- Standard AT operations for SIM, SMS, calls, registration and radio controls.
- QMI/ModemManager data bearers and a per-line traffic proxy.
- QCM410-specific RPMSG DATA6 binding, secondary QMI data and IMS bearers.
- Per-line eSIM reader settings for QMI, AT, PC/SC and MBIM lpac backends.
- A persisted standalone SIM-reader model and synthesized reader lines.

Airplane mode intentionally remains a normal per-line ModemManager radio
control. The experimental boot-time QMI offline guard was removed after QCM410
testing showed firmware-dependent enumeration and recovery behavior. SimAdmin
does not attempt to replace ModemManager during system startup.

The missing pieces are not another copy of the cellular stack. They are:

- explicit Quectel family identification and support diagnostics;
- reliable installation of the host tools/drivers used by USB Quectel modules;
- live PC/SC discovery and presence checks for standalone readers;
- PC/SC SIM identity and AKA access in the per-line VoWiFi runtime;
- an active UI for configuring reader slots;
- PC/SC-enabled lpac compatibility artifacts;
- physical-hardware validation across the requested module families/readers.

## 2. Capability matrix

| Capability | QCM410 | EC20/EC25/EG25/EG600 | USB SIM reader |
| --- | --- | --- | --- |
| Discovery | ModemManager + QCM410 sysfs | ModemManager, USB `2c7c`, QMI/AT ports | PC/SC + USB CCID |
| Stable line identity | physical slot + UIM slot | physical USB path/equipment ID + UIM slot | configured reader ID + slot |
| Data proxy | secondary DATA6 QMI | ModemManager/QMI primary bearer | not applicable |
| Calls/SMS | ModemManager or IMS | ModemManager or IMS | VoWiFi IMS only |
| VoLTE bearer | QCM410 secondary QMI | generic ModemManager/QMI, firmware dependent | not applicable |
| VoWiFi identity/AKA | QMI UIM | QMI UIM (AT fallback is future work) | PC/SC APDU |
| eSIM/lpac | QMI | QMI/AT/MBIM | PC/SC |
| Hotplug | registry refresh | ModemManager refresh | PC/SC refresh |

## 3. Architecture mapping

VoCat groups Quectel USB interfaces by their physical sysfs parent, recognizes
vendor `2c7c`, selects the AT interface from the module's USB composition and
uses standard AT/QMI operations. SimAdmin delegates that discovery and most
control-plane behavior to ModemManager, then creates stable `ModemBinding`
records. The compatible SimAdmin extension point is therefore metadata and
capability classification around `ModemBinding`, not a second modem manager.

VoCat accesses readers through PC/SC and keeps identity/APDU transactions bound
to one reader. SimAdmin already keeps SIM access per line in
`vowifi::live::LiveSimDevice`; that mapping must be extended from QMI-only to a
QMI-or-PC/SC access descriptor. eSIM continues to use lpac, while VoWiFi AKA
uses a small PC/SC APDU adapter and the same parsed AKA result type as QMI.

Device-specific code follows a vendor/family layout under `hardware/devices`:

```text
devices/
  qcm410/             # QCM410-only RPMSG/QMI behavior
  pcsc/               # USB PC/SC reader discovery and APDU transport
  quectel/
    mod.rs             # vendor-level classification and common metadata
    ec2x.rs            # shared EC20 / EC25 / EG25 family path
    eg600.rs           # EG600-specific family path
```

Quectel modules are split by shared driver behavior rather than one directory
per SKU. If `ec2x.rs` later grows separate AT, APDU and radio-control modules,
it can become an `ec2x/` directory without changing the public
`hardware::devices::quectel` entry point.

## 4. Implementation checklist

### Phase A: current cleanup and deployment correctness

- [x] Remove the unused frontend `provider/` assets and their carrier-logo code.
- [x] Remove unreachable frontend components and unused dependencies.
- [x] Make release/OTA packages carry the secondary-QMI system resources.
- [x] Install and start the secondary-QMI service before ModemManager.
- [x] Keep the static DATA6 rule as a package fallback, not an unconditional install.
- [x] Fix scheduled data consumption when persistent mobile data is disabled.
- [x] Bound scheduled data work and always restore temporary bearer state.
- [x] Treat an already-ended scheduled call as a successful completed call.
- [x] Fix the overview column width and bottom-row alignment.

### Phase B: Quectel module support

- [x] Add family classification for EC20, EC25, EG25 family and EG600 family.
- [x] Publish family/transport metadata in each modem line response.
- [x] Show the detected family in the SIM/line workbench.
- [x] Keep unknown Quectel `2c7c` devices on the generic compatible path.
- [x] Install/check ModemManager, libqmi tools and USB mode-switch support.
- [x] Add classification and compatibility tests.

### Phase C: USB SIM reader runtime

- [x] Discover PC/SC readers and expose a read-only reader inventory API.
- [x] Resolve `pcsc://<reader>` selectors and mark configured reader lines present.
- [x] Read ICCID, IMSI and EF_AD MNC length through PC/SC APDUs.
- [x] Verify the USIM application and execute 3G AKA on the selected reader.
- [x] Dispatch per-line VoWiFi identity and AKA to QMI or PC/SC without fallback to another card.
- [x] Add a SIM-reader management tab using the existing slot configuration API.
- [x] Keep readers cellular-data/VoLTE disabled while allowing VoWiFi operations.
- [x] Add parser, APDU, identity and per-line dispatch tests.
- [x] Add explicit status for missing `pcscd`, CCID driver or OpenSC tools.

### Phase D: eSIM and installer integration

- [x] Build the compatibility lpac artifact with `LPAC_WITH_APDU_PCSC=ON`.
- [x] Bundle or document the runtime `libpcsclite` dependency.
- [x] Install/start `pcscd` and install CCID/OpenSC packages where supported.
- [ ] Verify lpac reader name/index selection against a physical eUICC reader.
- [ ] Remove PC/SC services/packages only when SimAdmin installed them (future installer state tracking).

### Phase E: physical test matrix

- [ ] QCM410: DATA6 is ignored by ModemManager and normal data remains on the primary QMI port.
- [ ] QCM410: scheduled data consumption succeeds with persistent data disabled and restores it disabled.
- [ ] QCM410: scheduled call starts, auto-hangs up, and tolerates remote early hangup.
- [ ] EC20: discovery, AT, SIM identity, SMS, call, QMI data and proxy traffic.
- [ ] EC25: same matrix plus hot unplug/replug.
- [ ] EG25 family: interface composition, QMI data and radio mode controls.
- [ ] EG600: actual USB/PCIe composition, registration, data and supported radio controls.
- [ ] USB reader: no card, physical SIM, PIN-locked SIM, USIM AKA, reader hotplug.
- [ ] USB eUICC reader: profile list/download/enable/disable through PC/SC lpac.

## 5. API and data-model changes

- Extend `ModemBinding` with non-sensitive `device_family` and
  `control_transport` fields.
- Add `GET /api/sim/readers` returning reader name/index, card presence and a
  remediation status. Do not return ATR or subscriber identifiers by default.
- Continue using `StandaloneSimSlotConfig.reader_path` with
  `pcsc://<reader-name>` selectors to avoid a second persisted reader model.
- Keep SIM PIN out of API responses and logs. PIN persistence needs a separate
  encrypted-secret design and is not part of the first implementation batch.

## 6. Risks and acceptance rules

- Module names alone do not prove firmware capabilities. Unsupported commands
  must remain capability errors, not trigger baseband resets.
- ModemManager owns generic Quectel ports. SimAdmin must not apply the QCM410
  DATA6 udev rule to USB Quectel modules.
- A PC/SC reader line must never fall back to the first QMI modem or first PC/SC
  reader after it has been bound by name.
- RAND, AUTN, RES, CK, IK, AUTS, IMSI and ICCID must not enter logs or API error
  payloads.
- External helper commands require strict timeouts and bounded output parsing.
- Unit/integration tests may prove parsing and dispatch without hardware, but a
  module/reader is only marked physically verified after the matching device is
  tested.

## 7. Definition of done

The software implementation is complete when all Phase B-D code items pass
lint, type-check, Rust tests, cross compilation and package syntax checks.
QCM410 acceptance additionally requires live service/API checks on the provided
device. EC20/EC25/EG25/EG600 and USB-reader physical acceptance remains open
until those devices are attached; the application must report their absence
cleanly rather than claiming a successful hardware test.

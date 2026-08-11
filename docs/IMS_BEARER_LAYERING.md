# IMS bearer layering: what is on the device layer, and what is not

This document explains the seam between the *device-specific* modem drivers
(`hardware::devices`) and the *upper protocol layers* (`connectivity::modems::ims`),
what was moved behind that seam in this change, and why the data plane, SMS and
registration paths are deliberately **not** implemented on the device layer yet.

## The seam

`hardware/devices/transport.rs` defines one trait per *caller*, so an upper layer
depends only on the capability it actually needs:

| Trait                | Caller                     | Status |
|----------------------|----------------------------|--------|
| `ImsBearerTransport` | VoLTE bearer strategy      | **implemented (qcm410)** |
| `DataTransport`      | data path                  | not wired (stub) |
| `VoiceTransport`     | voice path                 | not wired (stub) |
| `SmsTransport`       | SMS path                   | not wired (stub) |
| `RegistrationTransport` | registration path       | not wired (stub) |

A turnkey chip (the Qualcomm 410) implements the subset it supports; a soft-stack
host could implement only registration + voice and leave data to ModemManager.
Dispatch between drivers (sysfs detection) is still future work —
`devices::detect_device_kind` returns `Unknown`.

## What this change did

### 1. The IMS bearer moved behind `ImsBearerTransport`

The one thing that is irreducibly 410-specific — and the thing that was the
*least* safe to leave in the upper layer — is the **DATA6 WDS bearer**. Before
this change `connectivity/.../volte/native_bearer.rs` imported
`secondary_qmi` directly and orchestrated WDS sessions by hand. Now:

- `hardware/devices/qcm410/ims_bearer.rs` is the qcm410 **driver**. It binds the
  secondary endpoint, starts retained WDS sessions (single-family or dual),
  reads the IMS context settings, resolves the bam-dmux netdev, and hands back a
  device-agnostic `ImsBearerInfo` plus an opaque `ImsBearerHandle` that tears it
  down. It is the only module that knows `secondary_qmi`, `DATA6`, `DATA*_CNTL`.
- `connectivity/.../volte/native_bearer.rs` is now pure **strategy**. It picks
  which family to attempt and in what order (plan + network-forced fallback),
  classifies failures (baseband wedge, settings missing, netdev unresolved), and
  projects the result onto the `BearerConnection` the rest of the stack consumes.
  It no longer imports anything from `secondary_qmi`.
- `hardware/cellular/cgcontrdp.rs` holds the device-agnostic `AT+CGCONTRDP`
  reader (address, gateway, DNS, P-CSCF) shared by both P-CSCF discovery and the
  qcm410 driver, so the driver does not reach into a protocol module.

Error codes and detail strings are preserved byte-for-byte, so the runtime
classifies exactly as before.

### 2. What the driver returned, in one structure

`ImsBearerInfo` is the device-agnostic contract: interface + how it was decided,
`ip_type`, the IP/P-CSCF settings, and the two strings the synthetic bearer path
(`qmi-wds:...`) is built from. Teardown is a single `Box<dyn ImsBearerHandle>`
method — the upper layer never sees a `SecondaryQmiEndpoint` or `ImsSession`.

## Why data, SMS and registration are not on the device layer yet

### Data plane

User data already runs through **ModemManager on the primary QMI port** for the
common case (`data_slot.rs` keeps IMS on the primary port when no data slot is
held, and moves IMS to DATA6 only when ModemManager is carrying ordinary data).
The 410 does have a spare-channel data driver (`secondary_qmi_data.rs`,
`SecondaryDataRuntime`) that keeps a retained WDS CID alive on DATA6 when IMS
owns the primary port, but:

- it is wired **directly** into `services/line_registry` and is not yet routed
  through the `DataTransport` trait;
- moving user data between slots is exactly the operation that **wedges this
  baseband** on the reference firmware — every such move must stay behind the
  guarded slot allocator, and the "don't hand a wedged baseband to ModemManager"
  rule in `live.rs`.

So data stays on ModemManager + the existing slot logic for now. A later step can
surface `SecondaryDataRuntime` behind `DataTransport` without changing upper
layers.

### SMS

SMS-over-IMS is implemented by the **user-space IMS stack itself**
(`connectivity/.../volte/sms.rs`, RP-Data over the SIP channel), and CS SMS runs
through ModemManager. The modem only needs to be reachable — it does not need a
device-specific SMS *transport* for either path. `SmsTransport` exists for
platforms where SMS must be driven at the driver level (e.g. a voice-only soft
stack), but the 410 has nothing to add today.

### Registration

Registration is observed two ways: the modem's 3GPP registration is polled via
ModemManager (`readiness.rs`), and the IMS-level REGISTER is driven by the
user-space stack (`runtime.rs`). There is no device-specific steering to do on
the 410 beyond what ModemManager already does, so `RegistrationTransport`
remains a stub. (The 410's `secondary_qmi` *could* read network state over QMI,
but that would duplicate ModemManager's role for no benefit.)

## What implementing each deferred transport would entail

| Trait | Prerequisite work | Risk |
|-------|-------------------|------|
| `DataTransport` | Move `SecondaryDataRuntime` behind the trait; keep the slot allocator and the wedge guard on top | high (baseband restart) |
| `SmsTransport` | None on the 410 (SMS is user-space/ModemManager) | — |
| `RegistrationTransport` | None on the 410 (ModemManager already owns it) | — |
| driver dispatch | Wire `detect_device_kind()` into construction of the transport objects | low |

The rule of thumb: put a capability on the device layer **only when the device
does something ModemManager or the user-space IMS stack cannot**. The IMS bearer
is the one case where the 410 must act directly on QMI, and it is now the single
place that does.

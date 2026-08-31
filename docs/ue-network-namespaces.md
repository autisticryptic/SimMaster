# UE Network Namespace Architecture

SimAdmin runs every physical line inside its own Linux network namespace. This
is mandatory runtime behavior, not an optional deployment mode.

There is no `ue_isolation` configuration block and no feature gate for VoWiFi,
VoLTE, cellular data proxy, secondary DATA bearers, or operator-facing RTP
sockets. A configuration file containing the removed block is rejected as an
unknown top-level field.

This removal increments the operator configuration schema to
`config_version: 5`; version 4 files are not migrated automatically.

## Fixed Runtime Layout

- Namespace prefix: `sa-ue`
- Host veth prefix: `savh`
- UE veth prefix: `save`
- Veth MTU: `1500`
- One UE worker process per active line
- VoWiFi TUN, IKE, SIP, XFRM, and operator RTP sockets live in the line namespace
- VoLTE bearer interfaces, SIP, XFRM, and operator RTP sockets live in the line namespace
- Cellular data proxy egress and owned secondary DATA bearer interfaces live in the line namespace

The host side of each veth pair exists only to provide namespace egress and
NAT. It is not a fallback data plane for line traffic.

## Failure Policy

Namespace creation, worker startup, veth configuration, NAT setup, interface
migration, or worker socket creation failures make the affected line path
unavailable. SimAdmin does not retry the operation in the host network
namespace.

Teardown may return a session-owned modem interface to the host immediately
before releasing that bearer. This is cleanup required by the kernel and modem
lifecycle, not an operational fallback.

## Configuration

There are no operator-configurable namespace switches. Prefixes and MTU are
code-owned constants so all access paths share the same ownership model.

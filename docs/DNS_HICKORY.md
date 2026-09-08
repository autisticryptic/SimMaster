# Pure-Rust system DNS migration (beta2)

## Scope

`platform::dns` uses `hickory-resolver` 0.25.2 with `tokio` and
`system-config`, compatible with reqwest 0.12's Hickory dependency.
No backend compilation is performed locally. The refactor branch has a
non-publishing GitHub Actions workflow.

Migrated system lookups: ePDG, Trunk, TS.43 entitlement, cellular data proxy,
and the SOCKS5 proxy endpoint itself. Production HTTP client builders share
the same resolver; reqwest's `hickory-dns` default is also enabled.
TS.43's checked address pins and redirect validation are retained.

## Resolution and isolation

1. Numeric IPv4/IPv6, including bracketed IPv6, bypass files and DNS.
2. Hickory's hosts parser runs before reading resolver configuration.
   Thus a hosts-pinned ePDG still works if resolv.conf is unavailable.
3. Hickory reads the current system nameservers, search domains and options.
   A/AAAA are both requested; the overall network lookup budget is four seconds.
   Fresh resolvers preserve the configured server order (`UserProvidedOrder`);
   randomized initial RTT statistics are not evidence for preferring later
   servers. System per-query timeouts and retry counts are not overwritten.
4. Empty answers/errors are failures. Generic DNS does not inject public servers.
   The existing, explicit ePDG public-fallback policy remains at its call site.

Each lookup owns a fresh, bounded resolver. There is no new global socket/cache
that can escape its originating runtime/network namespace, and no stale resolver
survives a hosts/nameserver configuration change. This deliberately trades a
small configuration-read cost for deterministic ownership. Existing line-level
ePDG caching remains separate.

The migration preserves each caller's existing network context. It does **not**
claim to move every parent-process lookup into a UE worker. Carrier-specified
DNS, P-CSCF DNS, NAPTR and SOCKS5 UDP DNS already use dedicated Rust transports;
their resolver/egress selection is not silently replaced with host DNS.
Further unification of those transports is a separate compatibility task,
not a prerequisite for removing libc-based system lookups.

## Verification

Hardware-free tests cover numeric addresses, hosts aliases/case/trailing dots
and replacement contents, local A/AAAA replies, isolated resolver configurations,
NXDOMAIN, bounded timeout and system search/nameserver options. Existing ePDG,
SOCKS5, Trunk, TS.43/SSRF and IMS refresh regressions are included in branch CI.

## 410 field validation (2026-09-08)

The first beta2 candidate `2bb6099` passed release workflow `34169025117`
and was deployed at 07:34 (Asia/Shanghai). Old line configuration, API
compatibility and initial VoWiFi registration passed device checks.
Its natural protected refresh passed at 08:25:52: CSeq=3, 532 seconds left,
200 OK after 0.36 seconds, renewed lease 2858 seconds, unchanged service/TUN
and `reused_access=true`.

The live HTTP DNS check found a release-gating issue in that candidate:
Hickory's default `QueryStatistics` initializes each new pool with random RTTs.
On the device, requests selected later unreachable configured servers, instead
of the reachable first server, and exhausted the four-second overall budget.
At 08:03, captured A/AAAA requests went to configured servers 2 and 4; a
separate numeric-address UDP diagnostic got an answer from server 0 in 21 ms.
The diagnostic is not misreported as an application resolver test.

The `UserProvidedOrder` correction `0b97b4f` passed branch validation
`34172883931` and final release workflow `34173433980`, including the
six-server/fresh-resolver regression and both architecture builds. It was
deployed at 08:43, retaining beta2/pre-release/non-latest status and matching
tag/package/device commits. The original candidate and backup were retained.

At 08:54:46 the corrected application's HTTP client queried configured
nameserver 0 for A/AAAA and received both responses (A answer and valid AAAA
NODATA). The complete HTTPS metadata fetch succeeded in 0.496 seconds.
This is the deployed Rust application's path, not curl/Python DNS standing in
for it. No resolv.conf change, public-DNS insertion, OTA preparation/installation
or registration command was used by the check.

Old/new APIs, authentication, persistent settings and initial registration
were checked again on `0b97b4f`. Its own natural protected refresh passed at
09:31:28: CSeq=3, Security-Verify present, 531 seconds left, matching 200 OK
after 0.292738 seconds and renewed lease 3100 seconds. Service identity and TUN
index were unchanged; the log reported `reused_access=true`. The check also
verified that the request followed the actual proactive refresh deadline.
The temporary metadata capture was stopped without restarting the service.

This verifies the deployed HTTP/system DNS path and the current single WLAN
registration, not live dual registration, an unconfigured Trunk, or every
carrier/proxy DNS path. Detailed evidence and untested cases remain in `plan.md`.

The later policy/cost-guard candidate `48e37fc` passed final workflow
`34197531701` and was deployed at 15:57. Its application HTTP DNS check passed
again at 16:23 (0.49 seconds), and its own protected natural refresh passed at
16:47:58. This candidate retains the same pure-Rust resolver and server-order
correction; it does not claim that the separate carrier/proxy transports were
rewritten or all physically exercised.

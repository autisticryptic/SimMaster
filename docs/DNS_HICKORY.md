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

The beta2 candidate `2bb6099` passed the final release workflow
`34169025117` and was deployed at 07:34 (Asia/Shanghai). Old line configuration,
API compatibility and initial VoWiFi registration passed device checks.
Its own first natural refresh is still pending; beta1's earlier result is not
substituted for it.

The live HTTP DNS check found a release-gating issue in that candidate:
Hickory's default `QueryStatistics` initializes each new pool with random RTTs.
On the device, requests selected later unreachable configured servers, instead
of the reachable first server, and exhausted the four-second overall budget.
At 08:03, captured A/AAAA requests went to configured servers 2 and 4; a
separate numeric-address UDP diagnostic got an answer from server 0 in 21 ms.
The diagnostic is not misreported as an application resolver test.

The `UserProvidedOrder` correction and a six-server/fresh-resolver regression
are now implemented on the refactor branch, **not yet built or deployed**.
It does not change resolv.conf, inject public DNS, or restart IMS.
Do not mark the live HTTP lookup successful until the corrected release is
validated. Detailed build, deployment and refresh evidence belongs in `plan.md`.

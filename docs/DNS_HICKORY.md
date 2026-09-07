# Pure-Rust system DNS migration (beta2 development)

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

At creation of this document the new DNS code is **not deployed or yet validated
on 410**. The device remains on beta1/e55780a. DNS build/CI results and later
beta2 field testing must be recorded in `plan.md`; do not infer live success
from source changes or unit tests.

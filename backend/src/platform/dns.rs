//! Pure-Rust system DNS, shared by explicit lookups and HTTP clients.
//!
//! No libc/NSS/getaddrinfo fallback. Load hosts BEFORE resolver configuration,
//! so an operator's hosts override works even without usable resolv.conf.
//! Rebuild the small resolver per lookup: no process-global sockets/cache can
//! cross a runtime, UE namespace or nameserver/hosts configuration change.
//! Sockets are created in the caller's existing network context. This module
//! does not move a host-side lookup into a worker or vice versa; explicit
//! carrier DNS and proxied DNS remain separate caller-selected transports.

use std::{
    io,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use hickory_resolver::{
    config::{LookupIpStrategy, ResolveHosts},
    proto::{
        op::Query,
        rr::{Name, RecordType},
    },
    Hosts, TokioResolver,
};

const LOOKUP_TIMEOUT: Duration = Duration::from_secs(4);

fn host_name(host: &str) -> io::Result<&str> {
    let host = host.trim();
    let host = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host);
    if host.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "dns_empty_host",
        ));
    }
    Ok(host)
}

fn hosts_addresses(hosts: &Hosts, host: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
    let name = Name::from_utf8(host)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "dns_invalid_host"))?;
    let mut addresses = Vec::new();
    for kind in [RecordType::A, RecordType::AAAA] {
        if let Some(lookup) = hosts.lookup_static_host(&Query::query(name.clone(), kind)) {
            addresses.extend(
                lookup
                    .iter()
                    .filter_map(|record| record.ip_addr())
                    .map(|ip| SocketAddr::new(ip, port)),
            );
        }
    }
    addresses.sort_unstable();
    addresses.dedup();
    Ok(addresses)
}

/// Resolve numbers without I/O, hosts through Hickory's parser, then names
/// through the CURRENT system configuration. This function adds no public DNS.
pub async fn resolve_socket_addrs(host: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
    let host = host_name(host)?;
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Ok(vec![SocketAddr::new(ip, port)]);
    }
    let hosts = Hosts::from_system().unwrap_or_default();
    let addresses = hosts_addresses(&hosts, host, port)?;
    if !addresses.is_empty() {
        return Ok(addresses);
    }
    let mut builder = TokioResolver::builder_tokio()
        .map_err(|error| io::Error::other(format!("dns_system_config_failed:{error}")))?;
    let options = builder.options_mut();
    options.ip_strategy = LookupIpStrategy::Ipv4AndIpv6;
    options.use_hosts_file = ResolveHosts::Never; // already read above
    options.cache_size = 32; // bounded and owned by this lookup only
    lookup(&builder.build(), host, port, LOOKUP_TIMEOUT).await
}

async fn lookup(
    resolver: &TokioResolver,
    host: &str,
    port: u16,
    timeout: Duration,
) -> io::Result<Vec<SocketAddr>> {
    let result = tokio::time::timeout(timeout, resolver.lookup_ip(host))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "dns_lookup_timeout"))?
        .map_err(|error| io::Error::other(format!("dns_lookup_failed:{error}")))?;
    let mut addresses = result
        .iter()
        .map(|ip| SocketAddr::new(ip, port))
        .collect::<Vec<_>>();
    addresses.sort_unstable();
    addresses.dedup();
    if addresses.is_empty() {
        return Err(io::Error::new(io::ErrorKind::NotFound, "dns_empty_answer"));
    }
    Ok(addresses)
}

#[derive(Debug)]
struct HttpDnsResolver;

impl reqwest::dns::Resolve for HttpDnsResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addresses = resolve_socket_addrs(&host, 0).await?;
            Ok(Box::new(addresses.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// Long-lived HTTP clients must also observe system DNS/hosts changes. Explicit
/// ClientBuilder::resolve/resolve_to_addrs pins still take precedence (TS.43).
pub fn http_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder().dns_resolver(Arc::new(HttpDnsResolver))
}

#[cfg(test)]
pub(crate) fn addresses_from_hosts_file(contents: &str, host: &str, port: u16) -> Vec<SocketAddr> {
    let mut hosts = Hosts::default();
    if hosts.read_hosts_conf(contents.as_bytes()).is_err() {
        return Vec::new();
    }
    hosts_addresses(&hosts, host.trim(), port).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_resolver::{
        config::{NameServerConfigGroup, ResolverConfig},
        name_server::TokioConnectionProvider,
        proto::{
            op::{Message, MessageType, ResponseCode},
            rr::{RData, Record},
        },
    };
    use std::net::{Ipv4Addr, Ipv6Addr};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::net::UdpSocket;

    fn resolver(server: SocketAddr) -> TokioResolver {
        let servers = NameServerConfigGroup::from_ips_clear(&[server.ip()], server.port(), true);
        let config = ResolverConfig::from_parts(None, vec![], servers);
        let mut builder =
            TokioResolver::builder_with_config(config, TokioConnectionProvider::default());
        let options = builder.options_mut();
        options.ip_strategy = LookupIpStrategy::Ipv4AndIpv6;
        options.use_hosts_file = ResolveHosts::Never;
        options.attempts = 1;
        options.timeout = Duration::from_millis(400);
        builder.build()
    }

    async fn dns_server(
        v4: Ipv4Addr,
        response_code: ResponseCode,
    ) -> (SocketAddr, tokio::task::JoinHandle<()>, Arc<AtomicUsize>) {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let addr = socket.local_addr().unwrap();
        let queries = Arc::new(AtomicUsize::new(0));
        let received = Arc::clone(&queries);
        let task = tokio::spawn(async move {
            let mut buffer = [0u8; 4096];
            loop {
                let (size, peer) = socket.recv_from(&mut buffer).await.unwrap();
                received.fetch_add(1, Ordering::SeqCst);
                let request = Message::from_vec(&buffer[..size]).unwrap();
                let mut response = Message::new();
                response
                    .set_id(request.id())
                    .set_message_type(MessageType::Response)
                    .set_recursion_desired(request.recursion_desired())
                    .set_recursion_available(true)
                    .set_authoritative(true)
                    .set_response_code(response_code)
                    .add_queries(request.queries().iter().cloned());
                if response_code == ResponseCode::NoError {
                    for query in request.queries() {
                        let ip = match query.query_type() {
                            RecordType::A => IpAddr::V4(v4),
                            RecordType::AAAA => IpAddr::V6(Ipv6Addr::LOCALHOST),
                            _ => continue,
                        };
                        response.add_answer(Record::from_rdata(
                            query.name().clone(),
                            30,
                            RData::from(ip),
                        ));
                    }
                }
                socket
                    .send_to(&response.to_vec().unwrap(), peer)
                    .await
                    .unwrap();
            }
        });
        (addr, task, queries)
    }

    #[tokio::test]
    async fn numeric_ipv4_and_ipv6_need_no_system_configuration() {
        for (host, expected) in [
            ("192.0.2.3", "192.0.2.3:5060"),
            ("2001:db8::3", "[2001:db8::3]:5060"),
            (" [2001:db8::3] ", "[2001:db8::3]:5060"),
        ] {
            assert_eq!(
                resolve_socket_addrs(host, 5060).await.unwrap(),
                vec![expected.parse::<SocketAddr>().unwrap()]
            );
        }
        assert_eq!(
            resolve_socket_addrs(" ", 53).await.unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    #[test]
    fn hosts_aliases_comments_case_trailing_dot_and_reloads() {
        let contents =
            "192.0.2.1 epdg.test Alias # comment\n2001:db8::1 EPDG.TEST\n192.0.2.1 epdg.test\n";
        let addresses = addresses_from_hosts_file(contents, " EPDG.TEST. ", 500);
        assert_eq!(
            addresses,
            vec![
                "192.0.2.1:500".parse().unwrap(),
                "[2001:db8::1]:500".parse().unwrap()
            ]
        );
        assert_eq!(addresses_from_hosts_file(contents, "alias", 500).len(), 1);
        let changed = addresses_from_hosts_file("192.0.2.2 epdg.test", "epdg.test", 500);
        assert_eq!(
            changed,
            vec!["192.0.2.2:500".parse::<SocketAddr>().unwrap()]
        );
    }

    #[tokio::test]
    async fn local_dns_returns_both_address_families_and_requested_port() {
        let (server, task, queries) =
            dns_server(Ipv4Addr::new(192, 0, 2, 5), ResponseCode::NoError).await;
        let result = lookup(
            &resolver(server),
            // .invalid is answered locally by RFC 6761, not by this server.
            "fixture.simadmin.test.",
            4500,
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        task.abort();
        assert!(
            queries.load(Ordering::SeqCst) >= 2,
            "A and AAAA must reach the fixture"
        );
        assert_eq!(
            result,
            vec![
                "192.0.2.5:4500".parse().unwrap(),
                "[::1]:4500".parse().unwrap()
            ]
        );
    }

    #[tokio::test]
    async fn resolver_configurations_do_not_share_cached_answers() {
        let (one, task_one, queries_one) =
            dns_server(Ipv4Addr::new(192, 0, 2, 1), ResponseCode::NoError).await;
        let (two, task_two, queries_two) =
            dns_server(Ipv4Addr::new(192, 0, 2, 2), ResponseCode::NoError).await;
        let first = lookup(
            &resolver(one),
            "same.simadmin.test.",
            80,
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        let second = lookup(
            &resolver(two),
            "same.simadmin.test.",
            80,
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        task_one.abort();
        task_two.abort();
        assert!(queries_one.load(Ordering::SeqCst) >= 2);
        assert!(queries_two.load(Ordering::SeqCst) >= 2);
        assert_ne!(first, second);
    }

    #[tokio::test]
    async fn nxdomain_and_timeout_are_errors_not_empty_success_or_public_fallback() {
        let (server, task, queries) = dns_server(Ipv4Addr::LOCALHOST, ResponseCode::NXDomain).await;
        assert!(lookup(
            &resolver(server),
            "missing.simadmin.test.",
            80,
            Duration::from_secs(2)
        )
        .await
        .is_err());
        assert!(
            queries.load(Ordering::SeqCst) > 0,
            "NXDOMAIN must come from the fixture"
        );
        task.abort();
        let silent = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let error = lookup(
            &resolver(silent.local_addr().unwrap()),
            "silent.simadmin.test.",
            80,
            Duration::from_millis(200),
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        let mut packet = [0u8; 4096];
        assert!(
            silent.try_recv_from(&mut packet).is_ok(),
            "timeout must follow an actual DNS query"
        );
    }

    #[cfg(unix)]
    #[test]
    fn system_config_keeps_nameservers_search_and_options() {
        let (config, options) = hickory_resolver::system_conf::parse_resolv_conf(
            "nameserver 192.0.2.53\nnameserver 2001:db8::53\nsearch ims.test\noptions ndots:2 timeout:3 attempts:1\n",
        ).unwrap();
        assert_eq!(config.name_servers().len(), 4); // UDP + TCP per server
        assert_eq!(
            config.search()[0].to_ascii().trim_end_matches('.'),
            "ims.test"
        );
        assert_eq!(options.ndots, 2);
        assert_eq!(options.timeout, Duration::from_secs(3));
        assert_eq!(options.attempts, 1);
        assert!(hickory_resolver::system_conf::parse_resolv_conf("nameserver invalid").is_err());
    }
}

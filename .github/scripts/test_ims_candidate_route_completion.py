"""Wiring guards for multi-address IMS routing; Rust behavior runs only in CI."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
IMS = ROOT / "backend/src/connectivity/modems/ims"


def source(path):
    return (IMS / path).read_text(encoding="utf-8")


def between(text, start, end):
    return text.split(start, 1)[1].split(end, 1)[0]


class CandidateRouteCompletionTests(unittest.TestCase):
    def test_cellular_prepares_all_routes_before_candidate_admission(self):
        live = source("cellular_ims/live.rs")
        flow = between(live, "async fn connect_inner(", "async fn verify_mm_task_bearer(")
        barrier = flow.index("let prepared_routes = route_pcscf_candidates_in_worker(")
        loop = flow.index("for (pcscf_index, pcscf)")
        gate = flow.index("match prepared_routes.require(pcscf.ip())")
        connect = flow.index("connect_family(", gate)
        self.assertLess(barrier, loop)
        self.assertLess(loop, gate)
        self.assertLess(gate, connect)
        admission = flow[loop:gate]
        for guard in ("verify_mm_task_bearer(", "ensure_generation(",
                      "ensure_worker_binding_current(", "native.check_liveness()?"):
            self.assertIn(guard, admission)
        self.assertIn("Err(error) => Err(error)", flow[gate:])
        family = between(live, "async fn connect_family(", "let route = ImsRoute")
        self.assertIn("route_pcscf_in_worker(bearer, pcscf.ip(), worker_binding).await?", family)

    def test_bearer_preparation_unions_accepted_and_selected_addresses_without_truncation(self):
        text = source("cellular_ims/bearer.rs")
        prepare = between(text, "async fn prepare_pcscf_routes_with", "pub async fn route_pcscf_in_worker")
        self.assertIn(".pcscf", prepare)
        self.assertIn(".chain(candidates.iter().map(SocketAddr::ip))", prepare)
        self.assertIn("Ok(op) => apply(op).await", prepare)
        self.assertIn("outcomes.push((host, outcome))", prepare)
        self.assertIn("code::RUNTIME_UE_WORKER_GENERATION_CHANGED", prepare)
        for forbidden in (".take(", ".truncate(", "settings.pcscf.clear("):
            self.assertNotIn(forbidden, prepare)
        gate = between(text, "pub fn require(&self, pcscf:", "/// Prepare every accepted bearer")
        self.assertIn("outcome.clone()", gate)
        self.assertIn("P-CSCF route was not prepared", gate)

    def test_worker_routes_keep_family_validation_and_terminal_generation_errors(self):
        text = source("cellular_ims/bearer.rs")
        route = between(text, "fn worker_host_route_op(", "async fn apply_worker_ops(")
        for invariant in ("local_addr_for_family(host)", "gateway_for_family(host)",
                          "code::ROUTE_FAMILY_MISMATCH", "code::ROUTE_GATEWAY_FAMILY_MISMATCH",
                          "target: host_selector(host)", "dev: Some(bearer.interface.clone())",
                          "src: Some(source.to_string())", "table: None"):
            self.assertIn(invariant, route)
        apply = between(text, "async fn apply_worker_ops(", "/// Ensure that the kernel data path")
        result = apply.index("let result = worker.apply_net_config(ops).await")
        self.assertLess(result, apply.index("if !worker.is_current()", result))
        self.assertLess(apply.index("if !worker.is_current()", result), apply.index("result.map_err"))

    def test_vowifi_full_handshake_has_no_address_prefix_cap(self):
        text = source("vowifi/live.rs")
        iterator = between(text, "async fn try_live_epdg_addresses", "async fn run_live_ike_until_depth_for_stack")
        self.assertIn("LiveProbeDepth::StatusSaInit => addresses.len().min(1)", iterator)
        self.assertIn("LiveProbeDepth::FullHandshake => addresses.len()", iterator)
        self.assertIn("attempt(*address).await", iterator)
        self.assertNotIn("LIVE_IKE_MAX_ENDPOINTS_PER_PASS", text)
        flow = between(text, "async fn run_live_ike_until_depth_for_stack", "fn live_ike_socket_spec")
        self.assertIn("try_live_epdg_addresses(&endpoint.addresses, depth", flow)
        self.assertIn("let route_kind = endpoint.route_policy.kind", flow)
        self.assertIn("destination.set_port(path.destination_port)", flow)

    def test_live_proxy_and_direct_paths_are_worker_bound_and_fail_closed(self):
        live = source("vowifi/live.rs")
        create = between(live, "async fn create_live_ike_transport(", "async fn run_live_ike_with_destination(")
        for expected in ("checked_live_ike_proxy(", "Socks5UdpClient::connect_in_worker(",
                         "&ue_socket.binding", "&ue_socket.ue_veth",
                         "UdpSocketDatagramTransport::from_socks5(client)",
                         "ue_socket.worker.create_socket(spec).await", "binding.is_current()"):
            self.assertIn(expected, create)
        for forbidden in ("UdpSocketDatagramTransport::bind(", "DatagramPath::direct_for(",
                          "Socks5UdpClient::connect("):
            self.assertNotIn(forbidden, create)
        socks = source("vowifi/socks5.rs")
        worker = between(socks, "pub async fn connect_in_worker(", "async fn connect_resolved_with")
        self.assertNotIn("host_socket", worker)
        self.assertIn("worker.worker().create_socket(spec).await", worker)
        associate = between(socks, "async fn connect_resolved_with", "pub(super) fn udp_socket")
        self.assertIn("for peer in addresses", associate)
        self.assertIn("relay_local_addr(relay)", associate)
        self.assertLess(associate.index("request_udp_associate("), associate.index("UeSocketSpec::udp_connected("))
        self.assertIn("Err(Socks5Error::WorkerChanged) =>", associate)

    def test_nat_t_and_esp_preserve_per_datagram_proxy_targets(self):
        transport = source("vowifi/transport.rs")
        send = between(transport, "async fn send_packet(", "async fn recv_packet(")
        self.assertIn("DatagramSocket::Socks5(client)", send)
        self.assertIn(".send_to(destination, payload)", send)
        self.assertIn("Socks5(Arc<super::socks5::Socks5UdpClient>)", transport)
        self.assertIn('self.send_packet("esp_nat_t_udp_4500", destination, esp_frame)', transport)
        self.assertIn("SOCKS5 reply has no numeric origin", transport)
        live = source("vowifi/live.rs")
        self.assertIn("transport.clone().with_recv_timeout(LIVE_IKE_AUTH_TIMEOUT)", live)
        # System-DNS fallback must not erase the requested IKE/ESP proxy policy.
        epdg = between(source("vowifi/epdg.rs"), "pub async fn resolve_epdg_via_socks5(", "async fn query_dns_via_socks5(")
        fallback = epdg.split("let client =", 1)[0]
        self.assertIn("endpoint.route_policy =", fallback)
        self.assertIn("ProxyKind::Socks5UdpAssociate", fallback)

    def test_both_ci_suites_execute_multi_address_and_transport_regressions(self):
        filters = (
            "connectivity::modems::ims::cellular_ims::bearer::tests",
            "connectivity::modems::ims::cellular_ims::pcscf::tests",
            "hardware::devices::qcm410::primary_ims_pcscf::tests",
            "connectivity::modems::ims::vowifi::epdg::tests",
            "connectivity::modems::ims::vowifi::socks5::tests",
            "connectivity::modems::ims::vowifi::transport::tests",
            "connectivity::modems::ims::vowifi::live::epdg_address_tests",
            "connectivity::modems::ims::vowifi::live::tests::full_handshake_keeps_proposal_and_transport_limits",
        )
        for workflow in ("beta-validation.yml", "build-release.yml"):
            text = (ROOT / ".github/workflows" / workflow).read_text(encoding="utf-8")
            for filter in filters:
                self.assertIn(filter, text)
            self.assertIn("Empty regression filter", text)


if __name__ == "__main__":
    unittest.main()

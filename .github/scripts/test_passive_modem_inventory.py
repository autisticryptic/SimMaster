"""Static safety contract for gate-blocked display (no hardware/compiler needed)."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]


def read(path):
    return (ROOT / path).read_text(encoding="utf-8")


def between(text, start, end):
    return text.split(start, 1)[1].split(end, 1)[0]


class PassiveInventoryTests(unittest.TestCase):
    def test_mm_display_uses_only_one_cached_snapshot(self):
        source = read("backend/src/hardware/cellular/modem_manager.rs")
        passive = between(source, "pub async fn discover_passive_modems(",
                          "/// Enumerate every ModemManager modem")
        self.assertEqual(passive.count('.call("GetManagedObjects", &())'), 1)
        for forbidden in ("get_all_properties(", "get_sim_path(",
                          "read_usim_identity_fallback(", "qmi_control_device(",
                          "resolve_physical_slot_id(", "run_recovery_command_owned(",
                          "Command::", "open_logical", "transmit", "LineRuntime::",
                          "UeContext::", "CellularImsRuntime::"):
            self.assertNotIn(forbidden, passive)
        pure = between(source, "fn physical_slot_fallback(", "fn sim_identity_keys(")
        self.assertNotIn(".await", pure)
        self.assertNotIn("Command", pure)
        self.assertIn('sim.as_str() == "/"', passive)
        self.assertIn('present: true', passive)
        self.assertIn('observation_source: "modemmanager_cache"', passive)

    def test_unsupported_providers_cannot_fall_back_to_operational_discovery(self):
        source = read("backend/src/hardware/cellular/observations.rs")
        passive = between(source, "    fn discover_passive(", "    /// Fresh, positive")
        self.assertIn('"passive_inventory_unsupported"', passive)
        self.assertNotIn("self.discover", passive)
        dto = between(source, "pub struct PassiveModemInventory", "/// A failed observation")
        for field in ("ModemBinding", "modem_path", "qmi_device", "profile", "runtime"):
            self.assertNotIn(field, dto)

    def test_blocked_registry_does_not_provision_or_mutate(self):
        source = read("backend/src/services/line_registry.rs")
        passive = between(source, "    pub async fn passive_inventory_if_blocked(",
                          "    /// Refresh presence")
        self.assertIn("blocked_reason().await?", passive)
        self.assertIn("self.observations.discover_passive().await", passive)
        for forbidden in ("self.refresh", "ensure_ready", "self.lines", "config_manager",
                          "recover_owned", "LineRuntime::", "UeContext::", "netns", "worker"):
            self.assertNotIn(forbidden, passive)
        refresh = between(source, "    pub async fn refresh(", "    fn observe_mm_inventory(")
        self.assertLess(refresh.index(".ensure_ready(devices::recover_owned_ims_sessions)"),
                        refresh.index("self.observations.discover().await"))
        self.assertLess(refresh.index(".ensure_ready("), refresh.index("LineRuntime::new_for_device("))
        get = between(source, "    pub async fn get(", "    pub async fn all(")
        self.assertIn("self.lines.read().await.get(line_id).cloned()", get)
        self.assertNotIn("passive", get)

    def test_list_only_fallback_precedes_operational_refresh(self):
        source = read("backend/src/api/handlers.rs")
        for name, end in (("get_modem_lines_handler", "/// GET /api/health"),
                          ("get_cellular_ims_lines_handler", "pub async fn get_cellular_ims_line_handler")):
            handler = between(source, "pub async fn " + name, end)
            self.assertLess(handler.index("blocked_modem_inventory("), handler.index(".refresh().await"))
            self.assertIn("return response;", handler)
        helper = between(source, "async fn blocked_modem_inventory", "/// Enumerate every physical")
        self.assertIn("StatusCode::SERVICE_UNAVAILABLE", helper)
        self.assertIn("Failed to discover passive modems", helper)
        detail = between(source, "pub async fn get_cellular_ims_line_handler", "pub struct CellularImsProfileSelectionRequest")
        self.assertNotIn("passive_inventory", detail)
        self.assertIn("app.line_registry.get(&line_id)", detail)

    def test_ui_never_mounts_controls_for_display_inventory(self):
        source = read("frontend/src/pages/sim/ModemLinesPanel.tsx")
        self.assertLess(source.index("await api.getCellularImsLines()"), source.index("api.getTrunkLines()"))
        branch = between(source, "if (lineResponse.blocked_reason)", "setLines(stableModemSort")
        self.assertIn("setLines([])", branch)
        self.assertIn("return", branch)
        display = between(source, "  if (blockedReason)", "  const renderLineList")
        self.assertIn("return <Stack", display)
        self.assertIn("displayOnlyLines.map", display)
        self.assertIn("MM 缓存报告 SIM 存在", display)
        for forbidden in ("<Switch", "<TrunkProfileDialog", "workbenchEsim", "line.runtime", "line.profile"):
            self.assertNotIn(forbidden, display)


if __name__ == "__main__":
    unittest.main()

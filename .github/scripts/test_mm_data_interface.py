"""MM bearer interface ownership wiring, supplemented by Rust and private D-Bus tests."""
from pathlib import Path
import unittest
ROOT=Path(__file__).resolve().parents[2]
QCM=ROOT/'backend/src/hardware/devices/qcm410'

class MmDataInterfaceTests(unittest.TestCase):
    def test_adapter_pins_actual_interface_before_network_mutation(self):
        lifecycle=(QCM/'primary_ims_lifecycle.rs').read_text()
        self.assertIn('selected_interface: OnceLock<String>', lifecycle)
        self.assertIn('record.interface = interface.to_string()', lifecycle)
        self.assertIn('self.bus.verify_data_interface(bearer, interface).await?', lifecycle)
        self.assertIn('*kind == 2', lifecycle)
        self.assertIn('self.data_interface()', lifecycle)
        session=(QCM/'primary_ims_session.rs').read_text()
        self.assertIn('bind_data_interface(&args[1], &status.interface)', session)
        self.assertNotIn('value(&status, "bearer.status.interface") != Some(request.interface)', session)
        driver=(QCM/'ims_bearer.rs').read_text()
        self.assertIn('let interface = session.interface().to_string()', driver)

    def test_cleanup_without_network_intent_does_not_touch_primary_interface(self):
        text=(QCM/'primary_ims_lifecycle.rs').read_text()
        block=text[text.index('pub async fn cleanup(&self)'):text.index('fn forget(&self')]
        self.assertLess(block.index('record.namespace.is_some() || record.networks().next().is_some()'), block.index('self.bus.may_clean_interface'))
        self.assertIn('self.bus.disconnect(&record.bearer)', block)

    def test_reported_netdev_requires_same_device_topology_not_only_name(self):
        text=(QCM/'netdev.rs').read_text()
        block=text[text.index('fn verify_mm_data_interface_at('):text.index('impl NetdevConfig')]
        self.assertIn('canonicalize',block)
        self.assertIn('strip_prefix(baseband)',block)
        self.assertIn(':bam-dmux',block)
        self.assertNotIn('contains(baseband)',block)

if __name__=='__main__':unittest.main()

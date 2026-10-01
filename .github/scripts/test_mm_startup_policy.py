"""Do not replace an already functioning MM owner to normalize a drop-in."""
from pathlib import Path
import unittest
ROOT=Path(__file__).resolve().parents[2]

class MmStartupPolicyTests(unittest.TestCase):
    def test_preserving_running_debug_precedes_any_write_or_restart(self):
        text=(ROOT/'backend/src/main.rs').read_text()
        block=text.split('fn ensure_modemmanager_debug_override()',1)[1].split('fn modemmanager_debug_command',1)[0]
        self.assertLess(block.index('if modemmanager_debug_is_running()'),block.index('std::fs::write('))
        guard=block.split('if modemmanager_debug_is_running()',1)[1].split('let override_dir',1)[0]
        self.assertIn('return;',guard)
        reader=text.split('fn modemmanager_debug_is_running()',1)[1].split('#[cfg(test)]',1)[0]
        self.assertIn('MainPID',reader)
        self.assertIn('read_link',reader)
        self.assertIn('cmdline',reader)
        self.assertNotIn('restart',reader)
        self.assertNotIn('std::fs::write',reader)
    def test_checks_are_selected_by_both_workflows(self):
        for name in ('beta-validation.yml','build-release.yml'):
            self.assertIn('mm_startup_policy_tests',(ROOT/'.github/workflows'/name).read_text())
if __name__=='__main__':unittest.main()

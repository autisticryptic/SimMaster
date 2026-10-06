"""Keep current documentation navigable after historical records are archived."""
from pathlib import Path
import re
import unittest
from urllib.parse import unquote

ROOT = Path(__file__).resolve().parents[2]


class DocumentationLayoutTests(unittest.TestCase):
    def test_internal_markdown_targets_exist(self):
        missing = []
        for source in [ROOT / "README.md", ROOT / "bruno-api/README.md", *(ROOT / "docs").rglob("*.md")]:
            text = source.read_text(encoding="utf-8-sig")
            for url in re.findall(r"\]\(([^\s)]+)", text):
                if re.match(r"^[A-Za-z][A-Za-z0-9+.-]*:", url) or url.startswith(("#", "//")):
                    continue
                target = (source.parent / unquote(url.split("#", 1)[0])).resolve()
                # Historical evidence is intentionally local-only, not required
                # in a clean checkout and never published as a CI fixture.
                if target.is_relative_to(ROOT / ".local"):
                    continue
                if not target.exists():
                    missing.append((source.relative_to(ROOT).as_posix(), url))
        self.assertEqual(missing, [])

    def test_consolidated_manuals_do_not_regrow_dated_handoff_pages(self):
        canonical = {
            'README.md', 'HANDOFF.md', 'INSTALL.md', 'ARCHITECTURE.md', 'DEVELOPER.md',
            'DEVICE_DRIVERS.md', 'CARRIER_PROFILES.md', 'CHANGELOG.md', 'DEVELOPMENT_PLAN.md',
            'NATIVE_BACKEND_STATUS.md', 'IMS_REGISTRATION_POLICY.md',
            'IMS_MM_EXACT_FAMILY_LEASE_DESIGN.md', 'IMS_DIAGNOSTICS.md',
            'QCM410_BAM_DMUX_MODEM_CRASH.md', 'archive/README.md',
        }
        # This private, untracked report belongs to the user and is not part of
        # the maintained public documentation set.
        private = {'ESIM_IMS_PROFILE_TEST_2026-09-01.md'}
        actual = {p.relative_to(ROOT / 'docs').as_posix() for p in (ROOT / 'docs').rglob('*.md')}
        self.assertEqual(actual - private, canonical)
        self.assertLessEqual(len((ROOT / 'docs/HANDOFF.md').read_text(encoding='utf-8').splitlines()), 150)
        archive = (ROOT / 'docs/archive/README.md').read_text(encoding='utf-8')
        self.assertIn('git show 0502395:', archive)
        self.assertIn('docs-before.zip', archive)

    def test_single_current_entry_and_local_archive_boundary(self):
        self.assertIn("./docs/HANDOFF.md", (ROOT / "README.md").read_text(encoding="utf8"))
        for name in ["docs/HANDOFF.md", "docs/README.md", "docs/NATIVE_BACKEND_STATUS.md",
                     "docs/IMS_DIAGNOSTICS.md", "docs/archive/README.md"]:
            self.assertTrue((ROOT / name).is_file(), name)
        ignore = (ROOT / ".gitignore").read_text(encoding="utf8").splitlines()
        self.assertIn(".local/", ignore)
        self.assertIn("__pycache__/", ignore)
        self.assertFalse((ROOT / "plan.md").exists())
        self.assertFalse((ROOT / "DNS_RESOLVER_HICKORY_MIGRATION_TASK.md").exists())


if __name__ == "__main__":
    unittest.main()

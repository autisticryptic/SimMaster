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

"""Static guardrails complement the Rust DNS/socket regression tests."""
from pathlib import Path
import re
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]


class DnsBoundaryTests(unittest.TestCase):
    def test_system_config_and_http_hickory_are_enabled(self):
        cargo = tomllib.loads((ROOT / "backend/Cargo.toml").read_text(encoding="utf8"))
        resolver = cargo["dependencies"]["hickory-resolver"]
        self.assertIn("system-config", resolver["features"])
        self.assertIn("tokio", resolver["features"])
        self.assertIn("hickory-dns", cargo["dependencies"]["reqwest"]["features"])

    def test_no_system_hostname_lookup_calls(self):
        for path in (ROOT / "backend/src").rglob("*.rs"):
            with self.subTest(path=str(path.relative_to(ROOT))):
                self.assertNotRegex(
                    path.read_text(encoding="utf8"),
                    re.compile(r"\b(?:lookup_host|getaddrinfo|to_socket_addrs)\s*\("),
                )

    def test_http_builders_use_shared_factory(self):
        for path in (ROOT / "backend/src").rglob("*.rs"):
            if path == ROOT / "backend/src/platform/dns.rs":
                continue
            with self.subTest(path=str(path.relative_to(ROOT))):
                self.assertNotRegex(
                    path.read_text(encoding="utf8"),
                    re.compile(r"\b(?:reqwest::)?Client::(?:new|builder)\s*\("),
                )


if __name__ == "__main__":
    unittest.main()

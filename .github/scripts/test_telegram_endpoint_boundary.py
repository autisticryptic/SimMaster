"""Static guardrails for the Telegram reverse-proxy endpoint.

The Rust unit tests in `telegram_endpoint.rs` cover normalisation behaviour.
These checks protect the boundaries that are easy to regress from elsewhere in
the tree: the official host must stay the only default, no third-party proxy
domain may be hardcoded, and the bot token must never reach operator-visible
strings unredacted.
"""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
ENDPOINT = ROOT / "backend/src/services/notify/telegram_endpoint.rs"
NOTIFICATION = ROOT / "backend/src/services/notify/notification.rs"
CONFIG = ROOT / "backend/src/platform/config.rs"
MODEL = ROOT / "frontend/src/pages/notifications/notificationModel.ts"
CONTRACTS = ROOT / "frontend/src/api/contracts.ts"


class TelegramEndpointBoundaryTests(unittest.TestCase):
    def setUp(self):
        self.endpoint = ENDPOINT.read_text(encoding="utf8")
        self.notification = NOTIFICATION.read_text(encoding="utf8")

    def test_official_host_is_the_only_default(self):
        self.assertIn(
            'pub const OFFICIAL_API_BASE: &str = "https://api.telegram.org";',
            self.endpoint,
        )
        # An unset api_base must resolve to the official host so existing
        # direct-connection installations are not migrated implicitly.
        self.assertRegex(
            self.endpoint,
            re.compile(
                r"if trimmed\.is_empty\(\) \{\s*return Ok\(OFFICIAL_API_BASE\.to_string\(\)\);",
            ),
        )

    def test_no_third_party_proxy_domain_is_hardcoded(self):
        # Only the official host may appear as a literal telegram endpoint.
        for path in (ENDPOINT, NOTIFICATION, CONFIG, MODEL, CONTRACTS):
            text = path.read_text(encoding="utf8")
            with self.subTest(path=str(path.relative_to(ROOT))):
                for match in re.findall(r"https://[A-Za-z0-9.\-]+", text):
                    if "telegram" not in match and "tg" not in match.split("//")[1]:
                        continue
                    self.assertIn(
                        match,
                        {"https://api.telegram.org", "https://tg.example.com"},
                        f"unexpected telegram endpoint literal {match}",
                    )
        self.assertNotIn("workers.dev", self.endpoint)
        self.assertNotIn("cmliu", self.endpoint)

    def test_https_only_and_no_url_reshaping(self):
        for guard in (
            'if url.scheme() != "https"',
            "TelegramEndpointError::BaseNotHttps",
            "TelegramEndpointError::BaseHasCredentials",
            "TelegramEndpointError::BaseHasQuery",
            "TelegramEndpointError::BaseHasFragment",
            "TelegramEndpointError::BaseTraversal",
        ):
            self.assertIn(guard, self.endpoint, f"missing guard {guard}")

    def test_ip_literal_bases_reuse_the_ssrf_range_check(self):
        self.assertIn(
            "use crate::services::e911::ssrf::is_public_address;", self.endpoint
        )
        self.assertEqual(self.endpoint.count("is_public_address(ip)"), 2)
        self.assertIn("TelegramEndpointError::BaseForbiddenIp", self.endpoint)

    def test_bot_token_charset_is_restricted(self):
        self.assertIn("TelegramEndpointError::TokenInvalidCharacters", self.endpoint)
        self.assertRegex(
            self.endpoint,
            re.compile(r"is_ascii_alphanumeric\(\) \|\| matches!\(ch, ':' \| '_' \| '-'\)"),
        )

    def test_send_path_validates_and_redacts(self):
        # The send path must build the URL through the validated helper rather
        # than formatting the official host inline.
        self.assertNotRegex(
            self.notification,
            re.compile(r'"https://api\.telegram\.org/bot\{\}'),
        )
        self.assertIn(
            'telegram_endpoint::method_url(&config.api_base, &config.bot_token, "sendMessage")',
            self.notification,
        )
        # Both success and error text are redacted, because the token is part of
        # the request path that transports and proxies echo back.
        self.assertIn(
            "telegram_endpoint::redact_token(&err, &config.bot_token)", self.notification
        )
        self.assertIn(
            "telegram_endpoint::redact_token(&ok, &config.bot_token)", self.notification
        )

    def test_config_and_frontend_expose_api_base(self):
        config = CONFIG.read_text(encoding="utf8")
        telegram_struct = config.split("pub struct TelegramConfig {", 1)[1].split("}", 1)[0]
        self.assertIn("pub api_base: String", telegram_struct)
        # Default must stay empty so the direct connection remains the default.
        telegram_default = config.split("impl Default for TelegramConfig {", 1)[1].split(
            "}", 1
        )[0]
        self.assertIn("api_base: String::new()", telegram_default)
        self.assertIn("api_base: ''", MODEL.read_text(encoding="utf8"))
        self.assertIn("api_base: string", CONTRACTS.read_text(encoding="utf8"))


if __name__ == "__main__":
    unittest.main()

"""The frontend must route IMS error codes by exact value, from the backend table.

`errors::code` is the single source of truth. The UI keeps an exact copy in
`cellularImsErrorCodes.ts` and matches `last_error` tokens by equality. These
checks keep the two layers from drifting:

  - the frontend list equals the backend table exactly, so a new backend code
    cannot ship without the UI knowing about it;
  - every code the formatter names is a real backend code (or one of the few
    documented non-table diagnostics), so a rename cannot leave a dead branch;
  - the formatter never falls back to substring matching on a code.
"""
from pathlib import Path
import re
import subprocess
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
ERRORS = ROOT / "backend/src/connectivity/modems/ims/cellular_ims/errors.rs"
CODES_TS = ROOT / "frontend/src/pages/sim/cellularImsErrorCodes.ts"
FORMAT_TS = ROOT / "frontend/src/pages/sim/cellularImsErrorFormat.ts"
GENERATOR = ROOT / ".github/scripts/gen_cellular_ims_error_codes.py"

CONST_RE = re.compile(r'pub const ([A-Z0-9_]+): &str\s*=\s*\n?\s*"([^"]+)"\s*;', re.MULTILINE)
# Code-shaped string literals in the formatter: `'<family>_...'`.
FAMILY = r"(?:volte|cellular_ims|line_volte|line_cellular_ims)"
FORMAT_CODE_RE = re.compile(rf"'({FAMILY}_[a-z0-9_:]+)'")
# Values the formatter may name that are deliberately not table codes:
# `format!` prefixes and the shared connectivity-core REGISTER code.
NON_TABLE_DIAGNOSTICS = {
    "ims_register_initial_receive_failed",
}
NON_TABLE_PREFIXES = re.compile(rf"^{FAMILY}_register_refresh_retry$")


def backend_codes() -> set[str]:
    return {value for _, value in CONST_RE.findall(ERRORS.read_text(encoding="utf8"))}


def frontend_codes() -> list[str]:
    text = CODES_TS.read_text(encoding="utf8")
    body = text[text.index("CELLULAR_IMS_ERROR_CODES = [") : text.index("] as const")]
    return re.findall(r"'([^']+)'", body)


class ImsErrorCodeContractTests(unittest.TestCase):
    def test_frontend_list_matches_backend_table_exactly(self):
        backend, frontend = backend_codes(), frontend_codes()
        self.assertGreater(len(backend), 100, "errors.rs parser regression")
        self.assertEqual(len(frontend), len(set(frontend)), "frontend list repeats a code")
        self.assertEqual(set(frontend) - backend, set(), "frontend lists codes the backend no longer emits")
        self.assertEqual(backend - set(frontend), set(), "backend codes missing from the frontend list")

    def test_generated_file_is_current(self):
        result = subprocess.run([sys.executable, str(GENERATOR), "--check"], check=False)
        self.assertEqual(result.returncode, 0, "run gen_cellular_ims_error_codes.py")

    def test_formatter_names_only_real_codes(self):
        backend = backend_codes()
        named = set(FORMAT_CODE_RE.findall(FORMAT_TS.read_text(encoding="utf8")))
        self.assertGreater(len(named), 20, "formatter parser regression")
        unknown = {
            code
            for code in named
            if code not in backend and code not in NON_TABLE_DIAGNOSTICS and not NON_TABLE_PREFIXES.match(code)
        }
        self.assertEqual(unknown, set(), "formatter names codes the backend does not emit")

    def test_formatter_never_substring_matches_a_code(self):
        text = FORMAT_TS.read_text(encoding="utf8")
        offenders = re.findall(rf"(?:includes|startsWith|endsWith|indexOf)\(\s*'{FAMILY}_[^']*'", text)
        offenders += re.findall(rf"/[^/\n]*\b{FAMILY}_[a-z0-9_]+[^/\n]*/[a-z]*\.test\(", text)
        self.assertEqual(offenders, [], "match codes with cellularImsErrorCodes(), not substrings")

    def test_persisted_name_migration_runs_in_both_workflows(self):
        name = "legacy_volte_persisted_names_migrate_to_cellular_ims"
        source = (ROOT / "backend/src/platform/db.rs").read_text()
        self.assertIn(f"fn {name}(", source)
        for workflow in ("beta-validation.yml", "build-release.yml"):
            text = (ROOT / ".github/workflows" / workflow).read_text()
            self.assertIn(f"platform::db::tests::{name}", text)

    def test_no_frontend_code_is_a_substring_of_another(self):
        codes = frontend_codes()
        offenders = [(a, b) for a in codes for b in codes if a != b and a in b]
        self.assertEqual(offenders, [], "substring relations make token routing ambiguous")


if __name__ == "__main__":
    unittest.main()

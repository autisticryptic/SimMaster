"""The central IMS error-code table must stay internally consistent.

`errors::code` is the single source of truth for the codes that reach
`last_error`, the status API and the UI hint mapping. Two properties have to
hold before the phase-2 rename can be mechanical:

  - `code::ALL` lists every declared constant, so a cross-layer guard can see
    the full set. A code missing from `ALL` is invisible to that guard, which
    is how a shipped code ends up with no UI hint.
  - no code is a substring of another. The frontend historically matched with
    `includes()`, so a prefix relation silently routes a failure to the wrong
    hint -- a defect neither the compiler nor the Rust tests can catch.

The Rust side asserts a count; this asserts the exact sets, so adding one
constant while dropping another from `ALL` cannot slip through.
"""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
ERRORS = ROOT / "backend/src/connectivity/modems/ims/cellular_ims/errors.rs"

# `pub const NAME: &str = "value";` -- the value may wrap onto the next line.
CONST_RE = re.compile(
    r'pub const ([A-Z0-9_]+): &str\s*=\s*\n?\s*"([^"]+)"\s*;',
    re.MULTILINE,
)


def read_errors() -> str:
    return ERRORS.read_text(encoding="utf8")


def declared_constants(text: str) -> dict[str, str]:
    """Map constant name -> code value, excluding the `ALL` aggregate."""
    return {name: value for name, value in CONST_RE.findall(text)}


def all_entries(text: str) -> list[str]:
    """Constant names listed in `code::ALL`, in declaration order."""
    marker = "pub const ALL: &[&str] = &["
    start = text.index(marker) + len(marker)
    body = text[start : text.index("];", start)]
    return [line.strip().rstrip(",") for line in body.splitlines() if line.strip()]


class ImsErrorCodeTableTests(unittest.TestCase):
    def setUp(self):
        self.text = read_errors()
        self.constants = declared_constants(self.text)
        self.all_names = all_entries(self.text)

    def test_table_is_not_empty(self):
        # Guard against a parser regression silently passing every assertion.
        self.assertGreater(len(self.constants), 100)
        self.assertGreater(len(self.all_names), 100)

    def test_all_lists_exactly_the_declared_constants(self):
        declared = set(self.constants)
        listed = set(self.all_names)
        self.assertEqual(
            declared - listed,
            set(),
            "declared constants missing from code::ALL",
        )
        self.assertEqual(
            listed - declared,
            set(),
            "code::ALL references constants that are not declared",
        )

    def test_all_has_no_duplicate_entries(self):
        self.assertEqual(
            len(self.all_names),
            len(set(self.all_names)),
            "code::ALL repeats a constant",
        )

    def test_rust_count_assertion_matches_the_real_total(self):
        # The Rust test pins a literal count; keep it honest from here so the
        # two guards cannot drift apart.
        match = re.search(r"code::ALL\.len\(\),\s*\n?\s*(\d+)", self.text)
        self.assertIsNotNone(match, "the Rust count assertion was removed")
        self.assertEqual(int(match.group(1)), len(self.all_names))

    def test_code_values_are_unique(self):
        values = list(self.constants.values())
        duplicates = {v for v in values if values.count(v) > 1}
        self.assertEqual(duplicates, set(), "two constants share one code value")

    def test_no_code_is_a_substring_of_another(self):
        values = sorted(set(self.constants.values()))
        offenders = [
            (inner, outer)
            for outer in values
            for inner in values
            if inner != outer and inner in outer
        ]
        self.assertEqual(
            offenders,
            [],
            "substring relations make exact-match UI routing ambiguous",
        )

    def test_call_sites_reference_constants_not_literals(self):
        """Only `errors.rs` may spell an IMS error code as a literal.

        Known exclusions, each a value that is not an error code:
          - `volte_ims` is the persisted SMS transport tag;
          - serde `rename` wire keys are the JSON contract;
          - `format!` templates carry a code plus runtime detail;
          - `connectivity/core` must not depend on `cellular_ims`, so its
            cross-layer match arms keep literals.
        """
        src = ROOT / "backend/src"
        allowed = {
            "connectivity/modems/ims/cellular_ims/errors.rs",
            "connectivity/core/register.rs",
        }
        codes = set(self.constants.values())
        offenders = []
        for path in src.rglob("*.rs"):
            rel = path.relative_to(src).as_posix()
            if rel in allowed:
                continue
            lines = path.read_text(encoding="utf8").splitlines()
            # Tests may assert on literals; only production code must use the
            # constants. Everything from the first file-level `#[cfg(test)]`
            # onward is test code.
            limit = next(
                (
                    number
                    for number, line in enumerate(lines)
                    if line.startswith("#[cfg(test)]")
                ),
                len(lines),
            )
            for number, line in enumerate(lines[:limit], start=1):
                if "serde(" in line or "format!" in line or line.lstrip().startswith("//"):
                    continue
                for code in codes:
                    if f'"{code}"' in line:
                        offenders.append(f"{rel}:{number}: {code}")
        self.assertEqual(offenders, [], "use errors::code::* instead of a literal")


if __name__ == "__main__":
    unittest.main()

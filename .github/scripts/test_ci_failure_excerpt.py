import unittest

from ci_failure_excerpt import escape, excerpt


class FailureExcerptTests(unittest.TestCase):
    def test_passing_test_list_cannot_hide_actual_failure(self):
        text = ("test passing ... ok\n" * 1000
                + "\nfailures:\n\n---- legacy stdout ----\n"
                + "assertion failed: persisted configuration mismatch\n")
        result = excerpt(text)
        self.assertIn("persisted configuration mismatch", result)
        self.assertNotIn("test passing", result)
        self.assertLess(len(result), 4096)

    def test_compiler_error_precedes_late_warnings_and_is_escaped(self):
        result = excerpt("header\n\x1b[31merror[E0308]: wrong type\x1b[0m\n" + "warning\n" * 1000)
        self.assertTrue(result.startswith("error[E0308]"))
        self.assertLess(len(result), 4096)
        self.assertEqual(escape("100%\r\nnext"), "100%25%0D%0Anext")

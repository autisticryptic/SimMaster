"""Exercise the portable read-only IMS evidence filter with synthetic logs only."""
import os
import re
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/ims-readonly-evidence.sh"
STAMP = "2026-09-26T03:00:00.123456Z"


def log(message, fields="", prefix=""):
    return f"{STAMP}  INFO {prefix}{message}{(' ' + fields) if fields else ''}\n"


@unittest.skipUnless(shutil.which("perl") and shutil.which("sh"), "requires sh and perl")
class ImsReadonlyEvidenceTests(unittest.TestCase):
    def project(self, text, code=0):
        result = subprocess.run(
            ["sh", str(SCRIPT), "--filter-log"],
            input=text, text=True, capture_output=True, timeout=10,
        )
        self.assertEqual(result.returncode, code, result.stderr)
        self.assertEqual(result.stderr, "")
        return result.stdout

    def test_initial_challenge_and_authenticated_request_are_preserved(self):
        text = (
            log("VoLTE IMS REGISTER request metadata prepared", 'local_family="ipv6" request_bytes=1420')
            + log("VoLTE IMS REGISTER authentication challenge metadata received",
                  "security_server_count=1 usable_security_server_present=true")
            + log("VoLTE IMS authenticated REGISTER request metadata prepared",
                  'registration_mode="ipsec" request_cseq=Some("2 REGISTER")')
        )
        out = self.project(text)
        for token in ["REGISTER_REQUEST", "REGISTER_CHALLENGE", "REGISTER_AUTH_REQUEST",
                      "local_family=ipv6", "security_server_count=1", "request_cseq=2"]:
            self.assertIn(token, out)
        self.assertIn("matched=3", out)

    def test_terminal_response_keeps_status_without_raw_challenge(self):
        out = self.project(log(
            "VoLTE IMS REGISTER terminal response metadata received",
            'error="ims_register_auth_rejected" sip_status=Some(403) auth_rounds=1 '
            'digest_challenge_present=true response_cseq=Some("2 REGISTER") '
            'nonce="private-nonce" authorization="Digest username=private-user"',
        ))
        for token in ["REGISTER_TERMINAL_RESPONSE", "sip_status=403", "auth_rounds=1",
                      "error=ims_register_auth_rejected", "response_cseq=2"]:
            self.assertIn(token, out)
        self.assertNotIn("private", out)
        self.assertNotIn("nonce", out)

    def test_receive_failure_is_not_mislabeled_as_sip_rejection(self):
        out = self.project(log(
            "VoLTE IMS REGISTER failed before a complete response",
            'error="ims_register_initial_receive_failed" auth_rounds=0',
        ))
        self.assertIn("REGISTER_NO_COMPLETE_RESPONSE", out)
        self.assertIn("error=ims_register_initial_receive_failed", out)
        self.assertNotIn("sip_status", out)
        self.assertNotIn("REGISTER_TERMINAL_RESPONSE", out)

    def test_none_status_does_not_become_zero_or_fabricated_response(self):
        out = self.project(log("VoLTE IMS REGISTER terminal response metadata received",
                               "sip_status=None auth_rounds=0"))
        self.assertNotIn("sip_status", out)
        self.assertIn("auth_rounds=0", out)

    def test_quoted_credentials_are_never_reinterpreted_as_fields(self):
        out = self.project(log(
            "VoLTE IMS REGISTER terminal response metadata received",
            'password="something sip_status=200 auth_rounds=999" sip_status=Some(403)',
        ))
        self.assertIn("sip_status=403", out)
        self.assertNotIn("sip_status=200", out)
        self.assertNotIn("auth_rounds", out)
        self.assertNotIn("password", out)
        self.assertNotIn("something", out)

    def test_malformed_quote_does_not_project_embedded_fields(self):
        out = self.project(log("VoLTE IMS REGISTER terminal response metadata received",
                               'password="unterminated sip_status=200 auth_rounds=999'))
        self.assertNotIn("sip_status", out)
        self.assertNotIn("auth_rounds", out)

    def test_escape_sequences_inside_unknown_fields_are_consumed(self):
        out = self.project(log(
            "VoLTE IMS REGISTER terminal response metadata received",
            r'note="escaped \" sip_status=200" sip_status=Some(403)',
        ))
        self.assertIn("sip_status=403", out)
        self.assertNotIn("sip_status=200", out)
        self.assertNotIn("escaped", out)

    def test_option_strings_and_actual_send_ports(self):
        out = self.project(log(
            "VoLTE REGISTER transmit path",
            'register_cseq=Some("3 REGISTER") advertised_port=5062 actual_send_port=Some(5064) '
            'send_route_port=5064 protected_channel=true advertised_pcscf=192.0.2.1:5060 '
            'send_pcscf=[2001:db8::1]:5060',
        ))
        for token in ["register_cseq=3", "advertised_port=5062", "actual_send_port=5064",
                      "send_route_port=5064", "protected_channel=true"]:
            self.assertIn(token, out)
        self.assertNotIn("192.0.2.1", out)
        self.assertNotIn("2001:db8", out)

    def test_identifiers_payloads_addresses_and_unknown_codes_are_omitted(self):
        out = self.project(log(
            "VoLTE IMS restore attempt failed",
            'attempt=2 line_id=line-deadbeef imsi=123456789012345 imei=999999999999999 '
            'iccid=8988888888888888888 pdu=DEADBEEF uri="sip:private@example.invalid" '
            'ck=bad ik=bad rand=bad autn=bad auts=bad token=private-token '
            'error="private-error:password=private-password"',
        ))
        self.assertIn("RESTORE_ATTEMPT_FAILED attempt=2 error=unlisted_error_redacted", out)
        for token in ["123456789", "999999999", "898888", "DEADBEEF", "deadbeef",
                      "example.invalid", "private-", "ck=", "ik=", "token=", "line_id="]:
            self.assertNotIn(token, out)

    def test_known_error_preserves_only_code_not_detail(self):
        out = self.project(log("VoLTE IMS restore attempt failed",
                               'attempt=1 error="cellular_ims_usim_aka_failed:AUTN=private-secret"'))
        self.assertIn("error=cellular_ims_usim_aka_failed", out)
        self.assertNotIn("AUTN", out)
        self.assertNotIn("private-secret", out)

    def test_unquoted_display_error_cannot_inject_fields_from_its_detail(self):
        for value in [
            "private-error:private-secret request_cseq=123456789 sip_status=200",
            "cellular_ims_usim_aka_failed:private-secret request_cseq=123456789 sip_status=200",
            'cellular_ims_usim_aka_failed:"private-secret" request_cseq=123456789 sip_status=200',
        ]:
            with self.subTest(value=value):
                out = self.project(log("VoLTE IMS restore attempt failed", f"attempt=1 error={value}"))
                self.assertIn("RESTORE_ATTEMPT_FAILED attempt=1 error=", out)
                self.assertNotIn("private-secret", out)
                self.assertNotIn("123456789", out)
                self.assertNotIn("sip_status", out)
                self.assertNotIn("request_cseq", out)

    def test_display_error_embedded_quotes_keep_known_code_only(self):
        out = self.project(log("VoLTE IMS restore attempt failed",
                               'attempt=1 error=cellular_ims_usim_aka_failed:"private-secret"'))
        self.assertIn("error=cellular_ims_usim_aka_failed", out)
        self.assertNotIn("private-secret", out)

    def test_actual_initial_authorization_enum_is_preserved(self):
        source = (ROOT / "backend/src/connectivity/modems/ims/cellular_ims/live.rs").read_text(encoding="utf8")
        self.assertTrue('Self::UriFirstEmptyAka => "aka_empty_uri_first"' in source,
                        "Initial authorization label changed; update the evidence enum")
        out = self.project(log("VoLTE IMS REGISTER request metadata prepared",
                               'initial_authorization="aka_empty_uri_first"'))
        self.assertIn("initial_authorization=aka_empty_uri_first", out)
        out = self.project(log("VoLTE IMS REGISTER request metadata prepared",
                               'initial_authorization="private-authorization"'))
        self.assertNotIn("initial_authorization", out)
        self.assertNotIn("private-authorization", out)

    def test_duplicate_fields_are_dropped_instead_of_last_wins(self):
        out = self.project(log("VoLTE IMS REGISTER terminal response metadata received",
                               "sip_status=403 sip_status=200 sip_status=500 auth_rounds=1"))
        self.assertNotIn("sip_status", out)
        self.assertIn("auth_rounds=1", out)

    def test_values_must_match_whole_bounded_types(self):
        out = self.project(log(
            "VoLTE REGISTER transmit path",
            'advertised_port=70000 actual_send_port=Some(1234567890123) protected_channel=true-secret '
            'register_cseq=Some("4 INVITE") send_route_port=5060',
        ))
        self.assertIn("send_route_port=5060", out)
        for key in ["advertised_port=", "actual_send_port=", "protected_channel=", "register_cseq="]:
            self.assertNotIn(key, out)
        out = self.project(log("VoLTE IMS REGISTER terminal response metadata received",
                               "sip_status=403junk auth_rounds=999999999999"))
        self.assertNotIn("sip_status=", out)
        self.assertNotIn("auth_rounds=", out)

    def test_current_and_legacy_log_prefix_ansi_and_crlf(self):
        text = log("VoLTE IMS REGISTER request metadata prepared", 'local_family="ipv4"',
                   "simadmin::connectivity::ims: ")
        text = text.replace("INFO", "\x1b[32mINFO\x1b[0m").replace("\n", "\r\n")
        self.assertIn("REGISTER_REQUEST local_family=ipv4", self.project(text))
        self.assertIn("REGISTER_REQUEST", self.project(text.replace(".123456Z", "+08:00")))

    def test_raw_sip_unrelated_logs_and_forged_quoted_messages_are_ignored(self):
        text = (
            'SIP/2.0 403 Forbidden\nWWW-Authenticate: private\n'
            + log("Unrelated", 'note="VoLTE IMS REGISTER terminal response metadata received" sip_status=403')
            + 'not-a-timestamp INFO VoLTE IMS REGISTER request metadata prepared\n'
        )
        self.assertEqual(self.project(text),
                         "LOG_FILTER_SUMMARY matched=0 retained=0 oversized=0 journal_read_failed=false\n")

    def test_initial_bearer_and_pcscf_failure_metadata(self):
        out = self.project(
            log("VoLTE P-CSCF observation failed; explicit configuration and bearer DNS remain available")
            + log("VoLTE retained IMS bearer ended", 'error="cellular_ims_bearer_session_lost:private-detail"')
            + log("VoLTE IMS restore attempt failed", "attempt=3"))
        for event in ["PCSCF_OBSERVATION_FAILED", "BEARER_ENDED", "RESTORE_ATTEMPT_FAILED"]:
            self.assertIn(event, out)
        self.assertNotIn("private-detail", out)

    def test_refresh_still_distinct_from_initial_success(self):
        out = self.project(
            log("VoLTE IMS REGISTER success metadata received", 'register_phase="initial" expires_seconds=3600')
            + log("VoLTE IMS REGISTER success metadata received", 'register_phase="refresh" expires_seconds=3600')
            + log("VoLTE REGISTER refresh will retry on the existing bearer"))
        self.assertIn("register_phase=initial", out)
        self.assertIn("register_phase=refresh", out)
        self.assertIn("REFRESH_RETRY", out)

    def test_refresh_schedule_retains_actual_source_field_names(self):
        out = self.project(log(
            "VoLTE IMS refresh scheduled",
            "refresh_after_seconds=3000 network_refresh_after_seconds=3000 lease_seconds=3600 "
            "test_override_seconds=None protected=true",
        ) + log("VoLTE REGISTER refresh will retry on the existing bearer", "retry_after_seconds=30"))
        for fact in ["refresh_after_seconds=3000", "network_refresh_after_seconds=3000",
                     "lease_seconds=3600", "protected=true", "retry_after_seconds=30"]:
            self.assertIn(fact, out)
        self.assertNotIn("test_override_seconds", out)

    def test_long_line_is_discarded_then_parser_recovers_at_newline(self):
        text = log("VoLTE IMS REGISTER request metadata prepared", 'secret="' + "x" * 100000 + '"')
        out = self.project(text + log("VoLTE IMS restore attempt failed", "attempt=3"))
        self.assertNotIn("REGISTER_REQUEST", out)
        self.assertIn("RESTORE_ATTEMPT_FAILED attempt=3", out)
        self.assertIn("oversized=1", out)

    def test_output_is_bounded_and_eof_without_newline_is_supported(self):
        out = self.project("".join(log("VoLTE IMS restore attempt failed", f"attempt={i}")
                                   for i in range(150)).rstrip("\n"))
        self.assertEqual(len(out.splitlines()), 121)
        self.assertIn("matched=150 retained=120", out)
        self.assertNotIn("attempt=0\n", out)
        self.assertIn("attempt=149", out)

    def test_journal_read_error_is_reported_not_an_empty_success(self):
        out = self.project("__JOURNAL_READ_FAILED__\n", code=1)
        self.assertIn("journal_read_failed=true", out)

    def test_filter_help_and_bad_arguments_never_probe_host(self):
        for args, expected in [(["--help"], 0), (["--unknown"], 2), (["--filter-log", "extra"], 2)]:
            result = subprocess.run(["sh", str(SCRIPT), *args], capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, expected)
            self.assertNotIn("__IMS_READ_ONLY_EVIDENCE_V2__", result.stdout)
            self.assertNotIn("running_program_sha256", result.stdout)

    def test_log_events_follow_current_source_contract(self):
        script = SCRIPT.read_text(encoding="utf8")
        events = re.findall(r'^      \["([^"]+)", "[A-Z_]+"\]', script, re.M)
        source = "\n".join((ROOT / name).read_text(encoding="utf8") for name in [
            "backend/src/connectivity/modems/ims/cellular_ims/live.rs",
            "backend/src/connectivity/modems/ims/cellular_ims/channel.rs",
            "backend/src/api/handlers.rs", "backend/src/services/ue_worker.rs",
        ])
        self.assertGreater(len(events), 20)
        for event in events:
            with self.subTest(event=event):
                self.assertTrue('"' + event + '"' in source, f"Missing log event: {event}")

    @unittest.skipUnless(Path("/proc/self/stat").exists(), "collector fixture requires Linux procfs")
    def test_collector_uses_running_executable_and_pinned_process_snapshot(self):
        # Only systemctl/journalctl stubs run; no device or actual service queries.
        with tempfile.TemporaryDirectory(prefix="ims-evidence-test-") as directory:
            root = Path(directory)
            systemctl = root / "systemctl"
            systemctl.write_text(
                '#!/bin/sh\n'
                'case "$*" in\n'
                '  "show simadmin.service -p MainPID --value")\n'
                '    if [ -r "$EVIDENCE_TEST_STATE" ]; then IFS= read -r pid < "$EVIDENCE_TEST_STATE";\n'
                '    else pid=$EVIDENCE_TEST_PID; fi; printf "%s\\n" "$pid" ;;\n'
                '  *) printf "MainPID=0\\nNRestarts=0\\nActiveState=active\\nSubState=running\\nType=simple\\n" ;;\n'
                'esac\n', encoding="utf8",
            )
            journalctl = root / "journalctl"
            journalctl.write_text(
                '#!/bin/sh\nprintf "%s" "$EVIDENCE_TEST_LOG"\n'
                'if [ "$EVIDENCE_TEST_CHANGE_PID" = 1 ]; then printf "0\\n" > "$EVIDENCE_TEST_STATE"; fi\n'
                'exit "$EVIDENCE_TEST_EXIT"\n', encoding="utf8",
            )
            systemctl.chmod(0o755)
            journalctl.chmod(0o755)
            env = dict(os.environ, PATH=directory + os.pathsep + os.environ["PATH"],
                       EVIDENCE_TEST_PID=str(os.getpid()), EVIDENCE_TEST_EXIT="0",
                       EVIDENCE_TEST_STATE=str(root / "state"), EVIDENCE_TEST_CHANGE_PID="0",
                       EVIDENCE_TEST_LOG=log("VoLTE IMS restore attempt failed", "attempt=1"))
            result = subprocess.run(["sh", str(SCRIPT)], capture_output=True, text=True, env=env, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("RESTORE_ATTEMPT_FAILED attempt=1", result.stdout)
            self.assertRegex(result.stdout, r"running_program_sha256=[0-9a-f]{64}")
            self.assertIn("running_process_stable_during_sample=true", result.stdout)
            refs = re.findall(r"running_process_reference(?:_after)?=(\d+:\d+)", result.stdout)
            self.assertEqual(len(refs), 2)
            self.assertEqual(refs[0], refs[1])
            self.assertIn("journal_scope=current_boot_all_service_processes", result.stdout)
            # A restart mid-sample invalidates the current-process association.
            env["EVIDENCE_TEST_CHANGE_PID"] = "1"
            result = subprocess.run(["sh", str(SCRIPT)], capture_output=True, text=True, env=env, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("running_program_sha256", result.stdout)
            self.assertIn("running_process_stable_during_sample=false", result.stdout)
            self.assertIn("running_process_reference_after=unavailable", result.stdout)
            env["EVIDENCE_TEST_CHANGE_PID"] = "0"
            (root / "state").unlink()
            env["EVIDENCE_TEST_PID"] = "0"
            result = subprocess.run(["sh", str(SCRIPT)], capture_output=True, text=True, env=env, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("running_process_stable_during_sample=false", result.stdout)
            self.assertNotIn("running_program_sha256", result.stdout)
            env["EVIDENCE_TEST_EXIT"] = "1"
            result = subprocess.run(["sh", str(SCRIPT)], capture_output=True, text=True, env=env, timeout=10)
            self.assertEqual(result.returncode, 1)
            self.assertIn("journal_read_failed=true", result.stdout)

    def test_error_allowlist_follows_known_source_codes(self):
        script = SCRIPT.read_text(encoding="utf8")
        error_table = script.split("my %errors = map { $_ => 1 } qw(", 1)[1].split(");", 1)[0]
        source = "\n".join((ROOT / name).read_text(encoding="utf8") for name in [
            "backend/src/connectivity/core/register.rs",
            "backend/src/connectivity/modems/ims/cellular_ims/errors.rs",
            "backend/src/connectivity/modems/ims/cellular_ims/live.rs",
        ])
        for code in error_table.split():
            with self.subTest(code=code):
                self.assertTrue('"' + code + '"' in source, f"Missing error code: {code}")

    def test_shell_syntax_and_readonly_boundary(self):
        subprocess.run(["sh", "-n", str(SCRIPT)], check=True, capture_output=True)
        script = SCRIPT.read_text(encoding="utf8")
        for forbidden in ["mmcli ", "qmicli ", "AT+", "curl ", "systemctl restart", "systemctl stop",
                          "systemctl start", "config.yaml", "data.db", "ip netns exec", "ExecStart"]:
            self.assertNotIn(forbidden, script)
        self.assertIn('"/proc/$running_pid/exe"', script)
        self.assertIn("running_process_stable_during_sample", script)
        self.assertIn("-b -u simadmin.service --no-pager -o cat -n 6000", script)


if __name__ == "__main__":
    unittest.main()

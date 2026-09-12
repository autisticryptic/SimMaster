import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from release_publication import resolve_publication


class ReleasePublicationTests(unittest.TestCase):
    def test_master_push_preserves_existing_release_flow(self):
        self.assertEqual(
            resolve_publication("push", "refs/heads/master")["publish_release"],
            "true",
        )

    def test_development_pushes_never_publish(self):
        for ref in (
            "refs/heads/dev/1.1.5-modem-backends",
            "refs/heads/fix/sim02-catalog-aka-baseline",
            "refs/heads/fix/1.1.4-beta3-cellular-ims",
            "refs/heads/master-copy",
            "refs/tags/v1.1.5-beta1",
            "",
        ):
            with self.subTest(ref=ref):
                self.assertEqual(
                    resolve_publication("push", ref, True)["publish_release"],
                    "false",
                )

    def test_dispatch_defaults_to_artifacts_even_on_master(self):
        for requested in ("", "false", False, None, "0", "yes", 1):
            with self.subTest(requested=requested):
                self.assertEqual(
                    resolve_publication(
                        "workflow_dispatch", "refs/heads/master", requested
                    )["publish_release"],
                    "false",
                )

    def test_only_explicit_master_dispatch_publishes(self):
        for requested in (True, "true", " TRUE "):
            with self.subTest(requested=requested):
                result = resolve_publication(
                    "workflow_dispatch", "refs/heads/master", requested
                )
                self.assertEqual(result["publish_release"], "true")
                self.assertEqual(
                    result["publication_reason"], "explicit_master_dispatch"
                )

    def test_development_dispatch_cannot_override_the_ref_guard(self):
        for ref in (
            "refs/heads/dev/1.1.5-modem-backends",
            "refs/heads/fix/sim02-catalog-aka-baseline",
            "refs/tags/v1.1.5-beta1",
        ):
            with self.subTest(ref=ref):
                self.assertEqual(
                    resolve_publication("workflow_dispatch", ref, "true"),
                    {
                        "publish_release": "false",
                        "publication_reason": "non_release_ref",
                    },
                )

    def test_unknown_events_fail_closed(self):
        for event in ("", "pull_request", "pull_request_target", "schedule"):
            with self.subTest(event=event):
                self.assertEqual(
                    resolve_publication(event, "refs/heads/master", True)[
                        "publish_release"
                    ],
                    "false",
                )

    def test_malformed_request_does_not_inject_outputs(self):
        result = resolve_publication(
            "workflow_dispatch",
            "refs/heads/master",
            "true\npublication_reason=overridden",
        )
        self.assertEqual(result["publish_release"], "false")
        self.assertTrue(all("\n" not in value for value in result.values()))

    def test_cli_writes_default_artifact_policy(self):
        script = Path(__file__).with_name("release_publication.py").resolve()
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "output.txt"
            env = {
                **os.environ,
                "GITHUB_OUTPUT": str(output),
                "EVENT_NAME": "workflow_dispatch",
                "GITHUB_REF": "refs/heads/master",
                "INPUT_PUBLISH_RELEASE": "",
            }
            subprocess.run(
                [sys.executable, str(script)],
                env=env,
                check=True,
                capture_output=True,
            )
            values = dict(
                line.split("=", 1) for line in output.read_text().splitlines()
            )
            self.assertEqual(
                values,
                {
                    "publish_release": "false",
                    "publication_reason": "dispatch_artifacts_only",
                },
            )

    def test_workflows_use_the_tested_gate_and_minimum_permissions(self):
        root = Path(__file__).resolve().parents[1]
        build = (root / "workflows/build-release.yml").read_text()
        validation = (root / "workflows/beta-validation.yml").read_text()
        before_release, release = build.split("\n  release:\n", 1)
        self.assertIn("permissions:\n  contents: read\n", before_release)
        self.assertIn("run: python3 .github/scripts/release_publication.py", build)
        self.assertIn(
            "publish_release: ${{ steps.publication.outputs.publish_release }}",
            build,
        )
        self.assertIn(
            "if: github.ref == 'refs/heads/master' && needs.prepare.outputs.publish_release == 'true'",
            release.split("    steps:", 1)[0],
        )
        self.assertIn("    permissions:\n      contents: write\n", release)
        self.assertNotIn("uses: softprops/action-gh-release@", before_release)
        self.assertIn("needs: [prepare, build, check-tests]", release)
        for workflow in (build, validation):
            self.assertIn(
                "      - dev/1.1.5-modem-backends\n",
                workflow.split("  workflow_dispatch:", 1)[0],
            )
        self.assertIn(
            "      publish_release:\n"
            "        description: '仅 master 可发布；开发分支始终只生成候选 artifact'\n"
            "        required: false\n"
            "        type: boolean\n"
            "        default: false",
            build,
        )


if __name__ == "__main__":
    unittest.main()

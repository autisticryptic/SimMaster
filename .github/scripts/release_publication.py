"""Decide whether a build may publish; development refs are artifacts-only."""
import os
from pathlib import Path


MASTER_REF = "refs/heads/master"


def resolve_publication(event, ref, requested=False):
    """Fail closed except master pushes or explicitly authorized master dispatches."""
    if ref != MASTER_REF:
        return {"publish_release": "false", "publication_reason": "non_release_ref"}
    if event == "push":
        return {"publish_release": "true", "publication_reason": "master_push"}
    if event != "workflow_dispatch":
        return {"publish_release": "false", "publication_reason": "unsupported_event"}
    explicit = requested is True or (
        isinstance(requested, str) and requested.strip().lower() == "true"
    )
    return {
        "publish_release": "true" if explicit else "false",
        "publication_reason": (
            "explicit_master_dispatch" if explicit else "dispatch_artifacts_only"
        ),
    }


def main():
    values = resolve_publication(
        os.environ.get("EVENT_NAME", ""),
        os.environ.get("GITHUB_REF", ""),
        os.environ.get("INPUT_PUBLISH_RELEASE", ""),
    )
    with Path(os.environ["GITHUB_OUTPUT"]).open("a", encoding="utf8") as output:
        for key, value in values.items():
            output.write(f"{key}={value}\n")
    print(
        f"Publish release: {values['publish_release']} "
        f"({values['publication_reason']})"
    )


if __name__ == "__main__":
    main()

"""Refuse reusing a published release/tag; never treat API failure as absence."""
import os
import re
from urllib.error import HTTPError
from urllib.parse import quote
from urllib.request import HTTPRedirectHandler, Request, build_opener

from release_version import resolve_version


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


def require_absent(kind, status):
    if status == 404:
        return
    if status == 200:
        raise ValueError(f"release_target_{kind}_already_exists")
    raise ValueError(f"release_target_{kind}_check_failed_http_{status}")


def check_target(repository, tag, get_status):
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
        raise ValueError("release_repository_invalid")
    if not tag.startswith("v"):
        raise ValueError("release_tag_invalid")
    resolve_version(tag[1:])  # Reuse strict semver validation, not shell interpolation.
    encoded = quote(tag, safe="")
    for kind, suffix in (("tag", f"git/ref/tags/{encoded}"), ("release", f"releases/tags/{encoded}")):
        require_absent(kind, get_status(f"https://api.github.com/repos/{repository}/{suffix}"))


def main():
    token = os.environ["GITHUB_TOKEN"]
    opener = build_opener(NoRedirect())

    def get_status(url):
        request = Request(url, headers={
            "Accept": "application/vnd.github+json",
            "Authorization": "Bearer " + token,
            "User-Agent": "SimMaster-release-target-guard",
        })
        try:
            with opener.open(request, timeout=30) as response:
                return response.status
        except HTTPError as error:
            return error.code
        # No response means failure, never permission to publish. Do not log
        # request headers, response bodies or credentials on that path.

    check_target(os.environ["GITHUB_REPOSITORY"], os.environ["RELEASE_TAG"], get_status)
    print("Verified new release target: no existing tag or release")


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        print(str(error) if isinstance(error, ValueError) else "release_target_check_unavailable")
        raise SystemExit(1)

"""Keep the actual failure visible in size-limited public check annotations."""
from pathlib import Path
import re
import sys


def excerpt(text, limit=3000):
    text = re.sub(r"\x1b\[[0-9;]*m", "", text)
    failure = text.find("\nfailures:")
    compiler = re.search(r"(?m)^error(?:\[|:)", text)
    if failure >= 0:
        text = text[failure:].strip()
    elif compiler:
        text = text[compiler.start():].strip()
    else:
        text = "\n".join(text.splitlines()[-24:]).strip()
    return text[:limit] + ("\n[See uploaded log for remaining details.]" if len(text) > limit else "")


def escape(text):
    return text.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")


if __name__ == "__main__":
    text = Path(sys.argv[1]).read_text(encoding="utf8", errors="replace")
    print(f"::error title={escape(sys.argv[2])}::{escape(excerpt(text))}")

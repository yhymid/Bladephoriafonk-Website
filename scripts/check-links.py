#!/usr/bin/env python3
"""Checks relative links and #anchors between the Markdown files of the repo.

External links (http, https, mailto) are not fetched. Exit code 1 lists every
link to a missing file or heading.
"""
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LINK = re.compile(r"(?<!!)\[[^\]]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
HEADING = re.compile(r"^(#{1,6})\s+(.*?)\s*#*\s*$")
FENCE = re.compile(r"^\s*(```|~~~)")


def slug(heading: str) -> str:
    """GitHub's anchor for a heading."""
    text = re.sub(r"`([^`]*)`", r"\1", heading)
    text = re.sub(r"\[([^\]]*)\]\([^)]*\)", r"\1", text)
    text = text.strip().lower()
    text = re.sub(r"[^\w\- ]", "", text, flags=re.UNICODE)
    return text.replace(" ", "-")


def scan(path: Path):
    """Anchors defined in a file and links used in it (outside code)."""
    anchors, links, seen = set(), [], {}
    in_code = False
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if FENCE.match(line):
            in_code = not in_code
            continue
        if in_code:
            continue
        heading = HEADING.match(line)
        if heading:
            base = slug(heading.group(2))
            count = seen.get(base, 0)
            seen[base] = count + 1
            anchors.add(base if count == 0 else f"{base}-{count}")
        for target in LINK.findall(re.sub(r"`[^`]*`", "", line)):
            links.append((number, target))
    return anchors, links


def main() -> int:
    tracked = subprocess.run(
        ["git", "ls-files", "*.md"], cwd=ROOT, capture_output=True, text=True, check=True
    ).stdout.split()
    files = {ROOT / name: scan(ROOT / name) for name in tracked}
    errors = []
    for path, (_, links) in files.items():
        for number, target in links:
            if re.match(r"^[a-z][a-z0-9+.-]*:", target):
                continue
            file_part, _, anchor = target.partition("#")
            linked = (path.parent / file_part).resolve() if file_part else path
            where = f"{path.relative_to(ROOT)}:{number}"
            if not linked.exists():
                errors.append(f"{where}: missing file {target}")
                continue
            if anchor and linked.suffix == ".md":
                anchors = files.get(linked, scan(linked))[0]
                if anchor not in anchors:
                    errors.append(f"{where}: missing heading #{anchor} in {file_part or path.name}")
    for error in errors:
        print(error)
    print(f"{len(files)} files, {sum(len(l) for _, l in files.values())} links, {len(errors)} broken")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())

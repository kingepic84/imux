#!/usr/bin/env python3
"""A release has notes: release-notes/vX.Y.Z.md exists for the tag, says something, and is finished.

    scripts/check-release-notes.py vX.Y.Z            check the notes of that tag
    scripts/check-release-notes.py --dir DIR vX.Y.Z  look in DIR instead of release-notes/

The release workflow publishes that file as the text of the release, so a tag without it, with an
empty one, or with a placeholder in it (TODO, TBD, FIXME) is refused before anything is built.
Exit status 0 when the notes are fine, 1 and the reason when not.
"""
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
TOP = os.path.dirname(HERE)
MIN_CHARS = 200
PLACEHOLDER = re.compile(r"\b(TODO|TBD|FIXME)\b")


def problems(tag, directory):
    if not re.fullmatch(r"v\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?", tag):
        return [f"{tag!r} is not a version tag (vX.Y.Z)"]
    path = os.path.join(directory, f"{tag}.md")
    if not os.path.isfile(path):
        return [f"there are no release notes: write {os.path.relpath(path, TOP) if path.startswith(TOP) else path} before tagging {tag}"]
    try:
        text = open(path, encoding="utf8").read()
    except (OSError, UnicodeDecodeError) as e:
        return [f"{path} cannot be read as UTF-8 text ({e})"]
    found = []
    body = text.strip()
    if len(body) < MIN_CHARS:
        found.append(f"{path} says too little ({len(body)} characters; at least {MIN_CHARS}): what changed, what to know when upgrading, which file to download")
    if "\x00" in text:
        found.append(f"{path} holds a NUL character")
    m = PLACEHOLDER.search(text)
    if m:
        found.append(f"{path} is not finished: it still says {m.group(0)}")
    return found


def main(argv):
    args = argv[1:]
    directory = os.path.join(TOP, "release-notes")
    if args[:1] == ["--dir"]:
        directory, args = args[1], args[2:]
    if len(args) != 1:
        print(__doc__.strip().splitlines()[0])
        return 2
    found = problems(args[0], directory)
    for p in found:
        print(f"check-release-notes: {p}")
    if found:
        return 1
    print(f"check-release-notes: {args[0]} has release notes")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

#!/usr/bin/env python3
"""Checks that the version of imux follows Semantic Versioning (https://semver.org) and has not
gone backwards. Run from anywhere in the repository; CI runs it on every push and on every tag.

    scripts/check-version.py          check the version of the tree
    scripts/check-version.py v0.1.1   also check that this tag is the release of the tree

What it holds to:
  - `version` in Cargo.toml is a valid semantic version (MAJOR.MINOR.PATCH, with an optional
    pre-release and build part), and Cargo.lock names the same version for imux;
  - every release tag (v*) is a valid semantic version, and the tree is not behind the newest of
    them: a release is never reused or taken back, and a version never goes down;
  - for a tag: it is exactly `v` plus the version, and that version is higher than every other
    tag, so a release can be made only by raising the version first.

Whether a change is a patch, a minor or a major one is for the person who makes it: breaking
what users rely on (key defaults, option names, command flags, the config and snapshot formats,
the socket protocol) needs a new major version, and while the version is 0.y.z it needs at least a
new minor one. The script cannot see that, so it checks only what can be checked.
"""
import re
import subprocess
import sys

IDENT = r"(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)"
SEMVER = re.compile(
    rf"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-({IDENT}(?:\.{IDENT})*))?(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$"
)


def parse(text):
    """The version as a sort key (semver precedence: build metadata does not count), or None."""
    m = SEMVER.match(text)
    if not m:
        return None
    major, minor, patch, pre, _build = m.groups()
    if pre is None:
        # a release is higher than any of its pre-releases
        pre_key = (1,)
    else:
        parts = []
        for ident in pre.split("."):
            # numbers are lower than words, and are compared as numbers
            parts.append((0, int(ident), "") if ident.isdigit() else (1, 0, ident))
        pre_key = (0, tuple(parts))
    return (int(major), int(minor), int(patch), pre_key)


def git(*args):
    return subprocess.run(["git", *args], capture_output=True, text=True, check=True).stdout


def root():
    return git("rev-parse", "--show-toplevel").strip()


def cargo_toml_version(top):
    text = open(f"{top}/Cargo.toml", encoding="utf8").read()
    m = re.search(r"^\[package\][^\[]*?^version\s*=\s*\"([^\"]*)\"", text, re.M | re.S)
    return m.group(1) if m else None


def cargo_lock_version(top):
    try:
        text = open(f"{top}/Cargo.lock", encoding="utf8").read()
    except FileNotFoundError:
        return None
    m = re.search(r'^name = "imux"\nversion = "([^"]*)"', text, re.M)
    return m.group(1) if m else None


def main(argv):
    tag = argv[1] if len(argv) > 1 and argv[1] else None
    top = root()
    problems = []

    version = cargo_toml_version(top)
    if version is None:
        print("check-version: no version in the [package] section of Cargo.toml")
        return 1
    key = parse(version)
    if key is None:
        problems.append(f"Cargo.toml says version {version!r}, which is not a semantic version (MAJOR.MINOR.PATCH)")

    locked = cargo_lock_version(top)
    if locked != version:
        problems.append(f"Cargo.lock has imux {locked!r} but Cargo.toml says {version!r}: run cargo build and commit the lock")

    tags = [t for t in git("tag", "--list", "v*").split() if re.match(r"^v\d", t)]
    released = {}
    for t in tags:
        k = parse(t[1:])
        if k is None:
            problems.append(f"the tag {t} is not a semantic version")
        else:
            released[t] = k
    others = {t: k for t, k in released.items() if t != tag}

    if key is not None and others:
        newest = max(others, key=others.get)
        if key < others[newest]:
            problems.append(f"version {version} is lower than the release {newest}: a version never goes down")

    if tag is not None:
        if tag != f"v{version}":
            problems.append(f"the tag is {tag} but Cargo.toml says version {version}: a release is v plus the version")
        if key is not None and others:
            newest = max(others, key=others.get)
            if key <= others[newest]:
                problems.append(f"{tag} is not higher than the release {newest}: raise the version in Cargo.toml first")

    if problems:
        for p in problems:
            print(f"check-version: {p}")
        return 1
    what = f"{version}, release {tag}" if tag else version
    print(f"check-version: {what} is a valid version and follows the releases so far ({len(released)} tag{'s' if len(released) != 1 else ''})")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

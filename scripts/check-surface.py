#!/usr/bin/env python3
"""Holds the version of imux to what changed in the part of it that people rely on.

The user-facing surface is what a person or a script can see from outside: the default keys, the
options with their defaults, the commands and the flags of imux's own command line, the version of
the plugin API and of the snapshot format. This script reads it from a built imux and compares it
with the surface of the last release, which is kept in surface/vX.Y.Z.txt. What changed decides how
far the version must have been raised:

  - something taken away or changed (a key, an option or its default, a command, a flag, an API or
    snapshot version): a break, which needs a new major version (a new minor one while the major
    version is 0, where a break is allowed to come with a minor one);
  - something only added: needs at least a new minor version;
  - nothing changed: any version at or above the release is fine (a patch, say).

    scripts/check-surface.py [vX.Y.Z]   check the built imux (target/debug/imux) against the last
                                         release; with a tag, also that surface/<tag>.txt is the
                                         surface of this tree
    scripts/check-surface.py --binary PATH ...      another imux than target/debug/imux
    scripts/check-surface.py --print    print the surface of the built imux
    scripts/check-surface.py --write    write surface/v<version>.txt, as part of making a release
    scripts/check-surface.py --compare BASE.txt NEW.txt BASE_VERSION NEW_VERSION
                                         only the verdict for two surface files and two versions

It cannot see a change in what something does while its name and default stay (a command that acts
differently, an output format): that stays for review. Run it on Linux, as CI does: a few defaults
differ per system.
"""
import importlib.util
import os
import re
import subprocess
import sys
import tempfile

sys.dont_write_bytecode = True  # importing check-version.py must not leave a __pycache__ in scripts/

HERE = os.path.dirname(os.path.abspath(__file__))
TOP = os.path.dirname(HERE)
SURFACE_DIR = os.path.join(TOP, "surface")


def load_semver():
    """parse() of check-version.py (its name has a dash, so it is loaded by path)."""
    spec = importlib.util.spec_from_file_location("check_version", os.path.join(HERE, "check-version.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


SEMVER = load_semver()


def run(binary, args, env, timeout=60):
    out = subprocess.run([binary, *args], env=env, capture_output=True, text=True, timeout=timeout)
    return out.stdout


def collect(binary):
    """The surface of a built imux, as sections of text lines."""
    with tempfile.TemporaryDirectory(prefix="imux-surface-") as tmp:
        env = {
            "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
            "HOME": tmp,
            "TMPDIR": tmp,
            "IMUX_STATE_DIR": os.path.join(tmp, "state"),
            "IMUX_CONFIG": "/dev/null",
            "TERM": "xterm-256color",
            "LC_ALL": "C",
        }
        try:
            subprocess.run([binary, "new-session", "-d", "-s", "surface"], env=env, check=True, capture_output=True, timeout=60)
            keys = run(binary, ["list-keys"], env)
            options = run(binary, ["show-options", "-g"], env)
            run(binary, ["save", "surface"], env)
            snapshot = open(os.path.join(tmp, "state", "snapshots", "surface.imux"), encoding="utf8").readline().strip()
        finally:
            subprocess.run([binary, "kill-server"], env=env, capture_output=True, timeout=60)
        usage = run(binary, ["list-commands"], env)
        words = run(binary, ["completions", "bash"], env)
        api = run(binary, ["api", "version"], env).strip()

    sections = {"keys": [], "options": [], "commands": [], "usage": [], "versions": []}
    for line in keys.splitlines():
        m = re.match(r"^bind-key (?:-r )?-T (\S+)\s+(\S+)\s+(.*)$", line)
        sections["keys"].append(f"{m.group(1)} {m.group(2)}\t{m.group(3).strip()}" if m else f"?\t{line.strip()}")
    for line in options.splitlines():
        name, _, value = line.partition(" ")
        sections["options"].append(f"{name}\t{value}")
    m = re.search(r'compgen -W "([^"]*)"', words)
    sections["commands"] = sorted(set(m.group(1).split())) if m else []
    sections["usage"] = sorted(set(t for t in re.split(r"[\s\[\]|()]+", usage) if t))
    sections["versions"] = [f"api\t{api}", f"snapshot\t{snapshot}"]
    for name in ("keys", "options"):
        sections[name].sort()
    return sections


def render(sections):
    lines = ["# The user-facing surface of imux. Made by scripts/check-surface.py; do not edit by hand."]
    for name in ("keys", "options", "commands", "usage", "versions"):
        lines.append(f"[{name}]")
        lines.extend(sections[name])
    return "\n".join(lines) + "\n"


def parse(text):
    sections, current = {}, None
    for line in text.splitlines():
        if line.startswith("#") or not line:
            continue
        m = re.match(r"^\[(\w+)\]$", line)
        if m:
            current = sections.setdefault(m.group(1), [])
        elif current is not None:
            current.append(line)
    return sections


def mapping(lines):
    """`key<TAB>value` lines as a dict; a line with no tab is a key with no value."""
    out = {}
    for line in lines:
        key, _, value = line.partition("\t")
        out[key] = value
    return out


def diff(base, new):
    """(breaking, added): what a person would have to notice, as text lines."""
    breaking, added = [], []
    for name in ("keys", "options", "versions"):
        b, n = mapping(base.get(name, [])), mapping(new.get(name, []))
        for key in sorted(b):
            if key not in n:
                breaking.append(f"{name}: {key} is gone")
            elif b[key] != n[key]:
                breaking.append(f"{name}: {key} was {b[key]!r} and is now {n[key]!r}")
        for key in sorted(n):
            if key not in b:
                added.append(f"{name}: {key} is new")
    for name in ("commands", "usage"):
        b, n = set(base.get(name, [])), set(new.get(name, []))
        breaking.extend(f"{name}: {word} is gone" for word in sorted(b - n))
        added.extend(f"{name}: {word} is new" for word in sorted(n - b))
    return breaking, added


def required(breaking, added, base_version):
    """The lowest rise of the version that the changes allow: 'major', 'minor' or 'none'."""
    if breaking:
        return "minor" if base_version[0] == 0 else "major"
    if added:
        return "minor"
    return "none"


def rise(base_version, new_version):
    """How far `new_version` is above `base_version`: 'major', 'minor', 'patch', 'none' or 'below'."""
    if new_version < base_version:
        return "below"
    if new_version == base_version:
        return "none"
    if new_version[0] != base_version[0]:
        return "major"
    if new_version[1] != base_version[1]:
        return "minor"
    return "patch"


ORDER = {"below": -1, "none": 0, "patch": 1, "minor": 2, "major": 3}


def verdict(base, new, base_version, new_version):
    """(ok, lines): do the versions fit what changed between two surfaces?"""
    breaking, added = diff(base, new)
    need = required(breaking, added, base_version)
    got = rise(base_version, new_version)
    lines = []
    ok = got != "below" and ORDER[got] >= ORDER[need]
    if got == "below":
        lines.append(f"the version is below the release it is compared with")
    elif ORDER[got] < ORDER[need]:
        why = "a break" if breaking else "an addition"
        lines.append(f"{why} since the release needs the version raised by at least a {need} step, and it has been raised by {got if got != 'none' else 'nothing'}")
    lines.extend(f"  break: {b}" for b in breaking)
    lines.extend(f"  added: {a}" for a in added)
    return ok, lines


def version_key(text):
    return SEMVER.parse(text)


def baselines():
    """[(version text, key, path)] of surface/v*.txt, lowest first."""
    found = []
    if os.path.isdir(SURFACE_DIR):
        for name in os.listdir(SURFACE_DIR):
            m = re.match(r"^v(.+)\.txt$", name)
            if m and version_key(m.group(1)) is not None:
                found.append((m.group(1), version_key(m.group(1)), os.path.join(SURFACE_DIR, name)))
    return sorted(found, key=lambda f: f[1])


def cargo_version():
    return SEMVER.cargo_toml_version(TOP)


def main(argv):
    args = argv[1:]
    if args[:1] == ["--compare"]:
        base_file, new_file, base_v, new_v = args[1:5]
        ok, lines = verdict(parse(open(base_file, encoding="utf8").read()), parse(open(new_file, encoding="utf8").read()), version_key(base_v), version_key(new_v))
        print("check-surface: " + ("the versions fit what changed" if ok else "the versions do not fit what changed"))
        for line in lines:
            print(line)
        return 0 if ok else 1

    binary = os.path.join(TOP, "target", "debug", "imux")
    mode, tag = "check", None
    while args:
        a = args.pop(0)
        if a == "--binary":
            binary = args.pop(0)
        elif a in ("--print", "--write"):
            mode = a[2:]
        elif a:
            tag = a
    current = collect(binary)
    text = render(current)
    version = cargo_version()
    if mode == "print":
        sys.stdout.write(text)
        return 0
    if mode == "write":
        os.makedirs(SURFACE_DIR, exist_ok=True)
        path = os.path.join(SURFACE_DIR, f"v{version}.txt")
        open(path, "w", encoding="utf8").write(text)
        print(f"check-surface: wrote {os.path.relpath(path, TOP)}")
        return 0

    problems = []
    # the releases before this one: the baseline is the newest, other than the one a tag is for
    known = [b for b in baselines() if tag is None or b[0] != tag[1:]]
    if not known:
        print("check-surface: there is no surface/v*.txt to compare with (make one with --write for a release)")
        return 1
    base_text, base_key, base_path = known[-1]
    ok, lines = verdict(parse(open(base_path, encoding="utf8").read()), current, base_key, version_key(version))
    if not ok:
        problems.append(f"since v{base_text} the version {version} does not fit what changed:")
        problems.extend(lines)
    if tag is not None:
        own = os.path.join(SURFACE_DIR, f"{tag}.txt")
        if not os.path.exists(own):
            problems.append(f"the tag {tag} has no surface/{tag}.txt (make it with --write before the release)")
        elif open(own, encoding="utf8").read() != text:
            problems.append(f"surface/{tag}.txt is not the surface of this tree: a release is made from the tree its baseline describes")
    if problems:
        for p in problems:
            print(f"check-surface: {p}")
        return 1
    print(f"check-surface: version {version} fits the surface against v{base_text}" + (f", and {tag} matches its baseline" if tag else ""))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

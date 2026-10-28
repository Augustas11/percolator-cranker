#!/usr/bin/env python3
"""Allowlist export of the public reference-keeper tree (private repo only).

    export_public.py <target-dir>

* <target-dir> must NOT exist. The tree is built in a fresh temporary
  directory next to it and published only after every check passes, by an
  atomic mkdir of <target-dir> (fails if anything exists there) and moving
  the entries in. A failed export leaves nothing behind and never touches
  existing files.
* HEAD is resolved once. Exactly the files in scripts/public_manifest.txt
  (as committed at that HEAD) are exported, read from that commit with
  `git cat-file`, never from the working tree.
* Every file committed at that HEAD under the exported directories must be
  listed in the manifest; a new file reaches the public tree only by being
  added on purpose.
* Destinations are unique (case-insensitively), normalized, and must stay
  inside the tree, outside the
  directories the gate skips (target/, upstream/), and free of dot
  components other than .github / .gitignore.
* Layout rewrites are explicit and anchored; then the content gate runs
  (scripts/check_public_tree.py) with the private denylist.

Env: PUBLIC_NAME (default "percolator-reference-keeper") replaces
@PUBLIC_NAME@. SECURITY_CONTACT replaces @SECURITY_CONTACT@; without it the
export is a DRAFT (the gate runs with --draft) and is not releasable.
"""

import os
import posixpath
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.realpath(__file__))
SRC = os.path.dirname(HERE)
SCANNED_DIRS = ["cranker/src", "cranker/tests", "client/src", "public"]
DOT_OK = {".github", ".gitignore"}
FORBIDDEN_NAMES = {
    "prep.md", "ROADMAP.md", "cranker-strategic-questions.md", "prompts", "digests",
    "signals.md", "radar.md", "contributors.md", "watchlist.md", "releases", "state",
    "repo", "repo-prog", ".preview", ".solana-test-ledger", "public_denylist.txt",
    "export_public.sh", "export_public.py", "public_manifest.txt", "scenario-findings.md",
}
REWRITES = [
    ("crates/cranker/Cargo.toml", 'path = "../repo-prog"', 'path = "../../upstream/percolator-prog"'),
    ("crates/bringup/Cargo.toml", 'path = "../repo-prog"', 'path = "../../upstream/percolator-prog"'),
]


def die(msg):
    print(f"export failed: {msg}", file=sys.stderr)
    sys.exit(1)


def git(*args, binary=False):
    out = subprocess.run(["git", "-C", SRC, *args], capture_output=True)
    if out.returncode != 0:
        die(f"git {' '.join(args)}: {out.stderr.decode(errors='replace').strip()}")
    return out.stdout if binary else out.stdout.decode()


def check_dest(to):
    norm = posixpath.normpath(to)
    if norm != to or to.startswith("/") or norm.startswith("../") or norm == "..":
        die(f"manifest destination not normalized or escapes the tree: {to!r}")
    comps = norm.split("/")
    if comps[0] in ("target", "upstream"):
        die(f"manifest destination inside a directory the gate skips: {to}")
    if any(c.startswith(".") and c not in DOT_OK for c in comps):
        die(f"manifest destination has a dot component: {to}")


def main(argv):
    if len(argv) != 1:
        print(__doc__.strip().splitlines()[2], file=sys.stderr)
        return 2
    dest = os.path.abspath(argv[0])
    if os.path.lexists(dest):
        die(f"{dest} already exists; export into a path that does not exist yet")
    parent = os.path.dirname(dest)
    os.makedirs(parent, exist_ok=True)

    head = git("rev-parse", "--verify", "HEAD^{commit}").strip()
    manifest = git("cat-file", "blob", f"{head}:scripts/public_manifest.txt")
    entries, listed, dests = [], set(), set()
    for raw in manifest.splitlines():
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        parts = line.split()
        if len(parts) != 2:
            die(f"bad manifest line: {raw!r}")
        src, to = parts
        check_dest(to)
        if to.lower() in dests:  # lower(): case-insensitive filesystems collide too
            die(f"duplicate manifest destination: {to}")
        dests.add(to.lower())
        entries.append((src, to))
        listed.add(src)
    committed = git("ls-tree", "-r", "-z", "--name-only", head, "--", *SCANNED_DIRS).split("\0")
    unlisted = [p for p in committed if p and p not in listed]
    if unlisted:
        die("committed files not in scripts/public_manifest.txt (add them, or keep them "
            "private by moving them): " + ", ".join(unlisted))

    tmp = tempfile.mkdtemp(prefix=".export-", dir=parent)
    ok = False
    try:
        for src, to in entries:
            mode_line = git("ls-tree", head, "--", src).strip()
            if not mode_line:
                die(f"manifest file not committed at HEAD: {src}")
            mode = mode_line.split()[0]
            if mode not in ("100644", "100755"):
                die(f"manifest file is not a regular file at HEAD ({mode}): {src}")
            data = git("cat-file", "blob", f"{head}:{src}", binary=True)
            out = os.path.join(tmp, to)
            os.makedirs(os.path.dirname(out), exist_ok=True)
            with open(out, "wb") as f:
                f.write(data)
            if mode == "100755" or to.endswith((".sh", ".py")):
                os.chmod(out, 0o755)

        for rel, a, b in REWRITES:
            p = os.path.join(tmp, rel)
            s = open(p, encoding="utf-8").read()
            if a not in s:
                die(f"rewrite anchor missing in {rel}: {a}")
            open(p, "w", encoding="utf-8").write(s.replace(a, b))

        subs = {"@PUBLIC_NAME@": os.environ.get("PUBLIC_NAME", "percolator-reference-keeper")}
        contact = os.environ.get("SECURITY_CONTACT", "")
        if contact:
            subs["@SECURITY_CONTACT@"] = contact
        for dirpath, _, files in os.walk(tmp):
            for fn in files:
                if not fn.endswith((".md", ".yml", ".toml", ".sh", ".html")):
                    continue
                p = os.path.join(dirpath, fn)
                s = open(p, encoding="utf-8").read()
                t = s
                for k, v in subs.items():
                    t = t.replace(k, v)
                if t != s:
                    open(p, "w", encoding="utf-8").write(t)

        for dirpath, dirs, files in os.walk(tmp):
            for name in dirs + files:
                if name in FORBIDDEN_NAMES:
                    die(f"export leaked forbidden path: {os.path.relpath(os.path.join(dirpath, name), tmp)}")

        n = sum(len(f) for _, _, f in os.walk(tmp))
        print(f"exported {n} files from {head[:12]}")
        gate = [sys.executable, "-I", os.path.join(HERE, "check_public_tree.py")]
        if contact:
            if "@" in contact:
                gate += ["--allow-email", contact]
        else:
            print("DRAFT: SECURITY_CONTACT not set, so @SECURITY_CONTACT@ remains; "
                  "this tree is not releasable yet.")
            gate.append("--draft")
        gate += [tmp, os.path.join(HERE, "public_denylist.txt")]
        if subprocess.run(gate).returncode != 0:
            die("content gate failed")
        # No-replace publication: mkdir is atomic and fails if anything now
        # exists at dest (a plain rename could replace an empty directory
        # created meanwhile). Then move the entries into the directory we own.
        try:
            os.mkdir(dest)
        except FileExistsError:
            die(f"{dest} appeared during the export; not replacing it")
        for name in os.listdir(tmp):
            os.rename(os.path.join(tmp, name), os.path.join(dest, name))
        os.rmdir(tmp)
        ok = True
        print(f"exported to {dest}")
        return 0
    finally:
        if not ok:
            shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

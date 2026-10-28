#!/usr/bin/env python3
"""Content gate for the public tree. FAIL-CLOSED: any scan problem fails.

    check_public_tree.py [--draft] [--allow-email ADDR] <dir> [denylist]

Fails (exit 1, every hit listed) on:
  * walk errors (unreadable directories), file names outside
    [A-Za-z0-9._/-], path components starting with '.' other than
    .github/.gitignore, symlinks, and anything that is not strict UTF-8
    text without NUL bytes (the WHOLE file is decoded, so late binary or
    UTF-16 content cannot hide);
  * secret-looking file names (.env*, id.json*, *keypair*.json,
    wallet*.json, *.pem, *.key), 64-element byte arrays (decimal or hex,
    optional u8 suffix and trailing comma, across whitespace and comments),
    and private-key / wallet-word-list markers, matched over the whole text;
  * absolute local paths (user home, root and volume directories, macOS
    private temp dirs, Windows user profiles), case-insensitive;
  * email addresses (except --allow-email ADDR, only in SECURITY.md);
  * URLs whose parsed host/path is not on the allowlist (dot segments,
    literal or percent-encoded, are rejected), and every GitHub
    owner/repo mention that is not an allowed repository (Cargo.lock
    included);
  * operator-stack paths (private by owner decision), case-insensitive;
  * unreplaced template placeholders (unless --draft);
  * denylist patterns (case-insensitive fixed strings) in contents and in
    file names. The denylist itself is never exported.

Exemption: a line ending in "// public-check: allow-test-fixture" inside
crates/*/tests/ may contain URLs/emails whose hosts are loopback or
documentation placeholders (*.example, *.example.com, *.invalid, *.evil).

Only <dir>/target, <dir>/upstream and <dir>/.git (top level) are skipped.
The gate scans its own files like any other: its source is written so that
none of its patterns match itself.
"""

import os
import re
import stat
import sys
from urllib.parse import urlsplit

SKIP_TOP = {"target", "upstream", ".git"}
DOT_OK = {".github", ".gitignore"}
NAME_OK = re.compile(r"[A-Za-z0-9._/-]+")  # used with fullmatch
FIXTURE = "// public-check: allow-test-fixture"
PLACEHOLDER_HOST = re.compile(r"(\.example|\.example\.com|\.invalid|\.evil)$")
LOOPBACK = {"127.0.0.1", "localhost", "::1", "[::1]"}

SECRET_NAME = re.compile(
    r"^(\.env.*|id\.json.*|.*keypair.*\.json|wallet.*\.json|.*\.pem|.*\.key)$", re.I)
_BYTE = r"(?:0x[0-9a-f_]+|[0-9][0-9_]*)(?:u8)?"
KEY_ARRAY = re.compile(r"\[(?:" + _BYTE + r",){63}" + _BYTE + r",?\]", re.I)
# Built from fragments so the gate's own source never matches its patterns.
KEY_MARKER = re.compile(
    "-" * 5 + "BEGIN [A-Z ]*PRIV" + "ATE KEY"
    + r"|(seed|recovery)[\s_-]*phr" + "ase"
    + r"|\bmnemo" + r"nic\b",
    re.I)
ABS_PATH = re.compile(
    "/" + "(us" + "ers|ho" + "me)/[a-z]"
    + "|/" + "ro" + "ot/"
    + "|/" + "vol" + "umes/"
    + "|/" + "priv" + "ate/(var|tmp)/"
    + r"|c:\\" + "us" + r"ers\\",
    re.I)
EMAIL = re.compile(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}")
URL = re.compile(r"\b[a-z][a-z0-9+.-]*://[^\s)\"'`<>]+", re.I)
GITHUB = re.compile(r"github\.com/([A-Za-z0-9_.-]+)(?:/([A-Za-z0-9_.-]+))?", re.I)
ALLOWED_REPOS = {
    ("aeyakovenko", "percolator"),
    ("aeyakovenko", "percolator-prog"),
    ("rust-lang", "crates.io-index"),
}
ALLOWED_HOSTS = {
    "release.anza.xyz", "docs.rs", "crates.io", "static.rust-lang.org", "sh.rustup.rs",
}
OPERATOR = re.compile(
    r"(^|/)(staging|monitor|scenario|deploy|trader|matcher|backing[_-]?manager|"
    r"load[_-]?test|devnet|hosting|operator[_-]?playbook|live[_-]?shaped)[a-z0-9_.-]*(/|$)"
    r"|(^|/)[a-z0-9_.-]*(operator[_-]?playbook|live[_-]?shaped|staging[_-]|devnet[_-]|"
    r"backing[_-]?manager)[a-z0-9_.-]*$"
    r"|(^|/)(alerts|dashboard)\.[a-z]+$",
    re.I)
PLACEHOLDER = re.compile("@" + "[A-Z][A-Z_]+" + "@")
DOT_SEGMENT = re.compile(r"(^|/)(\.|%2e){1,2}(/|$)", re.I)


def norm_repo(r):
    r = r.rstrip(".")
    return r[:-4] if r.lower().endswith(".git") else r


def url_ok(url):
    try:
        u = urlsplit(url)
    except ValueError:
        return False
    if u.scheme.lower() not in ("http", "https") or u.username or u.password:
        return False
    if DOT_SEGMENT.search(u.path):
        return False
    host = (u.hostname or "").lower()
    if host in LOOPBACK:
        return True
    if host in ALLOWED_HOSTS:
        return True
    if host == "github.com":
        parts = [p for p in u.path.split("/") if p]
        return len(parts) >= 2 and (parts[0].lower(), norm_repo(parts[1]).lower()) in ALLOWED_REPOS
    return False


def fixture_ok(rel, line):
    if not re.match(r"^crates/[A-Za-z0-9_-]+/tests/", rel) or not line.rstrip().endswith(FIXTURE):
        return False
    hosts = []
    for url in URL.findall(line):
        try:
            u = urlsplit(url)
        except ValueError:
            return False
        if u.scheme.lower() not in ("http", "https", "ftp"):
            return False
        hosts.append((u.hostname or "").lower())
        if u.username:  # userinfo is the fixture's subject; it must be a placeholder too
            hosts.append(u.username.lower())
    for m in EMAIL.finditer(line):
        hosts.append(m.group(0).split("@", 1)[1].lower())
    if GITHUB.search(line):
        return False  # GitHub mentions are never exempt
    # every host must be present and be loopback or a documentation placeholder
    return all(h and (h in LOOPBACK or PLACEHOLDER_HOST.search(h)) for h in hosts)


def main(argv):
    draft, allow_email, args = False, None, []
    it = iter(argv)
    for a in it:
        if a == "--draft":
            draft = True
        elif a == "--allow-email":
            allow_email = next(it, None)
        else:
            args.append(a)
    if not args:
        print(__doc__.strip().splitlines()[2], file=sys.stderr)
        return 2
    root = os.path.realpath(args[0])
    deny_path = args[1] if len(args) > 1 else None
    if not os.path.isdir(root):
        print(f"no such directory: {args[0]}", file=sys.stderr)
        return 2
    deny = []
    if deny_path:
        if not os.path.isfile(deny_path):
            print(f"denylist not found: {deny_path}", file=sys.stderr)
            return 2
        for raw in open(deny_path, encoding="utf-8"):
            p = raw.split("#", 1)[0].strip()
            if p:
                deny.append(p)

    hits = []
    hit = hits.append
    print(f"== check_public_tree: {root}")

    walk_errors = []
    files = []
    for dirpath, dirnames, filenames in os.walk(root, onerror=walk_errors.append, followlinks=False):
        reld = os.path.relpath(dirpath, root)
        reld = "" if reld == "." else reld
        if reld == "":
            dirnames[:] = [d for d in dirnames if d not in SKIP_TOP]
        for d in list(dirnames):
            if os.path.islink(os.path.join(dirpath, d)):
                hit(f"symlink: {os.path.join(reld, d)}")
                dirnames.remove(d)
        for fn in filenames:
            files.append(os.path.join(reld, fn) if reld else fn)
    for e in walk_errors:
        hit(f"SCAN ERROR (walk): {e}")

    texts = {}
    for rel in sorted(files):
        full = os.path.join(root, rel)
        if not NAME_OK.fullmatch(rel):
            hit(f"bad file name (allowed: A-Za-z0-9._/-): {rel!r}")
            continue
        comps = rel.split("/")
        bad_dot = [c for c in comps if c.startswith(".") and c not in DOT_OK]
        if bad_dot:
            hit(f"dot path component not allowed: {rel}")
            continue
        try:
            st = os.lstat(full)
        except OSError as e:
            hit(f"SCAN ERROR (stat): {rel}: {e}")
            continue
        if stat.S_ISLNK(st.st_mode):
            hit(f"symlink: {rel}")
            continue
        if not stat.S_ISREG(st.st_mode):
            hit(f"not a regular file: {rel}")
            continue
        try:
            data = open(full, "rb").read()
        except OSError as e:
            hit(f"SCAN ERROR (read): {rel}: {e}")
            continue
        if b"\0" in data:
            hit(f"non-text file (NUL bytes): {rel}")
            continue
        try:
            texts[rel] = data.decode("utf-8", errors="strict")
        except UnicodeDecodeError:
            hit(f"non-text file (not strict UTF-8): {rel}")

    for rel, text in texts.items():
        base = rel.rsplit("/", 1)[-1]
        lines = text.splitlines()
        if SECRET_NAME.match(base):
            hit(f"secret-looking file: {rel}")
        flat = re.sub(r"/\*.*?\*/", "", text, flags=re.S)
        flat = re.sub(r"//[^\n]*", "", flat)
        if KEY_ARRAY.search(re.sub(r"\s+", "", flat)):
            hit(f"keypair-like 64-byte array: {rel}")
        for m in KEY_MARKER.finditer(text):
            n = text.count("\n", 0, m.start()) + 1
            hit(f"private key marker: {rel}:{n}: {m.group(0)!r}")
        if OPERATOR.search(rel):
            hit(f"operator-stack path: {rel}")
        for pat in deny:
            if pat.lower() in rel.lower():
                hit(f"denylisted {pat!r} in file name: {rel}")
        is_lock = base == "Cargo.lock"
        for n, line in enumerate(lines, 1):
            loc = f"{rel}:{n}"
            if ABS_PATH.search(line):
                hit(f"absolute path: {loc}: {line.strip()}")
            fixture = fixture_ok(rel, line)
            for m in EMAIL.finditer(line):
                addr = m.group(0)
                if fixture or (allow_email and addr == allow_email and rel == "SECURITY.md"):
                    continue
                hit(f"email: {loc}: {addr}")
            if not is_lock and not fixture:
                for url in URL.findall(line):
                    url = url.rstrip(".,;:")
                    if not url_ok(url):
                        hit(f"url not allowlisted: {loc}: {url}")
            if not fixture:  # GitHub mentions are checked everywhere, Cargo.lock included
                for m in GITHUB.finditer(line):
                    tail = line[m.end():]
                    if DOT_SEGMENT.search(tail.split()[0] if tail.split() else ""):
                        hit(f"github path traversal: {loc}: {m.group(0)}{tail.split()[0]}")
                        continue
                    owner, repo = m.group(1).lower(), norm_repo(m.group(2) or "").lower()
                    if (owner, repo) not in ALLOWED_REPOS:
                        hit(f"github mention outside the allowed repos: {loc}: {m.group(0)}")
            if not draft:
                for m in PLACEHOLDER.finditer(line):
                    hit(f"unreplaced placeholder: {loc}: {m.group(0)}")
            low = line.lower()
            for pat in deny:
                if pat.lower() in low:
                    hit(f"denylisted {pat!r}: {loc}: {line.strip()}")

    if not deny_path:
        print("  (no denylist given: generic checks only)")
    for h in hits:
        print(f"  {h}")
    if hits:
        print("== FAIL")
        return 1
    print(f"== OK ({len(texts)} files checked)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

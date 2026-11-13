#!/usr/bin/env python3
"""Allowlist export of a public tree (private repo only).

    export_public.py [--profile drip|drip-markets] <target-dir>

Profiles (PROFILES below): "drip" (the default) is the reference keeper tree,
"drip-markets" the products tree. Each has its own manifest, checked
directories, forbidden names, rewrites and public name.

* <target-dir> must NOT exist. The tree is built in a fresh temporary
  directory next to it and published only after every check passes, by an
  atomic mkdir of <target-dir> (fails if anything exists there) and moving
  the entries in. A failed export, including one that fails while moving
  entries into the directory it just created, leaves nothing behind and
  never touches existing files.
* HEAD is resolved once. Exactly the files in the profile's manifest
  (as committed at that HEAD) are exported, read from that commit with
  `git cat-file`, never from the working tree.
* Every file committed at that HEAD under the profile's checked directories
  must be listed in the manifest (as a file, under a `dir:` entry, or as a
  deliberate `skip:`); a new file reaches the public tree only by being
  added on purpose.
* Destinations are unique (case-insensitively), normalized, and must stay
  inside the tree, outside the
  directories the gate skips (target/, upstream/), and free of dot
  components other than .github / .gitignore (drip-markets: also
  .dockerignore).
* Layout rewrites are explicit and anchored; then the leak helper
  (scripts/check_public_tree.sh, a plain pattern check) runs with the
  private denylist. The allowlist above is the control; the helper and the
  owner's read of the tree before the first push are the backstops.

Manifest lines (`#` starts a comment):
    <private path> <public path>     one committed file
    dir: <private dir> <public dir>  every committed file under the dir that
                                     no other line names; refused if any of
                                     them matches the exclude rules
                                     (excluded()) or a denylisted name
    skip: <private path>             a committed file under a checked dir
                                     that deliberately stays private
    allow-phrase: <words>            the leak helper ignores this phrase
                                     (letters, digits, space, - _) when it
                                     matches denylisted strings

Env: PUBLIC_NAME (default: the profile's name) replaces
@PUBLIC_NAME@. SECURITY_CONTACT replaces @SECURITY_CONTACT@; without it the
export is a DRAFT (the gate runs with --draft) and is not releasable.
"""

import fnmatch
import os
import posixpath
import re
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
    "ops", "research", "hedger", "matcher", "indexer",
}
REWRITES = [
    ("crates/cranker/Cargo.toml", 'path = "../repo-prog"', 'path = "../../upstream/percolator-prog"'),
    ("crates/bringup/Cargo.toml", 'path = "../repo-prog"', 'path = "../../upstream/percolator-prog"'),
]

# drip-markets: the products tree, private layout kept (cranker/, client/,
# flop-index/, ...), the pinned wrapper fetched into upstream/.
MARKETS_SCANNED = [
    "agents", "client", "cranker", "deploy", "docs/flop-compute", "faucet",
    "flop-index", "frontend", "indexer", "launch", "matcher", "mcp",
    "public-markets", "staging",
]
MARKETS_FORBIDDEN = {
    "prep.md", "ROADMAP.md", "cranker-strategic-questions.md", "prompts", "digests",
    "signals.md", "radar.md", "contributors.md", "watchlist.md", "releases", "state",
    "repo", "repo-prog", ".preview", ".solana-test-ledger", "public_denylist.txt",
    "export_public.sh", "export_public.py", "public_manifest.txt",
    "public_manifest.drip-markets.txt", "scenario-findings.md", "ops", "research",
    "hedger", "monitor", "engine-fuzz", "launch-runs", ".staging-runs", ".claude",
    ".omc", ".vercel", "PREMISE.md", "PRIOR_ART.md", "node_modules", "keys",
}
# Every (path, old, new) must match: a stale anchor fails the export. Path "*"
# applies to every exported UTF-8 file and must match at least one. An `old`
# of (start, end) replaces the text from start up to (not including) end, or
# to the end of the file when end is None; each anchor must occur exactly
# once. Applied in order. Comments and docs only, except the wrapper path, the
# hosting defaults and the private monitor stack cut out of deploy/.
MARKETS_REWRITES = [
    # The server address behind the site's API rewrites stays private.
    ("frontend/vercel.json", "http://159.223.165.194:", "http://@DEPLOY_HOST@:"),
    # deploy/: the monitor, Prometheus and Alertmanager (monitor/, private)
    # are cut; the bots, indexer, faucet, agents and feed stay.
    ("deploy/docker-compose.yml",
     ("# Day-0 hosting on one server:", "# Nothing here creates accounts"),
     "# Hosting on one server: the bots (pusher, matcher, backing, insurance,\n"
     "# keeper), the indexer, the faucet, the agents and the compute feed.\n"
     "# Reference / dry-run only; not a paid live service.\n"),
    ("deploy/docker-compose.yml", ' docs/LAUNCH.md, "Hosting".', ""),
    ("deploy/docker-compose.yml", ("  monitor:\n", "  # Read-only indexer"), ""),
    ("deploy/docker-compose.yml", ("\n  prometheus:\n", None), "\n"),
    ("deploy/Dockerfile", ("# Day-0 bots + monitor image", "# Build from the repo root"),
     "# Bots image (staging/). Reference / dry-run only; not a paid live service.\n"),
    ("deploy/Dockerfile", " \\\n && cargo build --release --manifest-path monitor/Cargo.toml", ""),
    ("deploy/Dockerfile", " /src/monitor/target/release/monitor", ""),
    ("deploy/launch.env.example", "# deploy/.env (docker compose) or /etc/percolator/launch.env (systemd).",
     "# deploy/.env (docker compose)."),
    ("deploy/launch.env.example", "TELEGRAM_BOT_TOKEN=\nTELEGRAM_CHAT_ID=\n", ""),
    ("frontend/README.md",
     ("For test USDC, ask the mint owner to run", "Connect and deposit"),
     "For test USDC, use the faucet on the site's trade page (`faucet/` is its server).\n"),
    ("frontend/README.md", "Copy `.env.example` to `.env.local`",
     "Copy `env.example` to `.env.local` (`env.production.example` is the live\nsite's build env: copy it to `.env.production`)"),
    # Private references in comments and docs.
    ("staging/src/main.rs", "not a paid live service;\n//! M3 NO-GO. See docs/STAGING.md.",
     "not a paid live service."),
    ("*", "; M3 NO-GO", ""),
    ("launch/sol-perp.toml", " (2-of-3, ops/KEYS.md)", " (2-of-3)"),
    ("launch/sol-perp.toml", " (public keys, ops/KEYS.md)", " (public keys)"),
    ("*", "# (REPORT-post-fix.md):", "# (the post-fix replay):"),
    ("launch/sol-perp.toml", "# REPORT.md: tickets", "# The replay study: tickets"),
    ("*", "ops/KEYS.md", "the key plan"),
    ("staging/src/main.rs", "// Effective signed quantity, copied from hedger/src/chain.rs::net_q.",
     "// Effective signed quantity."),
    ("staging/Cargo.toml", "# Private: the local staging market (docs/STAGING.md). Not exported.\n",
     "# The market bots: mark pusher, OpenRouter recorder, launch steps.\n"),
    ("staging/src/pusher/flop.rs", "from Pearl on", "from the bot server on"),
    ("staging/src/pusher/flop.rs", "on Pearl:", "on the bot server:"),
    ("launch/sol-perp.toml", "# staging binary (staging/) and scripts/staging_local.sh.",
     "# staging binary (staging/)."),
    ("*", "docs/OPERATOR_PLAYBOOK.md section 2", "the operator notes"),
    ("*", " (research/replay/REPORT.md)", " (a private replay study)"),
    ("*", " (research/replay/REPORT.md, \"Recommended start\")", " (a private replay study)"),
    ("launch/sol-perp.toml", "REPORT.md inventory limit (~line 260), ops/CAPITAL_PLAN.md:",
     "replay study inventory limit, capital plan:"),
    ("matcher/README.md", "funded by `scripts/staging_usdc_faucet.sh`", "funded with test USDC"),
    ("indexer/README.md", "# Private v16 indexer\n", "# Read-only v16 indexer\n"),
    ("indexer/README.md", "- [mizuki0x](https://github.com/mizuki0x): ", "- Upstream's other contributors: "),
    ("docs/flop-compute/SCORER.md", "after swarm round 1:", "after the first round of candidates:"),
    ("docs/flop-compute/SCORER.md", "(a disprover showed", "(a counter-example showed"),
    # The launch wrapper script stays private (it drives the private monitor);
    # the public steps are in docs/RUNBOOK.md.
    ("docs/flop-compute/INDEX.md", "(`scripts/launch_market.sh`):", "([RUNBOOK](../RUNBOOK.md)):"),
    ("staging/src/launch.rs", "Driven by `scripts/launch_market.sh`; see docs/LAUNCH.md.",
     "Driven by `staging step` and `staging bots`; see docs/RUNBOOK.md."),
    ("staging/src/launch.rs", "// `pins` and `monitor` are read by scripts/launch_market.sh.",
     "// `pins` and `monitor` are read by launch tooling outside this repository."),
    ("staging/src/launch.rs", "Then re-run the same launch_market.sh command:",
     "Then re-run the same `staging step handover` command:"),
    ("staging/src/launch.rs", "run scripts/launch_market.sh first", "run the `staging step` commands first (docs/RUNBOOK.md)"),
    ("staging/src/launch.rs", "(scripts/launch_market.sh)", "(docs/RUNBOOK.md)"),
    ("staging/src/main.rs", "(`launch.rs`, `scripts/launch_market.sh`)", "(`launch.rs`, docs/RUNBOOK.md)"),
    ("launch/sol-perp.toml", "the one file for scripts/launch_market.sh, the", "the one file for the launch steps, the"),
    ("*", "build.rs read. launch_market.sh\n# refuses", "build.rs read. The launch tooling\n# refuses"),
    ("docs/flop-compute/INDEX.md", ("## Running it\n", "## What this is not\n"),
     "## Running it\n\nThe `staging` steps and bots with `launch/flop-compute.toml`: "
     "[RUNBOOK](../RUNBOOK.md), \"Launch a market\".\n\n"),
    ("deploy/docker-compose.yml", "# LAUNCH_RUN_DIR is the launch's run dir (state.json + keys/, 0600), made by\n"
     "# scripts/launch_market.sh;", "# LAUNCH_RUN_DIR is the launch's run dir (state.json + keys/, 0600), made by\n"
     "# the launch steps (docs/RUNBOOK.md);"),
    ("frontend/src/site/config.ts", "// TODO-OWNER: ", "// "),
    # Hosting defaults: no server paths.
    ("*", "/opt/drip/", "/srv/drip/"),
    # Wrapper checkout: upstream/percolator-prog (scripts/fetch_upstream.sh).
    ("*", "repo-prog", "upstream/percolator-prog"),
]

PROFILES = {
    "drip": {
        "manifest": "scripts/public_manifest.txt",
        "scanned": SCANNED_DIRS,
        "forbidden": FORBIDDEN_NAMES,
        "rewrites": REWRITES,
        "subs": {},
        "keep": [],
        "dot_ok": DOT_OK,
    },
    "drip-markets": {
        "manifest": "scripts/public_manifest.drip-markets.txt",
        "scanned": MARKETS_SCANNED,
        "forbidden": MARKETS_FORBIDDEN,
        "rewrites": MARKETS_REWRITES,
        # The live site, one value for every skeleton file that names it.
        "subs": {"@SITE_URL@": "https://drip-devnet.vercel.app"},
        # Placeholders the public tree keeps on purpose (a deployer fills them).
        "keep": ["DEPLOY_HOST"],
        "dot_ok": DOT_OK | {".dockerignore"},
    },
}

# dir: entries refuse any expanded file that matches these (path components,
# base-name globs, size caps). Explicit file lines are deliberate and exempt.
EXCLUDE_COMPONENTS = {"node_modules", "dist", "target", ".vercel", "keys", ".git"}
EXCLUDE_GLOBS = [".env*", "*.sqlite*", "*.db", "*.pem", "*keypair*.json", "id.json", "*.so",
                 "*.key", "*.p8", "*.p12", "*.jks", "*.log", "*secret*", "credentials*"]
MAX_BIN = 64 * 1024      # *.bin fixtures
MAX_FILE = 1024 * 1024   # anything else
PHRASE_OK = re.compile(r"^[A-Za-z0-9 _-]+$")


def die(msg):
    print(f"export failed: {msg}", file=sys.stderr)
    sys.exit(1)


def git(*args, binary=False):
    out = subprocess.run(["git", "-C", SRC, *args], capture_output=True)
    if out.returncode != 0:
        die(f"git {' '.join(args)}: {out.stderr.decode(errors='replace').strip()}")
    return out.stdout if binary else out.stdout.decode()


def check_dest(to, dot_ok=DOT_OK):
    norm = posixpath.normpath(to)
    if norm != to or to.startswith("/") or norm.startswith("../") or norm == "..":
        die(f"manifest destination not normalized or escapes the tree: {to!r}")
    comps = norm.split("/")
    if comps[0] in ("target", "upstream"):
        die(f"manifest destination inside a directory the gate skips: {to}")
    if any(c.startswith(".") and c not in dot_ok for c in comps):
        die(f"manifest destination has a dot component: {to}")


def excluded(path, size):
    """Why a dir: expansion must not export this file, or None."""
    comps = path.split("/")
    hit = EXCLUDE_COMPONENTS.intersection(comps)
    if hit:
        return f"excluded directory {sorted(hit)[0]}/"
    base = comps[-1]
    for g in EXCLUDE_GLOBS:
        if fnmatch.fnmatchcase(base.lower(), g):
            return f"excluded name {g}"
    if base.endswith(".bin") and size > MAX_BIN:
        return f"*.bin over {MAX_BIN} bytes"
    if size > MAX_FILE:
        return f"over {MAX_FILE} bytes"
    return None


def denylist_patterns(head):
    text = git("cat-file", "blob", f"{head}:scripts/public_denylist.txt")
    out = []
    for raw in text.splitlines():
        p = raw.split("#", 1)[0].strip()
        if p:
            out.append(p.lower())
    return out


def parse_args(argv):
    profile = "drip"
    rest = []
    i = 0
    while i < len(argv):
        a = argv[i]
        if a == "--profile" and i + 1 < len(argv):
            profile = argv[i + 1]
            i += 2
            continue
        if a.startswith("--profile="):
            profile = a.split("=", 1)[1]
        else:
            rest.append(a)
        i += 1
    return profile, rest


def main(argv):
    profile_name, args = parse_args(argv)
    if len(args) != 1 or profile_name not in PROFILES:
        print(__doc__.strip().splitlines()[2], file=sys.stderr)
        print("profiles: " + ", ".join(PROFILES), file=sys.stderr)
        return 2
    prof = PROFILES[profile_name]
    forbidden = prof["forbidden"]
    dest = os.path.abspath(args[0])
    if os.path.lexists(dest):
        die(f"{dest} already exists; export into a path that does not exist yet")
    parent = os.path.dirname(dest)
    os.makedirs(parent, exist_ok=True)

    head = git("rev-parse", "--verify", "HEAD^{commit}").strip()
    manifest_path = prof["manifest"]
    manifest = git("cat-file", "blob", f"{head}:{manifest_path}")
    entries, listed, dests, dirs, skips, phrases = [], set(), set(), [], set(), []

    def add(src, to):
        if any(part in forbidden for part in posixpath.normpath(src).split("/")):
            die(f"manifest source is under a never-export path: {src}")  # e.g. ops/, research/
        check_dest(to, prof["dot_ok"])
        if to.lower() in dests:  # lower(): case-insensitive filesystems collide too
            die(f"duplicate manifest destination: {to}")
        dests.add(to.lower())
        entries.append((src, to))
        listed.add(src)

    for raw in manifest.splitlines():
        line = raw.split("#", 1)[0].strip()
        if not line:
            continue
        if line.startswith("allow-phrase:"):
            phrase = line[len("allow-phrase:"):].strip()
            if not PHRASE_OK.match(phrase):
                die(f"bad allow-phrase (letters, digits, space, - _ only): {raw!r}")
            phrases.append(phrase)
            continue
        parts = line.split()
        if parts[0] == "dir:":
            if len(parts) != 3:
                die(f"bad manifest line: {raw!r}")
            src_dir, to_dir = (posixpath.normpath(p) for p in parts[1:])
            if src_dir != parts[1].rstrip("/") or src_dir.startswith(("/", "..")) or src_dir == ".":
                die(f"bad dir: source: {raw!r}")
            dirs.append((src_dir, to_dir))
            continue
        if parts[0] == "skip:":
            if len(parts) != 2:
                die(f"bad manifest line: {raw!r}")
            skips.add(parts[1])
            continue
        if len(parts) != 2:
            die(f"bad manifest line: {raw!r}")
        add(*parts)

    for src in skips & listed:
        die(f"manifest both exports and skips: {src}")
    if dirs:
        deny = denylist_patterns(head)
        for src_dir, to_dir in dirs:
            tree = git("ls-tree", "-r", "-l", "-z", head, "--", src_dir + "/").split("\0")
            found = False
            for rec in tree:
                if not rec:
                    continue
                meta, path = rec.split("\t", 1)
                found = True
                if path in listed or path in skips:
                    continue
                size = int(meta.split()[3]) if meta.split()[3] != "-" else 0
                why = excluded(path, size)
                if why:
                    die(f"dir: {src_dir} would export {path} ({why}); skip: it or export it "
                        "by an explicit line")
                low = path.lower()
                for p in deny:
                    if p in low:
                        die(f"dir: {src_dir} would export a denylisted name: {path}")
                add(path, posixpath.join(to_dir, path[len(src_dir) + 1:]))
            if not found:
                die(f"dir: {src_dir} has no committed files")

    committed = git("ls-tree", "-r", "-z", "--name-only", head, "--", *prof["scanned"]).split("\0")
    committed = [p for p in committed if p]
    for s in sorted(skips):
        if s not in committed:
            die(f"skip: {s} is not a committed file under a checked directory")
    unlisted = [p for p in committed if p not in listed and p not in skips]
    if unlisted:
        die(f"committed files not in {manifest_path} (add them, or keep them "
            "private by moving them): " + ", ".join(unlisted))

    tmp = tempfile.mkdtemp(prefix=".export-", dir=parent)
    ok = created = False
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

        texts = None
        for rel, a, b in prof["rewrites"]:
            if rel == "*":
                if texts is None:
                    texts = []
                    for dirpath, _, files in os.walk(tmp):
                        for fn in files:
                            p = os.path.join(dirpath, fn)
                            try:
                                open(p, encoding="utf-8", newline="").read()
                            except (UnicodeDecodeError, ValueError):
                                continue
                            texts.append(p)
                hit = False
                for p in texts:
                    s = open(p, encoding="utf-8", newline="").read()
                    if a in s:
                        hit = True
                        open(p, "w", encoding="utf-8", newline="").write(s.replace(a, b))
                if not hit:
                    die(f"rewrite anchor missing in every file: {a}")
                continue
            p = os.path.join(tmp, rel)
            if not os.path.isfile(p):
                die(f"rewrite target not exported: {rel}")
            s = open(p, encoding="utf-8", newline="").read()  # keep line endings
            if isinstance(a, tuple):  # cut from start up to end (or EOF)
                start, end = a
                if s.count(start) != 1 or (end is not None and s.count(end) != 1):
                    die(f"rewrite anchors not found exactly once in {rel}: {a}")
                i = s.index(start)
                j = len(s) if end is None else s.index(end)
                if j < i:
                    die(f"rewrite anchors out of order in {rel}: {a}")
                open(p, "w", encoding="utf-8", newline="").write(s[:i] + b + s[j:])
                continue
            if a not in s:
                die(f"rewrite anchor missing in {rel}: {a}")
            open(p, "w", encoding="utf-8", newline="").write(s.replace(a, b))

        subs = {"@PUBLIC_NAME@": os.environ.get("PUBLIC_NAME", profile_name)}
        subs.update(prof["subs"])
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

        for dirpath, dirs_, files in os.walk(tmp):
            for name in dirs_ + files:
                if name in forbidden:
                    die(f"export leaked forbidden path: {os.path.relpath(os.path.join(dirpath, name), tmp)}")

        n = sum(len(f) for _, _, f in os.walk(tmp))
        print(f"exported {n} files from {head[:12]} (profile {profile_name})")
        gate = ["bash", os.path.join(HERE, "check_public_tree.sh")]
        if contact:
            if "@" in contact:
                gate += ["--allow-email", contact]
        else:
            print("DRAFT: SECURITY_CONTACT not set, so @SECURITY_CONTACT@ remains; "
                  "this tree is not releasable yet.")
            gate.append("--draft")
        for k in prof["keep"]:
            gate += ["--keep-placeholder", k]
        for ph in phrases:
            gate += ["--allow-phrase", ph]
        if profile_name != "drip":
            gate.append("--ipv4")
        gate += [tmp, os.path.join(HERE, "public_denylist.txt")]
        if subprocess.run(gate).returncode != 0:
            die("leak helper found hits (see above)")
        # No-replace publication: mkdir is atomic and fails if anything now
        # exists at dest (a plain rename could replace an empty directory
        # created meanwhile). Then move the entries into the directory we own.
        try:
            os.mkdir(dest)
        except FileExistsError:
            die(f"{dest} appeared during the export; not replacing it")
        created = True  # dest is ours from here on; a failure below removes it
        for name in os.listdir(tmp):
            os.rename(os.path.join(tmp, name), os.path.join(dest, name))
        os.rmdir(tmp)
        ok = True
        print(f"exported to {dest}")
        return 0
    finally:
        if not ok:
            shutil.rmtree(tmp, ignore_errors=True)
            if created:
                shutil.rmtree(dest, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

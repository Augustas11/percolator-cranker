#!/usr/bin/env python3
"""Tests for the public export (private; never exported). Run:

    python3 scripts/test_check_public_tree.py

* Exporter: the allowlist is the control, so it is tested properly. Only
  manifest files are copied, from the committed HEAD; unlisted committed
  files are refused; bad destinations, duplicates and existing targets are
  refused; a failed export leaves nothing behind.
* Leak helper (check_public_tree.sh): a plain pattern check. One hit per
  pattern, and a clean tree passes. Bypasses are out of scope by design.
* Profiles: --profile drip-markets reads its own manifest; glob: entries
  expand committed files and refuse excluded or denylisted ones; skip:,
  allow-phrase:, anchored and cut rewrites, placeholder substitution, kept
  placeholders, must-not-contain strings and the strict gate.
* CLI: argparse, --help, flag-like targets, --release; the gates and the
  denylist are read from HEAD, not the working copy.
* Strict gate (check_public_strict.py): one test per pattern.
* Binaries: pinned by sha256 in the manifest; fixtures are data files only.
"""

import base64
import hashlib
import json
import os
import runpy
import shutil
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.realpath(__file__))
HELPER = os.path.join(HERE, "check_public_tree.sh")
DENY = os.path.join(HERE, "public_denylist.txt")
OWNER = "aug" + "star"  # denylisted owner handle, split so this file stays greppable


def run_helper(root, *flags, deny=DENY):
    args = ["bash", HELPER, *flags, root] + ([deny] if deny else [])
    p = subprocess.run(args, capture_output=True, text=True)
    return p.returncode, p.stdout + p.stderr


class Helper(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.mkdtemp(prefix="helper-probe-")
        self.put("README.md", "clean keeper docs, see http://127.0.0.1:8899 and "
                              "https://github.com/aeyakovenko/percolator-prog\n")

    def tearDown(self):
        shutil.rmtree(self.root, ignore_errors=True)

    def put(self, rel, text):
        p = os.path.join(self.root, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "w", encoding="utf-8") as f:
            f.write(text)

    def fresh(self):
        self.tearDown()
        self.setUp()

    def assertHit(self, label, *flags):
        rc, out = run_helper(self.root, *flags)
        self.assertEqual(rc, 1, out)
        self.assertIn(label, out)

    def test_clean_tree_passes(self):
        rc, out = run_helper(self.root)
        self.assertEqual(rc, 0, out)

    def test_key_array(self):
        self.put("id.txt", "[" + ",".join(str(i) for i in range(64)) + "]\n")
        self.assertHit("64-number key array")

    def test_private_key_header(self):
        self.put("k.txt", "-----BEGIN " + "PRIVATE KEY-----\n")
        self.assertHit("private key header")

    def test_secret_file_names(self):
        for name in ("deploy-keypair.json", "id.json", ".env"):
            with self.subTest(name=name):
                self.fresh()
                self.put(name, "x\n")
                self.assertHit("secret-looking file")

    def test_home_paths(self):
        for path in ("/Us" + "ers/alice/x", "/ho" + "me/bob/x"):
            with self.subTest(path=path):
                self.fresh()
                self.put("a.md", path + "\n")
                self.assertHit("home path")

    def test_email_and_security_contact(self):
        self.put("a.md", "mail me: someone@mailhost.io\n")
        self.assertHit("email")
        self.fresh()
        self.put("t.rs", "fixture: user@remote.example, x@y.invalid\n")  # reserved domains
        rc, out = run_helper(self.root)
        self.assertEqual(rc, 0, out)
        self.fresh()
        self.put("SECURITY.md", "report to sec@keeper-project.io\n")
        rc, out = run_helper(self.root, "--allow-email", "sec@keeper-project.io")
        self.assertEqual(rc, 0, out)
        self.put("docs/x.md", "sec@keeper-project.io\n")  # only SECURITY.md may carry it
        self.assertHit("email", "--allow-email", "sec@keeper-project.io")

    def test_denylist_contents_file_names_and_private_repo(self):
        for rel, text in (("a.md", f"by {OWNER}\n"),
                          (f"{OWNER}-notes.md", "x\n"),
                          ("b.md", "see github.com/Augustas" + "11/percolator-cranker/pull/1\n")):
            with self.subTest(rel=rel):
                self.fresh()
                self.put(rel, text)
                self.assertHit("denylisted")

    def test_relative_denylist_path(self):
        self.put("RELEASE_NOTES.md", f"by {OWNER}\n")
        p = subprocess.run(["bash", HELPER, self.root, "scripts/public_denylist.txt"],
                           capture_output=True, text=True, cwd=os.path.dirname(HERE))
        self.assertEqual(p.returncode, 1, p.stdout + p.stderr)
        self.assertIn("denylisted", p.stdout)

    def test_placeholder_unless_draft(self):
        self.put("SECURITY.md", "contact @SECURITY" + "_CONTACT@\n")
        self.assertHit("unreplaced placeholder")
        rc, out = run_helper(self.root, "--draft")
        self.assertEqual(rc, 0, out)

    def test_binaries_skipped_and_bad_usage(self):
        with open(os.path.join(self.root, "blob.bin"), "wb") as f:
            f.write(b"\0\1\2" + OWNER.encode())
        rc, out = run_helper(self.root)
        self.assertEqual(rc, 0, out)
        rc, _ = run_helper(os.path.join(self.root, "missing"))
        self.assertEqual(rc, 2)

    def test_ipv4_only_with_flag(self):
        self.put("vercel.json", '{"destination": "http://203.' + '45.67.89:8091/"}\n')
        rc, out = run_helper(self.root)
        self.assertEqual(rc, 0, out)
        self.assertHit("ipv4 literal", "--ipv4")
        self.fresh()
        self.put("ok.rs", "127.0.0.1:8090 10.1.2.3 192.168.1.1 172.20.0.1 0.0.0.0 "
                          "192.0.2.7 before=01.111.0.0 v1.2.3 999.1.1.1\n")
        rc, out = run_helper(self.root, "--ipv4")
        self.assertEqual(rc, 0, out)
        # Each address is judged alone: a private one next to it hides nothing.
        self.put("hosts.txt", "10.0.0.1,198." + "18.4.4 and 127.0.0.1 end 198." + "18.9.9.\n")
        rc, out = run_helper(self.root, "--ipv4")
        self.assertEqual(rc, 1, out)
        self.assertIn("198." + "18.4.4 in", out)
        self.assertIn("198." + "18.9.9 in", out)

    def test_keep_placeholder(self):
        self.put("vercel.json", "http://@DEPLOY" + "_HOST@:8091\n")
        self.assertHit("unreplaced placeholder")
        rc, out = run_helper(self.root, "--keep-placeholder", "DEPLOY_HOST")
        self.assertEqual(rc, 0, out)
        self.put("SECURITY.md", "contact @SECURITY" + "_CONTACT@\n")  # others still fail
        self.assertHit("unreplaced placeholder", "--keep-placeholder", "DEPLOY_HOST")
        rc, _ = run_helper(self.root, "--keep-placeholder", "bad name")
        self.assertEqual(rc, 2)

    def test_allow_phrase(self):
        deny = os.path.join(self.root, "..", os.path.basename(self.root) + "-deny.txt")
        with open(deny, "w") as f:
            f.write("Claude\n")
        try:
            self.put("Home.tsx", 'one line in Claude Code: "claude mcp add drip"\n')
            rc, out = run_helper(self.root, deny=deny)
            self.assertEqual(rc, 1, out)
            rc, out = run_helper(self.root, "--allow-phrase", "Claude Code",
                                 "--allow-phrase", "claude mcp add", deny=deny)
            self.assertEqual(rc, 0, out)
            self.put("notes.md", "asked Claude about it\n")  # the bare word still hits
            rc, out = run_helper(self.root, "--allow-phrase", "Claude Code",
                                 "--allow-phrase", "claude mcp add", deny=deny)
            self.assertEqual(rc, 1, out)
            self.assertIn("notes.md", out)
            os.remove(os.path.join(self.root, "notes.md"))
            with open(deny, "a") as f:
                f.write("Codex\n")
            self.put("x.md", "built with Claude Codex\n")  # the phrase must be a whole word
            rc, out = run_helper(self.root, "--allow-phrase", "Claude Code", deny=deny)
            self.assertEqual(rc, 1, out)
            self.assertIn("x.md", out)
            rc, _ = run_helper(self.root, "--allow-phrase", "a.b", deny=deny)
            self.assertEqual(rc, 2)
        finally:
            os.remove(deny)

    def test_helper_passes_on_its_own_source(self):
        shutil.copy(HELPER, os.path.join(self.root, "check_public_tree.sh"))
        rc, out = run_helper(self.root)
        self.assertEqual(rc, 0, out)


class Exporter(unittest.TestCase):
    """The exporter, run on a throwaway git repo holding a copy of the scripts."""

    def setUp(self):
        self.repo = tempfile.mkdtemp(prefix="export-probe-")
        os.makedirs(os.path.join(self.repo, "scripts"))
        for f in ("export_public.py", "check_public_tree.sh", "check_public_strict.py"):
            shutil.copy(os.path.join(HERE, f), os.path.join(self.repo, "scripts", f))
        open(os.path.join(self.repo, "scripts", "public_denylist.txt"), "w").close()
        self.write("a.md", "clean\n")
        self.write("b.md", "clean\n")
        self.write("dep.toml", '[dependencies]\nx = { path = "../repo-prog" }\n')
        self.dest = os.path.join(self.repo, "out", "tree")

    def tearDown(self):
        shutil.rmtree(self.repo, ignore_errors=True)

    def write(self, rel, text):
        p = os.path.join(self.repo, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "w") as fh:
            fh.write(text)

    def commit(self, manifest):
        manifest += "dep.toml crates/cranker/Cargo.toml\ndep.toml crates/bringup/Cargo.toml\n"
        self.write("scripts/public_manifest.txt", manifest)
        g = ["git", "-C", self.repo, "-c", "user.name=t", "-c", "user.email=t@t.invalid"]
        subprocess.run(g + ["init", "-q"], check=True)
        subprocess.run(g + ["add", "-A"], check=True)
        subprocess.run(g + ["commit", "-qm", "t", "--allow-empty"], check=True)

    def export(self):
        p = subprocess.run([sys.executable, "-I", os.path.join(self.repo, "scripts", "export_public.py"),
                            self.dest], capture_output=True, text=True)
        return p.returncode, p.stdout + p.stderr

    def leftovers(self):
        out = os.path.dirname(self.dest)
        return sorted(os.listdir(out)) if os.path.isdir(out) else []

    def exported(self):
        return sorted(os.path.relpath(os.path.join(d, f), self.dest)
                      for d, _, fs in os.walk(self.dest) for f in fs)

    def test_only_manifest_files_are_copied(self):
        self.write("private.md", "not listed\n")  # committed, outside the checked dirs
        self.commit("a.md README.md\n")
        rc, out = self.export()
        self.assertEqual(rc, 0, out)
        self.assertEqual(self.exported(),
                         ["README.md", "crates/bringup/Cargo.toml", "crates/cranker/Cargo.toml"])
        self.assertEqual(self.leftovers(), ["tree"])

    def test_reads_committed_head_not_working_tree(self):
        self.commit("a.md README.md\n")
        self.write("a.md", "uncommitted edit\n")
        rc, out = self.export()
        self.assertEqual(rc, 0, out)
        with open(os.path.join(self.dest, "README.md")) as fh:
            self.assertEqual(fh.read(), "clean\n")

    def test_unlisted_committed_file_in_checked_dir_refused(self):
        self.write("cranker/src/lib.rs", "// listed\n")
        self.write("cranker/src/extra.rs", "// not listed\n")
        self.commit("a.md README.md\ncranker/src/lib.rs crates/cranker/src/lib.rs\n")
        rc, out = self.export()
        self.assertEqual(rc, 1, out)
        self.assertIn("cranker/src/extra.rs", out)
        self.assertEqual(self.leftovers(), [])

    def test_manifest_file_not_committed_refused(self):
        self.commit("a.md README.md\nnope.md docs/NOPE.md\n")
        rc, out = self.export()
        self.assertEqual(rc, 1, out)
        self.assertIn("not committed", out)
        self.assertEqual(self.leftovers(), [])

    def test_bad_destinations_refused(self):
        for to in ("../escape.md", "/abs.md", "docs/../../x.md", "target/x.md",
                   "upstream/x.md", ".secret/x.md", "docs/./x.md"):
            with self.subTest(to=to):
                self.commit(f"a.md {to}\n")
                rc, out = self.export()
                self.assertEqual(rc, 1, out)
                self.assertEqual(self.leftovers(), [])

    def test_forbidden_name_refused(self):
        self.commit("a.md docs/prep.md\n")
        rc, out = self.export()
        self.assertEqual(rc, 1, out)
        self.assertIn("forbidden", out)
        self.assertEqual(self.leftovers(), [])

    def test_duplicate_destinations(self):
        for manifest in ("a.md README.md\nb.md README.md\n", "a.md README.md\nb.md readme.md\n"):
            with self.subTest(manifest=manifest):
                self.commit(manifest)
                rc, out = self.export()
                self.assertEqual(rc, 1, out)
                self.assertIn("duplicate manifest destination", out)
                self.assertEqual(self.leftovers(), [])

    def test_existing_destination_untouched(self):
        self.commit("a.md README.md\n")
        os.makedirs(self.dest)
        rc, out = self.export()
        self.assertEqual(rc, 1, out)
        self.assertEqual(os.listdir(self.dest), [])

    def test_helper_hit_leaves_nothing(self):
        self.write("a.md", f"by {OWNER}\n")
        self.write("scripts/public_denylist.txt", OWNER + "\n")
        self.commit("a.md README.md\n")
        rc, out = self.export()
        self.assertEqual(rc, 1, out)
        self.assertEqual(self.leftovers(), [])

    def test_failure_while_publishing_removes_destination(self):
        self.commit("a.md README.md\nb.md docs/B.md\n")
        # Fail the second rename into dest, in-process.
        driver = (
            "import os, sys, runpy\n"
            "real, n = os.rename, [0]\n"
            "def flaky(a, b):\n"
            "    n[0] += 1\n"
            "    if n[0] == 2: raise OSError('injected')\n"
            "    real(a, b)\n"
            "os.rename = flaky\n"
            "sys.argv = [sys.argv[1], sys.argv[2]]\n"
            "runpy.run_path(sys.argv[0], run_name='__main__')\n")
        p = subprocess.run([sys.executable, "-I", "-c", driver,
                            os.path.join(self.repo, "scripts", "export_public.py"), self.dest],
                           capture_output=True, text=True)
        self.assertNotEqual(p.returncode, 0, p.stdout + p.stderr)
        self.assertIn("injected", p.stderr)
        self.assertEqual(self.leftovers(), [])


# Runs the exporter with one profile's settings patched (JSON on argv), so a
# throwaway repo needs none of the real profile's rewrite anchors.
DRIVER = (
    "import json, sys, runpy\n"
    "script, dest, prof, patch = sys.argv[1:5]\n"
    "g = runpy.run_path(script, run_name='export_public')\n"
    "p = g['PROFILES'][prof]\n"
    "for k, v in json.loads(patch).items():\n"
    "    if k == 'rewrites':\n"
    "        v = [(r, tuple(a) if isinstance(a, list) else a, b) for r, a, b in v]\n"
    "    p[k] = v\n"
    "sys.exit(g['main'](['--profile', prof, dest]))\n")


class MarketsProfile(unittest.TestCase):
    """drip-markets: own manifest, glob:/skip:/allow-phrase: lines, rewrites, subs."""

    # The throwaway-repo helpers, without re-running Exporter's tests.
    setUp, tearDown, write, commit = Exporter.setUp, Exporter.tearDown, Exporter.write, Exporter.commit
    leftovers, exported, export = Exporter.leftovers, Exporter.exported, Exporter.export

    def commit_markets(self, manifest, deny=""):
        self.write("scripts/public_manifest.drip-markets.txt", manifest)
        self.write("scripts/public_denylist.txt", deny)
        self.commit("")

    def export_markets(self, env=None, **patch):
        patch.setdefault("rewrites", [])
        patch.setdefault("subs", {})
        patch.setdefault("keep", [])
        p = subprocess.run([sys.executable, "-I", "-c", DRIVER,
                            os.path.join(self.repo, "scripts", "export_public.py"), self.dest,
                            "drip-markets", json.dumps(patch)],
                           capture_output=True, text=True, env={**os.environ, **(env or {})})
        return p.returncode, p.stdout + p.stderr

    def read(self, rel):
        with open(os.path.join(self.dest, rel)) as fh:
            return fh.read()

    def test_profile_selects_its_manifest(self):
        self.write("c.md", "markets\n")
        self.commit_markets("c.md README.md\n")
        rc, out = self.export_markets()
        self.assertEqual(rc, 0, out)
        self.assertEqual(self.exported(), ["README.md"])
        self.assertEqual(self.read("README.md"), "markets\n")

    def test_unknown_profile_is_usage_error(self):
        self.commit("a.md README.md\n")
        p = subprocess.run([sys.executable, "-I", os.path.join(self.repo, "scripts", "export_public.py"),
                            "--profile", "nope", self.dest], capture_output=True, text=True)
        self.assertEqual(p.returncode, 2, p.stdout + p.stderr)
        self.assertEqual(self.leftovers(), [])

    def test_glob_entry_expands_committed_files_only(self):
        self.write("frontend/index.html", "<p>site</p>\n")
        self.write("frontend/src/app.ts", "export {}\n")
        self.write("frontend/src/a/b.tsx", "export {}\n")
        self.write("frontend/src/c/d.css", "p {}\n")
        self.write("frontend/src/c/e.svg", "<svg/>\n")
        self.commit_markets("frontend/index.html frontend/index.html\n"
                            "glob: frontend/src/*.ts\nglob: frontend/src/{a,c}/*.{tsx,css}\n"
                            "skip: frontend/src/c/e.svg\n")
        self.write("frontend/src/uncommitted.ts", "x\n")
        rc, out = self.export_markets()
        self.assertEqual(rc, 0, out)
        self.assertEqual(self.exported(), ["frontend/index.html", "frontend/src/a/b.tsx",
                                           "frontend/src/app.ts", "frontend/src/c/d.css"])
        # ** spans directories; ? is one character within a segment.
        shutil.rmtree(self.dest)
        os.remove(os.path.join(self.repo, "frontend/src/uncommitted.ts"))
        self.commit_markets("frontend/index.html frontend/index.html\nglob: frontend/**/?.{tsx,css}\n"
                            "glob: frontend/src/app.ts\nskip: frontend/src/c/e.svg\n")
        rc, out = self.export_markets()
        self.assertEqual(rc, 0, out)
        self.assertEqual(len(self.exported()), 4)

    def test_new_file_type_or_top_level_file_fails(self):
        self.write("frontend/src/app.ts", "x\n")
        for extra in ("frontend/src/new.wasm", "frontend/NEW.md", "frontend/src/newdir/x.ts"):
            with self.subTest(extra=extra):
                self.tearDown()
                self.setUp()
                self.write("frontend/src/app.ts", "x\n")
                self.write(extra, "x\n")
                self.commit_markets("glob: frontend/src/*.{ts,tsx}\n")
                rc, out = self.export_markets()
                self.assertEqual(rc, 1, out)
                self.assertIn(extra, out)
                self.assertEqual(self.leftovers(), [])

    def test_glob_matching_nothing_and_dir_entries_refused(self):
        self.write("frontend/src/app.ts", "x\n")
        for manifest, why in (("glob: frontend/src/app.ts\nglob: frontend/src/*.vue\n", "matches no"),
                              ("dir: frontend frontend\n", "dir: entries are gone"),
                              ("glob: ../x/*.ts\n", "bad manifest line")):
            with self.subTest(manifest=manifest):
                self.commit_markets(manifest)
                rc, out = self.export_markets()
                self.assertEqual(rc, 1, out)
                self.assertIn(why, out)
                self.assertEqual(self.leftovers(), [])

    def test_checked_dir_must_be_fully_enumerated(self):
        self.write("agents/a.ts", "x\n")
        self.write("agents/b.ts", "x\n")
        self.commit_markets("agents/a.ts agents/a.ts\n")
        rc, out = self.export_markets()
        self.assertEqual(rc, 1, out)
        self.assertIn("agents/b.ts", out)
        self.assertEqual(self.leftovers(), [])
        self.commit_markets("agents/a.ts agents/a.ts\nskip: agents/b.ts\n")
        rc, out = self.export_markets()
        self.assertEqual(rc, 0, out)
        self.assertEqual(self.exported(), ["agents/a.ts"])

    def test_glob_entry_refuses_excluded_files_unless_skipped(self):
        cases = {
            "frontend/.env.production": "excluded name",
            "frontend/node_modules/x.js": "excluded directory",
            "frontend/dist/x.js": "excluded directory",
            "frontend/.vercel/project.json": "excluded directory",
            "frontend/keys/lp.json": "excluded directory",
            "frontend/idx.sqlite-wal": "excluded name",
            "frontend/deploy-keypair.json": "excluded name",
            "frontend/big.bin": "*.bin over",
            "frontend/state.db": "excluded name",
            "frontend/client_secret.json": "excluded name",
            "frontend/credentials.txt": "excluded name",
        }
        for path, why in cases.items():
            with self.subTest(path=path):
                self.tearDown()
                self.setUp()
                self.write("frontend/ok.ts", "x\n")
                self.write(path, "0" * (70 * 1024) if path.endswith(".bin") else "x\n")
                self.commit_markets("glob: frontend/**/*\n")
                rc, out = self.export_markets()
                self.assertEqual(rc, 1, out)
                self.assertIn(why, out)
                self.assertEqual(self.leftovers(), [])
                self.commit_markets(f"glob: frontend/**/*\nskip: {path}\n")
                rc, out = self.export_markets()
                self.assertEqual(rc, 0, out)
                self.assertEqual(self.exported(), ["frontend/ok.ts"])

    def test_dockerignore_destination_only_in_markets(self):
        self.write("di", "target/\n")
        self.commit_markets("di .dockerignore\n")
        self.write("scripts/public_manifest.txt", "di .dockerignore\ndep.toml crates/cranker/Cargo.toml\n"
                   "dep.toml crates/bringup/Cargo.toml\n")
        g = ["git", "-C", self.repo, "-c", "user.name=t", "-c", "user.email=t@t.invalid"]
        subprocess.run(g + ["commit", "-qam", "drip manifest"], check=True)
        rc, out = self.export()  # drip: refused as before
        self.assertEqual(rc, 1, out)
        self.assertIn("dot component", out)
        rc, out = self.export_markets()
        self.assertEqual(rc, 0, out)
        self.assertEqual(self.exported(), [".dockerignore"])

    def test_rewrites_keep_crlf(self):
        with open(os.path.join(self.repo, "w.md"), "wb") as fh:
            fh.write(b"one\r\ntwo repo-x\r\n")
        self.commit_markets("w.md W.md\n")
        rc, out = self.export_markets(rewrites=[["W.md", "repo-x", "upstream/x"]])
        self.assertEqual(rc, 0, out)
        with open(os.path.join(self.dest, "W.md"), "rb") as fh:
            self.assertEqual(fh.read(), b"one\r\ntwo upstream/x\r\n")

    def test_glob_entry_refuses_denylisted_name(self):
        self.write(f"frontend/{OWNER}-notes.md", "x\n")
        self.commit_markets("glob: frontend/*.md\n", deny=OWNER + "\n")
        rc, out = self.export_markets()
        self.assertEqual(rc, 1, out)
        self.assertIn("denylisted name", out)
        self.assertEqual(self.leftovers(), [])

    def test_skip_must_name_a_committed_file_and_not_an_exported_one(self):
        self.commit_markets("a.md README.md\nskip: frontend/gone.ts\n")
        rc, out = self.export_markets()
        self.assertEqual(rc, 1, out)
        self.assertIn("not a committed file", out)
        self.write("frontend/x.ts", "x\n")
        self.commit_markets("frontend/x.ts frontend/x.ts\nskip: frontend/x.ts\n")
        rc, out = self.export_markets()
        self.assertEqual(rc, 1, out)
        self.assertIn("both exports and skips", out)

    def test_rewrites_global_cut_and_stale_anchor(self):
        self.write("a.md", "keep\n# BEGIN private\nsecret block\n# END\nrepo-x here\n")
        self.write("b.md", "repo-x there\n")
        self.commit_markets("a.md docs/A.md\nb.md docs/B.md\n")
        rw = [["docs/A.md", ["# BEGIN private", "# END"], ""], ["*", "repo-x", "upstream/x"]]
        rc, out = self.export_markets(rewrites=rw)
        self.assertEqual(rc, 0, out)
        self.assertEqual(self.read("docs/A.md"), "keep\n# END\nupstream/x here\n")
        self.assertEqual(self.read("docs/B.md"), "upstream/x there\n")
        for bad in ([["*", "not anywhere", "y"]], [["docs/A.md", "absent", "y"]],
                    [["docs/A.md", ["# END", "# BEGIN private"], ""]],
                    [["docs/A.md", ["missing", None], ""]], [["docs/NONE.md", "x", "y"]]):
            with self.subTest(bad=bad):
                shutil.rmtree(self.dest, ignore_errors=True)
                rc, out = self.export_markets(rewrites=bad)
                self.assertEqual(rc, 1, out)
                self.assertIn("rewrite", out)
                self.assertEqual(self.leftovers(), [])

    def test_placeholder_substitution_and_kept_placeholder(self):
        self.write("r.md", "# @PUBLIC_" + "NAME@\nsite @SITE" + "_URL@\n")
        self.write("v.json", '{"d": "http://@DEPLOY' + '_HOST@:8091"}\n')
        self.commit_markets("r.md README.md\nv.json frontend/vercel.json\n")
        subs = {"@SITE" + "_URL@": "https://site.example"}
        env = {"SECURITY_CONTACT": "https://sec.example/report"}  # not a draft: placeholders gated
        rc, out = self.export_markets(env=env, subs=subs)
        self.assertEqual(rc, 1, out)  # @DEPLOY_HOST@ is not kept
        self.assertIn("unreplaced placeholder", out)
        rc, out = self.export_markets(env=env, subs=subs, keep=["DEPLOY_HOST"])
        self.assertEqual(rc, 0, out)
        self.assertEqual(self.read("README.md"), "# drip-markets\nsite https://site.example\n")
        self.assertIn("@DEPLOY" + "_HOST@", self.read("frontend/vercel.json"))

    def test_denylist_hit_fails_and_allow_phrase(self):
        self.write("h.tsx", "one line in Claude Code\n")
        self.commit_markets("h.tsx frontend/h.tsx\n", deny="claude\n")
        rc, out = self.export_markets()
        self.assertEqual(rc, 1, out)
        self.assertIn("denylisted", out)
        self.assertEqual(self.leftovers(), [])
        self.commit_markets("allow-phrase: Claude Code\nh.tsx frontend/h.tsx\n", deny="claude\n")
        rc, out = self.export_markets()
        self.assertEqual(rc, 0, out)
        shutil.rmtree(self.dest)
        self.commit_markets("allow-phrase: bad.phrase\nh.tsx frontend/h.tsx\n", deny="claude\n")
        rc, out = self.export_markets()
        self.assertEqual(rc, 1, out)
        self.assertIn("bad allow-phrase", out)

    def test_markets_export_checks_ipv4(self):
        self.write("v.json", '{"d": "http://198.' + '18.7.9:8091"}\n')
        self.commit_markets("v.json frontend/vercel.json\n")
        rc, out = self.export_markets()
        self.assertEqual(rc, 1, out)
        self.assertIn("ipv4 literal", out)
        self.assertEqual(self.leftovers(), [])


    def test_must_not_contain_private_docs(self):
        self.write("x.rs", "// see docs/LAUNCH" + ".md\n")
        self.commit_markets("x.rs staging/x.rs\n")
        rc, out = self.export_markets()
        self.assertEqual(rc, 1, out)
        self.assertIn("refers to private", out)
        self.assertEqual(self.leftovers(), [])

    def test_markets_export_runs_strict_gate(self):
        self.write("x.ts", 'const u = "https://rpc.unlisted-host.io/v1";\n')
        self.commit_markets("x.ts mcp/x.ts\n")
        rc, out = self.export_markets()
        self.assertEqual(rc, 1, out)
        self.assertIn("host not on the allow list", out)
        self.assertEqual(self.leftovers(), [])


class Cli(unittest.TestCase):
    """argparse, --release, and gates read from HEAD."""

    setUp, tearDown, write, commit = Exporter.setUp, Exporter.tearDown, Exporter.write, Exporter.commit
    leftovers, exported = Exporter.leftovers, Exporter.exported

    def run_export(self, *args, env=None):
        p = subprocess.run([sys.executable, "-I", os.path.join(self.repo, "scripts", "export_public.py"),
                            *args], capture_output=True, text=True,
                           env={**os.environ, **(env or {})}, cwd=self.repo)
        return p.returncode, p.stdout + p.stderr

    def test_help(self):
        rc, out = self.run_export("--help")
        self.assertEqual(rc, 0, out)
        self.assertIn("--profile", out)
        self.assertIn("--release", out)

    def test_flag_like_target_and_extra_args_refused(self):
        self.commit("a.md README.md\n")
        for args in (["--", "-out"], ["--bogus", "x"], ["a", "b"], []):
            with self.subTest(args=args):
                rc, out = self.run_export(*args)
                self.assertEqual(rc, 2, out)
                self.assertFalse(os.path.exists(os.path.join(self.repo, "-out")))

    def test_draft_says_not_releasable_and_release_refuses(self):
        self.commit("a.md README.md\n")
        env = {"SECURITY_CONTACT": ""}
        rc, out = self.run_export(self.dest, env=env)
        self.assertEqual(rc, 0, out)
        self.assertIn("NOT RELEASABLE", out)
        shutil.rmtree(self.dest)
        rc, out = self.run_export("--release", self.dest, env=env)
        self.assertEqual(rc, 1, out)
        self.assertIn("NOT RELEASABLE", out)
        self.assertEqual(self.leftovers(), [])
        rc, out = self.run_export("--release", self.dest, env={"SECURITY_CONTACT": "https://sec.example/r"})
        self.assertEqual(rc, 0, out)
        self.assertNotIn("NOT RELEASABLE", out)

    def test_gate_and_denylist_come_from_head(self):
        self.write("a.md", f"by {OWNER}\n")
        self.write("scripts/public_denylist.txt", OWNER + "\n")
        self.commit("a.md README.md\n")
        # Uncommitted edits that would let the leak through change nothing.
        self.write("scripts/public_denylist.txt", "")
        self.write("scripts/check_public_tree.sh", "exit 0\n")
        rc, out = self.run_export(self.dest)
        self.assertEqual(rc, 1, out)
        self.assertIn("leak helper found hits", out)
        self.assertEqual(self.leftovers(), [])
        # And an uncommitted denylist entry does not fail a clean committed tree.
        self.write("a.md", "clean\n")
        self.write("scripts/public_denylist.txt", "")
        subprocess.run(["git", "-C", self.repo, "checkout", "--", "scripts/check_public_tree.sh"], check=True)
        self.commit("a.md README.md\n")
        self.write("scripts/public_denylist.txt", "clean\n")
        rc, out = self.run_export(self.dest)
        self.assertEqual(rc, 0, out)


STRICT = os.path.join(HERE, "check_public_strict.py")


class StrictGate(unittest.TestCase):
    """check_public_strict.py: one hit per pattern; a clean tree passes."""

    OPTS = ["--allow-domain", "github.com", "--fixture", "fx/*", "--allow-binary", "ok.png",
            "--binary-cap", "100"]

    def setUp(self):
        self.root = tempfile.mkdtemp(prefix="strict-probe-")
        self.deny = self.root + "-deny.txt"
        with open(self.deny, "w") as fh:
            fh.write(OWNER + "\n198." + "18.7.9\n")
        self.put("README.md", "see https://github.com/x/y and http://127.0.0.1:8899, "
                              "http://localhost:3000 and https://api.example.com/?api_key=1\n")

    def tearDown(self):
        shutil.rmtree(self.root, ignore_errors=True)
        os.remove(self.deny)

    def put(self, rel, data):
        p = os.path.join(self.root, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "wb") as fh:
            fh.write(data.encode() if isinstance(data, str) else data)

    def gate(self, *extra):
        p = subprocess.run([sys.executable, "-I", STRICT, self.root, self.deny, *self.OPTS, *extra],
                           capture_output=True, text=True)
        return p.returncode, p.stdout + p.stderr

    def assertHit(self, rel, data, label):
        self.put(rel, data)
        rc, out = self.gate()
        self.assertEqual(rc, 1, out)
        self.assertIn(label, out)
        os.remove(os.path.join(self.root, rel))

    def test_clean_tree_passes(self):
        rc, out = self.gate()
        self.assertEqual(rc, 0, out)

    def test_base58_run(self):
        run = "5" + "K" * 87
        self.assertHit("k.md", f"key {run}\n", "base58 run")
        self.put("fx/tx.json", f'["{run}"]\n')  # fixtures are exempt
        rc, out = self.gate()
        self.assertEqual(rc, 0, out)
        self.put("k.md", f"key {run}\n")
        rc, out = self.gate("--allow-token", run)
        self.assertEqual(rc, 0, out)

    def test_hex_run_outside_locks(self):
        h = "ab" * 32
        self.assertHit("h.rs", f'const K: &str = "{h}";\n', "hex run")
        self.put("Cargo.lock", f'checksum = "{h}"\n')
        rc, out = self.gate()
        self.assertEqual(rc, 0, out)

    def test_key_parameter_in_url(self):
        for q in ("api-key", "api_key", "apikey", "dkey", "token", "secret"):
            with self.subTest(q=q):
                self.assertHit("u.ts", f'fetch("https://github.com/rpc?{q}=abc")\n', "key parameter")

    def test_rpc_vendor_urls(self):
        for host in ("lb.drpc.org", "devnet.helius-rpc.com", "x.solana-devnet.quiknode.pro",
                     "solana-devnet.g.alchemy.com"):
            with self.subTest(host=host):
                self.assertHit("u.ts", f'"https://{host}/"\n', "RPC vendor URL")

    def test_host_allowlist(self):
        self.assertHit("u.md", "https://private-dashboard.io/x\n", "host not on the allow list")
        self.put("package-lock.json", '{"resolved": "https://registry.npmjs.org/x"}\n')
        rc, out = self.gate()
        self.assertEqual(rc, 0, out)

    def test_ipv4_public_including_after_a_dot(self):
        self.assertHit("a.conf", "server 8.8." + "4.4;\n", "ipv4 literal")
        self.assertHit("b.conf", "host=x.8." + "8.4.4\n", "ipv4 literal")  # IP right after a dot
        self.put("v.md", "versions 1.2.3.4.5 and 10.0.0.1, 192.0." + "2.7, 127.0.0.1\n")
        rc, out = self.gate()
        self.assertEqual(rc, 0, out)

    def test_ipv6_other_than_loopback(self):
        self.assertHit("a.conf", "listen [2001:" + "4860:4860::8888]:80;\n", "ipv6 literal")
        self.put("b.conf", "listen [::1]:80; listen [::]:80; time 12:30:45\n")
        rc, out = self.gate()
        self.assertEqual(rc, 0, out)

    def test_multi_line_64_number_json(self):
        arr = "[\n" + ",\n".join(str(i) for i in range(64)) + "\n]"
        self.assertHit("k.json", '{"secret": ' + arr + "}\n", "keypair shape")
        self.put("ok.json", "[" + ",".join(str(i) for i in range(63)) + "]\n")
        rc, out = self.gate()
        self.assertEqual(rc, 0, out)

    def test_env_secret_lines_by_content(self):
        self.assertHit("notes.txt", "export DRIP_API_KEY=sk_live_abcdef\n", "env-style secret")
        self.assertHit("run.md", "WALLET_PRIVATE_KEY=3x9QabcDEF\n", "env-style secret")
        self.put("ok.md", "API_KEY=\nAUTH_TOKEN=<token>\nDRIP_KEYPAIR=~/k.json npm start\n"
                          "SEED=\nSECRET=your-secret\n")
        rc, out = self.gate()
        self.assertEqual(rc, 0, out)

    def test_binaries_allowlist_cap_and_denylist(self):
        self.assertHit("x.bin", b"\0\1\2", "binary file not on the allow list")
        self.assertHit("ok.png", b"\0" * 200, "binary over 100 bytes")
        self.assertHit("ok.png", b"\0\1" + OWNER.encode() + b"\0", "denylisted (binary)")

    def test_unicode_confusables_and_concatenation(self):
        tricked = OWNER.replace("a", "а", 1)  # Cyrillic a
        self.assertHit("c.md", f"by {tricked}\n", "denylisted")
        self.assertHit("w.md", "by " + OWNER[:3] + "​" + OWNER[3:] + "\n", "denylisted")
        self.assertHit("f.md", "by " + "ａ" + OWNER[1:] + "\n", "denylisted")  # fullwidth a, NFKC
        self.assertHit("j.ts", f'const h = "{OWNER[:3]}" + "{OWNER[3:]}";\n', "denylisted")

    def test_encoded_forms(self):
        n = (198 << 24) | (18 << 16) | (7 << 8) | 9
        self.assertHit("i.ts", f"const ip = {n};\n", "encoded form")
        b = base64.b64encode(f"user {OWNER} here".encode()).decode()
        self.assertHit("b.txt", b + "\n", "encoded form")

    def test_new_denylist_entries(self):
        with open(DENY) as fh:
            deny = fh.read().lower()
        for word in ("macprovider", "antseed", "ohmyclaude", "antfeed", "malibu", "flop-watch",
                     "flop-markets"):
            with self.subTest(word=word):
                self.assertIn(word, deny)


class MarketsPins(unittest.TestCase):
    """drip-markets: binaries pinned by sha256; fixtures are data files only."""

    setUp, tearDown, write, commit = Exporter.setUp, Exporter.tearDown, Exporter.write, Exporter.commit
    leftovers, exported = Exporter.leftovers, Exporter.exported
    commit_markets, export_markets = MarketsProfile.commit_markets, MarketsProfile.export_markets

    BLOB = b"\0asm\1\0\0\0" + bytes(range(16))

    def put_blob(self, data=None):
        with open(os.path.join(self.repo, "m.wasm"), "wb") as fh:
            fh.write(self.BLOB if data is None else data)

    def test_pinned_binary_passes(self):
        self.put_blob()
        h = hashlib.sha256(self.BLOB).hexdigest()
        self.commit_markets(f"m.wasm flop-index/testdata/m.wasm\nsha256: {h} flop-index/testdata/m.wasm\n")
        rc, out = self.export_markets()
        self.assertEqual(rc, 0, out)
        self.assertEqual(self.exported(), ["flop-index/testdata/m.wasm"])

    def test_unpinned_changed_and_unused_pins_fail(self):
        h = hashlib.sha256(self.BLOB).hexdigest()
        ip = bytes([198, 18, 7, 9])  # an address as 4 raw bytes: only the pin catches it
        cases = (
            (self.BLOB, "m.wasm flop-index/testdata/m.wasm\n", "without a sha256: pin"),
            (self.BLOB + ip, f"m.wasm flop-index/testdata/m.wasm\nsha256: {h} flop-index/testdata/m.wasm\n",
             "sha256 differs"),
            (self.BLOB, f"m.wasm flop-index/testdata/m.wasm\nsha256: {h} flop-index/testdata/m.wasm\n"
                        f"sha256: {h} flop-index/testdata/gone.wasm\n", "does not contain"),
            (self.BLOB, f"a.md README.md\nsha256: {h} README.md\nm.wasm x/m.wasm\n"
                        f"sha256: {h} x/m.wasm\n", "names a text file"),
            (self.BLOB, f"m.wasm x/m.wasm\nsha256: {h[:-1]} x/m.wasm\n", "bad manifest line"),
        )
        for data, manifest, why in cases:
            with self.subTest(why=why):
                self.put_blob(data)
                self.commit_markets(manifest)
                rc, out = self.export_markets()
                self.assertEqual(rc, 1, out)
                self.assertIn(why, out)
                self.assertEqual(self.leftovers(), [])

    def test_fixtures_are_data_files_only(self):
        prof = runpy.run_path(os.path.join(HERE, "export_public.py"), run_name="probe")["PROFILES"]
        for g in prof["drip-markets"]["strict"]["fixture"]:
            with self.subTest(glob=g):
                self.assertTrue(g.endswith((".json", "/MANIFEST")), g)

    def test_secret_appended_to_live_source_fails(self):
        key = "4" + "Zq" * 43  # 87 base58 chars
        for body in ('const RPC: &str = "https://github.com/rpc?api-key=abc";\n',
                     f'const K: &str = "{key}";\n'):
            with self.subTest(body=body[:30]):
                self.write("submit.rs", "// submit\n" + body)
                self.commit_markets("submit.rs cranker/src/submit.rs\n")
                rc, out = self.export_markets()
                self.assertEqual(rc, 1, out)
                self.assertIn("strict leak gate", out)
                self.assertEqual(self.leftovers(), [])


class StrictTokens(StrictGate):
    """check_public_strict.py, token shapes added after the second review."""

    def test_clean_tree_passes(self):  # inherited setUp tree, plus code that must stay clean
        self.put("ok.ts", 'const apiKey = process.env.KEY;\nconst token = "";\n'
                          'const secret = "<secret>";\nonKeyDown={e => go(e)}\nseed = (seed + 1) | 0;\n'
                          "[profile.dev]\n")
        rc, out = self.gate()
        self.assertEqual(rc, 0, out)

    def test_base64_keypair(self):
        b = base64.b64encode(bytes(range(64))).decode()
        self.assertHit("k.ts", f'const k = "{b}";\n', "base64 of 64 bytes")
        self.assertHit("k.ts", f'const k = "{b.rstrip("=")}";\n', "base64 of 64 bytes")
        u = base64.urlsafe_b64encode(bytes(range(190, 254))).decode()
        self.assertHit("k.ts", f"k={u}\n", "base64 of 64 bytes")

    def test_separated_hex_and_byte_lists(self):
        self.assertHit("h.txt", ":".join(f"{i:02x}" for i in range(32)) + "\n", "separated hex")
        self.assertHit("h.txt", " ".join(f"{i:02X}" for i in range(64)) + "\n", "separated hex")
        lst = ",\n    ".join(f"0x{i:02x}" for i in range(32))
        self.assertHit("b.rs", f"const K: [u8; 32] = [\n    {lst}\n];\n", "0x byte list")
        flat = "".join(f"{i:02x}" for i in range(32))
        self.put("b.rs", f"const K: [u8; 32] = [\n    {lst}\n];\n")
        rc, out = self.gate("--allow-token", flat)
        self.assertEqual(rc, 0, out)
        self.put("b.rs", "[" + ", ".join(f"0x{i:02x}" for i in range(31)) + "]\n")
        rc, out = self.gate()
        self.assertEqual(rc, 0, out)

    def test_token_prefixes(self):
        for t in ("ghp_" + "a1B2" * 9, "gho_" + "Zz09" * 9, "github_pat_" + "11AB_" * 8,
                  "sk-" + "proj-" + "x7Y" * 10, "xoxb-" + "1234-5678-abcdEFGH",
                  "AKIA" + "Q3XZ7KW2M4N6P8RT", "123456789:" + "AAF" + "q1w2e3r4" * 4):
            with self.subTest(t=t[:8]):
                self.assertHit("t.md", f"use {t} here\n", "token prefix")

    def test_uuid_assigned_to_secret_names(self):
        u = "3f2c9a1e-7b4d-4c8e-9f01-2a3b4c5d6e7f"
        for line in (f"API_KEY={u}", f'token: "{u}"', f"clientSecret = '{u}'", f"Password={u}"):
            with self.subTest(line=line):
                self.assertHit("u.txt", line + "\n", "UUID assigned")
        self.put("ok.txt", f"request_id = {u}\n")
        rc, out = self.gate()
        self.assertEqual(rc, 0, out)

    def test_secret_assignments_in_code(self):
        for line in ('const apiKey = "a8f9c2d1";', "api_key: 'q9w8e7'", 'TOKEN = "t0k3n-v4lue"',
                     'let password = "hunter2";', "client.secret = `s3cr3t`;"):
            with self.subTest(line=line):
                self.assertHit("c.ts", line + "\n", "secret-named assignment")
        self.put("c.ts", 'const apiKey = "a8f9c2d1";\n')
        rc, out = self.gate("--allow-value", "a8f9c2d1")
        self.assertEqual(rc, 0, out)

    def test_root_paths_ssh_forms_and_tailnet(self):
        self.assertHit("r.sh", "cp x /root/.config/solana/id.json\n", "/root/ path")
        self.assertHit("d.sh", "ssh -p 22 deploy@box.internal-host.io uptime\n", "user@host form")
        self.assertHit("d.md", "scp out.tar admin@10.1.2.3:/srv/\n", "user@host form")
        self.assertHit("d.md", "rsync -a dist/ ops@bastion:~/site\n", "user@host form")
        self.assertHit("t.md", "reach it at box-1.tail1234.ts.net\n", "tailnet name")
        self.put("ok.md", "git clone git@github.com:org/repo.git\n")
        rc, out = self.gate()
        self.assertEqual(rc, 0, out)

    def test_bare_host_names(self):
        self.assertHit("n.md", "the feed lives on metrics.private-co.io now\n", "host name not on the allow list")
        self.assertHit("n.toml", 'host = "rpc.somevendor.com"\n', "host name not on the allow list")
        self.put("ok.md", "see docs.github.com, api.example.com and build.sh, lib.rs, index.ts\n")
        rc, out = self.gate()
        self.assertEqual(rc, 0, out)

    def test_env_values_starting_with_slash_or_dollar_need_allow(self):
        for line in ("FAUCET_KEYPAIR=/srv/keys/faucet.json", "API_TOKEN=$PROD_TOKEN",
                     "db_password=/etc/pw", 'SECRET="$X"'):
            with self.subTest(line=line):
                self.assertHit("e.env.md", line + "\n", "env-style secret")
        self.put("e.md", "FAUCET_KEYPAIR=/srv/keys/faucet.json npm start\n")
        rc, out = self.gate("--allow-value", "/srv/keys/faucet.json")
        self.assertEqual(rc, 0, out)
        self.put("e.md", "DRIP_KEYPAIR=~/.config/drip/k.json\nAPI_KEY=<key>\nSEED=\n")
        rc, out = self.gate()
        self.assertEqual(rc, 0, out)

    def test_hex_equal_to_a_shipped_file_hash_passes(self):
        self.put("data.bin.txt", "payload\n")
        h = hashlib.sha256(b"payload\n").hexdigest()
        self.put("HASHES.md", f"{h}  data.bin.txt\n")
        rc, out = self.gate()
        self.assertEqual(rc, 0, out)
        self.assertHit("K.md", f"{hashlib.sha256(b'other').hexdigest()}  gone.txt\n", "hex run")


if __name__ == "__main__":
    unittest.main()

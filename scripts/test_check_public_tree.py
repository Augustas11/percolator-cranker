#!/usr/bin/env python3
"""Tests for the public export (private; never exported). Run:

    python3 scripts/test_check_public_tree.py

* Exporter: the allowlist is the control, so it is tested properly. Only
  manifest files are copied, from the committed HEAD; unlisted committed
  files are refused; bad destinations, duplicates and existing targets are
  refused; a failed export leaves nothing behind.
* Leak helper (check_public_tree.sh): a plain pattern check. One hit per
  pattern, and a clean tree passes. Bypasses are out of scope by design.
* Profiles: --profile drip-markets reads its own manifest; dir: entries
  expand committed files and refuse excluded or denylisted ones; skip:,
  allow-phrase:, anchored and cut rewrites, placeholder substitution and
  kept placeholders.
"""

import json
import os
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
        for f in ("export_public.py", "check_public_tree.sh"):
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
    """drip-markets: own manifest, dir:/skip:/allow-phrase: lines, rewrites, subs."""

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

    def test_dir_entry_expands_committed_files_only(self):
        self.write("frontend/index.html", "<p>site</p>\n")
        self.write("frontend/src/app.ts", "export {}\n")
        self.write("frontend/.gitignore", "dist\n")
        self.write("frontend/.env.example", "VITE_X=1\n")
        self.commit_markets("dir: frontend web\nfrontend/.env.example web/env.example\n")
        self.write("frontend/src/uncommitted.ts", "x\n")
        rc, out = self.export_markets()
        self.assertEqual(rc, 0, out)
        self.assertEqual(self.exported(), ["web/.gitignore", "web/env.example", "web/index.html",
                                           "web/src/app.ts"])

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

    def test_dir_entry_refuses_excluded_files_unless_skipped(self):
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
                self.commit_markets("dir: frontend frontend\n")
                rc, out = self.export_markets()
                self.assertEqual(rc, 1, out)
                self.assertIn(why, out)
                self.assertEqual(self.leftovers(), [])
                self.commit_markets(f"dir: frontend frontend\nskip: {path}\n")
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

    def test_dir_entry_refuses_denylisted_name(self):
        self.write(f"frontend/{OWNER}-notes.md", "x\n")
        self.commit_markets("dir: frontend frontend\n", deny=OWNER + "\n")
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


if __name__ == "__main__":
    unittest.main()

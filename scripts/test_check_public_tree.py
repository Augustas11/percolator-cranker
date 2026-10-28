#!/usr/bin/env python3
"""Tests for the public export (private; never exported). Run:

    python3 scripts/test_check_public_tree.py

* Exporter: the allowlist is the control, so it is tested properly. Only
  manifest files are copied, from the committed HEAD; unlisted committed
  files are refused; bad destinations, duplicates and existing targets are
  refused; a failed export leaves nothing behind.
* Leak helper (check_public_tree.sh): a plain pattern check. One hit per
  pattern, and a clean tree passes. Bypasses are out of scope by design.
"""

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


if __name__ == "__main__":
    unittest.main()

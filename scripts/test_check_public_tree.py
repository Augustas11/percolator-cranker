#!/usr/bin/env python3
"""Regression probes for the public content gate (private; never exported).

Every bypass found in audit rounds is planted in a fresh tree here, and the
gate must FAIL on it; a clean tree must pass. Run:

    python3 scripts/test_check_public_tree.py
"""

import os
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.realpath(__file__))
GATE = os.path.join(HERE, "check_public_tree.py")
DENY = os.path.join(HERE, "public_denylist.txt")
OWNER = "aug" + "star"  # denylisted owner handle, split so this file stays greppable
KEY64 = "[" + ",".join(str(i) for i in range(64)) + "]"


def run_gate(root, *flags, deny=True):
    args = [sys.executable, "-I", GATE, *flags, root] + ([DENY] if deny else [])
    p = subprocess.run(args, capture_output=True, text=True)
    return p.returncode, p.stdout + p.stderr


class Gate(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.mkdtemp(prefix="gate-probe-")
        self.put("README.md", "clean keeper docs, see http://127.0.0.1:8899 and "
                              "https://github.com/aeyakovenko/percolator-prog\n")

    def tearDown(self):
        for dirpath, dirs, _ in os.walk(self.root):
            for d in dirs:
                os.chmod(os.path.join(dirpath, d), 0o755)
        shutil.rmtree(self.root, ignore_errors=True)

    def put(self, rel, text=None, data=None):
        p = os.path.join(self.root, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "wb") as f:
            f.write(data if data is not None else text.encode("utf-8"))
        return p

    def assertFails(self, needle=None, *flags):
        rc, out = run_gate(self.root, *flags)
        self.assertEqual(rc, 1, out)
        if needle:
            self.assertIn(needle, out)
        return out

    def test_clean_tree_passes(self):
        rc, out = run_gate(self.root)
        self.assertEqual(rc, 0, out)

    # ---- names, links, encodings, walk errors ----
    def test_names_with_space_quote_newline(self):
        for name in ["a b.md", "maintainer's.md", "x\ny.md"]:
            self.put(name, f"{OWNER}\n")
        out = self.assertFails("bad file name")
        self.assertEqual(out.count("bad file name"), 3, out)

    def test_dot_components(self):
        self.put(".hidden/x.md", "x\n")
        self.put("docs/.secret.md", "x\n")
        self.assertFails("dot path component")

    def test_symlinks(self):
        self.put("real.md", "x\n")
        os.symlink("real.md", os.path.join(self.root, "link.md"))
        os.symlink(self.root, os.path.join(self.root, "dirlink"))
        out = self.assertFails("symlink")
        self.assertIn("dirlink", out)

    def test_late_binary_and_utf16(self):
        self.put("a.md", data=b"text\n" * 50000 + b"\0tail")
        self.put("b.md", data=b"public padding\n" * 15000 + f"{OWNER}\n{KEY64}\n".encode("utf-16le"))
        out = self.assertFails("non-text file")
        self.assertEqual(out.count("non-text file"), 2, out)

    def test_unreadable_directory(self):
        self.put("hidden/secret.md", f"{OWNER}\n")
        os.chmod(os.path.join(self.root, "hidden"), 0)
        if os.geteuid() == 0:
            self.skipTest("root can read anything")
        self.assertFails("SCAN ERROR")

    def test_missing_dir_is_an_error(self):
        rc, _ = run_gate(os.path.join(self.root, "nope"))
        self.assertEqual(rc, 2)

    # ---- secrets ----
    def test_secret_file_names(self):
        for name in [".env.production", ".envrc", ".env-production", "id.json.bak",
                     "WALLET-main.JSON", "deploy-keypair.json", "x.pem", "y.key"]:
            self.put("s/" + name if not name.startswith(".") else name, "x\n")
        out = self.assertFails()
        for name in [".env.production", ".envrc", ".env-production", "id.json.bak",
                     "WALLET-main.JSON", "deploy-keypair.json", "x.pem", "y.key"]:
            self.assertIn(name, out)  # dot files fail as dot components; the rest as secrets

    def test_key_arrays_any_layout(self):
        self.put("k.json", "[\n" + ",\n  ".join(str(i) for i in range(64)) + "\n]\n")
        self.put("k.rs", "let k: [u8; 64] = " + ", ".join(str(i) for i in range(64)).join("[]") + ";\n")
        out = self.assertFails("keypair-like")
        self.assertEqual(out.count("keypair-like"), 2, out)

    def test_key_markers(self):
        self.put("a.md", "seed phrase = alpha beta gamma\n")
        self.put("b.md", "mnemonic: alpha beta\n")
        self.put("c.md", "-----BEGIN PRIVATE KEY-----\n")
        out = self.assertFails("private key marker")
        self.assertEqual(out.count("private key marker"), 3, out)

    def test_plural_mnemonics_in_prose_is_fine(self):
        self.put("CONTRIBUTING.md", "No keys, mnemonics or RPC tokens in commits.\n")
        rc, out = run_gate(self.root)
        self.assertEqual(rc, 0, out)

    # ---- paths, emails, urls ----
    def test_absolute_paths(self):
        self.put("p.md", "/home/Alice/x\n/users/bob/y\n/Volumes/z\n/root/q\n/private/var/k\nC:\\Users\\x\n")
        out = self.assertFails("absolute path")
        self.assertEqual(out.count("absolute path"), 6, out)

    def test_emails(self):
        self.put("e.md", "mail owner@gmail.com\n")
        self.assertFails("email")

    def test_security_contact_email_allowed_only_in_security_md(self):
        self.put("SECURITY.md", "report to sec@team.dev\n")
        rc, out = run_gate(self.root, "--allow-email", "sec@team.dev")
        self.assertEqual(rc, 0, out)
        self.put("README2.md", "or sec@team.dev\n")
        rc, out = run_gate(self.root, "--allow-email", "sec@team.dev")
        self.assertEqual(rc, 1, out)

    def test_urls(self):
        self.put("u.md", "\n".join([
            "https://sh.rustup.rs.evil.com/x",
            "HTTPS://evil.example",
            "https://docs.rs.evil.com/a",
            "http://127.0.0.1@remote.example:8899/",
            "https://github.com/aeyakovenko/percolator-evil",
            "ftp://127.0.0.1/x",
        ]) + "\n")
        out = self.assertFails("url not allowlisted")
        self.assertEqual(out.count("url not allowlisted"), 6, out)

    def test_schemeless_github(self):
        self.put("g.md", "see github.com/someone/private\n"
                         "and github.com/aeyakovenko/percolator plus github.com/owner2/repo\n"
                         "github.com/aeyakovenko/percolator-progx\n")
        out = self.assertFails("github mention")
        self.assertEqual(out.count("github mention"), 3, out)

    def test_allowed_urls_pass(self):
        self.put("ok.md", "https://github.com/aeyakovenko/percolator/issues/243\n"
                          "https://github.com/aeyakovenko/percolator-prog.git\n"
                          "http://[::1]:8899 http://localhost:8899/ https://release.anza.xyz/v2.0.21/install\n")
        rc, out = run_gate(self.root)
        self.assertEqual(rc, 0, out)

    # ---- fixture marker ----
    def test_fixture_marker_scope(self):
        m = "// public-check: allow-test-fixture"
        self.put("crates/c/tests/ok.rs", f'"http://127.0.0.1@remote.example:8899" {m}\n')
        rc, out = run_gate(self.root)
        self.assertEqual(rc, 0, out)
        self.put("src/x.rs", f'"https://evil.example" {m}\n')
        self.put("crates/c/tests/bad.rs", f'"https://github.com/someone/privaterepo owner@gmail.com" {m}\n')
        self.put("crates/c/tests/bad2.rs", f'"http://127.0.0.1@realhost.com/" {m}\n')
        out = self.assertFails()
        self.assertIn("src/x.rs", out)
        self.assertIn("bad.rs", out)
        self.assertIn("bad2.rs", out)

    # ---- operator stack, placeholders, denylist ----
    def test_operator_paths(self):
        for p in ["Staging/a.rs", "src/staging-bot/a.rs", "trader/a.rs", "matcher/a.rs",
                  "backing_manager/a.rs", "src/monitoring/a.rs", "loadtest/a.rs",
                  "docs/operator_playbook.md", "scripts/devnet_deploy.sh",
                  "scripts/staging-local.sh", "x/my_devnet_ops.rs", "docs/STAGING.md",
                  "monitor/alerts.yml", "web/dashboard.html", "cranker/src/bin/live_shaped.rs"]:
            self.put(p, "x\n")
        out = self.assertFails("operator-stack path")
        self.assertEqual(out.count("operator-stack path"), 15, out)

    def test_placeholders(self):
        self.put("SECURITY.md", "contact @SECURITY_CONTACT@\n")
        self.assertFails("unreplaced placeholder")
        rc, out = run_gate(self.root, "--draft")
        self.assertEqual(rc, 0, out)

    def test_denylist_contents_and_names(self):
        self.put("notes.md", f"{OWNER.upper()} wrote this\n")
        self.put(f"docs/{OWNER}-notes.md", "x\n")
        out = self.assertFails("denylisted")
        self.assertIn("in file name", out)

    def test_skips_only_top_level(self):
        self.put("crates/x/src/upstream/leak.rs", f"{OWNER}\n")
        self.put("crates/x/target/leak.rs", f"{OWNER}\n")
        out = self.assertFails("denylisted")
        self.assertEqual(out.count("denylisted"), 2, out)
        shutil.rmtree(os.path.join(self.root, "crates"))
        self.put("upstream/x.rs", f"{OWNER}\n")
        self.put("target/x.rs", f"{OWNER}\n")
        rc, out = run_gate(self.root)
        self.assertEqual(rc, 0, out)


class GateRound3(Gate):
    """Round-3 audit bypasses."""

    def test_gate_scripts_are_scanned_too(self):
        self.put("scripts/check_public_tree.py", f"# seed phrase = alpha beta gamma\n# {OWNER}\n")
        self.put("scripts/check_public_tree.sh", f"# {OWNER}\n")
        out = self.assertFails()
        self.assertIn("scripts/check_public_tree.py", out)
        self.assertIn("scripts/check_public_tree.sh", out)

    def test_real_gate_source_passes_its_own_rules(self):
        shutil.copy(GATE, os.path.join(self.root, "check.py"))
        shutil.copy(os.path.join(HERE, "check_public_tree.sh"), os.path.join(self.root, "check.sh"))
        rc, out = run_gate(self.root)
        self.assertEqual(rc, 0, out)

    def test_rust_key_array_forms(self):
        vals = [str(i) for i in range(64)]
        self.put("a.rs", "const K: [u8; 64] = [" + ", ".join(vals) + ",];\n")
        self.put("b.rs", "const K: [u8; 64] = [" + ", ".join(hex(i) for i in range(64)) + "];\n")
        self.put("c.rs", "const K: [u8; 64] = [" + ", ".join(v + "u8" for v in vals) + "];\n")
        self.put("d.rs", "const K = [0, /* comment */ " + ", ".join(vals[1:]) + "];\n")
        out = self.assertFails("keypair-like")
        self.assertEqual(out.count("keypair-like"), 4, out)

    def test_recovery_markers_any_separator_and_multiline(self):
        self.put("a.md", 'seed_phrase = "alpha"\n')
        self.put("b.md", "seed-phrase: alpha\n")
        self.put("c.md", "seed\nphrase: alpha beta\n")
        self.put("d.md", "recovery phrase alpha\n")
        out = self.assertFails("private key marker")
        self.assertEqual(out.count("private key marker"), 4, out)

    def test_github_traversal(self):
        self.put("g.md", "\n".join([
            "https://github.com/aeyakovenko/percolator/../../other/private",
            "https://github.com/aeyakovenko/percolator/%2e%2e/%2e%2e/other/private",
            "https://github.com/aeyakovenko/percolator/issues/../../../other/x",
            "github.com/aeyakovenko/percolator/../../other/private",
        ]) + "\n")
        out = self.assertFails()
        self.assertGreaterEqual(out.count("g.md"), 4, out)

    def test_fixture_cannot_exempt_github_or_empty_host(self):
        m = "// public-check: allow-test-fixture"
        self.put("crates/c/tests/a.rs", f"github.com/other-owner/private-repo {m}\n")
        self.put("crates/c/tests/b.rs", f'"http:///realhost.com/private" {m}\n')
        out = self.assertFails()
        self.assertIn("a.rs", out)
        self.assertIn("b.rs", out)

    def test_cargo_lock_github_mentions_checked(self):
        self.put("Cargo.lock", "# github.com/other-owner/private-repo\n")
        self.assertFails("github mention")

    def test_trailing_newline_name_and_fifo(self):
        self.put("note.md\n", "x\n")
        os.mkfifo(os.path.join(self.root, "pipe.md"))
        out = self.assertFails()
        self.assertIn("bad file name", out)
        self.assertIn("not a regular file", out)


if __name__ == "__main__":
    unittest.main(verbosity=1)

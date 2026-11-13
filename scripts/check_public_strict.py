#!/usr/bin/env python3
"""Strict leak gate for an exported public tree (private; never exported).

    check_public_strict.py <dir> <denylist> [options]

Run by export_public.py after check_public_tree.sh for profiles that ask for
it (drip-markets). Like the shell helper it is a pattern check, not a proof;
it covers what a grep -I of fixed strings misses. One line per hit; exit 1 on
any hit, 2 on bad usage. Checks, on every file (binaries included):

* denylist: each fixed string, case-insensitive, after NFKC, removal of
  zero-width characters and folding of Latin look-alikes (Cyrillic, Greek),
  on the text and on the text with string concatenations ("a" + "b")
  removed; for an IPv4 entry also its integer and hex forms; for every entry
  its base64 forms. --allow-phrase WORDS exempts a hit inside a whole-word
  WORDS. Binaries are read as bytes.
* binaries: refused unless they match --allow-binary GLOB and are at most
  --binary-cap bytes.
Text files only:
* base58 runs of 80+ characters (a 64-byte secret key or a signature), and
  hex runs of 64+ characters, outside lock files and --fixture GLOB files,
  unless listed with --allow-token (byte lists: as plain lower-case hex) or,
  for hex, equal to the sha256 of a file in the tree;
* URLs: a key-bearing query parameter (api-key, dkey, token, secret, ...)
  unless the host is reserved or single-label; any dRPC, Helius, QuickNode
  or Alchemy URL; a host outside --allow-domain (and outside reserved,
  single-label and IP hosts, which have their own checks), except in lock
  files (registry and funding links);
* --fixture files are exempt from the base58, hex, key-parameter and host
  checks only (recorded transactions, hashes, URL-parsing tests);
* IPv4 other than loopback, private, unspecified and documentation ranges,
  each address judged alone; IPv6 other than loopback and unspecified;
* JSON: any array of exactly 64 integers 0-255 (a keypair), however laid out;
* env-style lines (NAME=value) whose name looks secret (any case) and whose
  value is not a placeholder, whatever the file is called; a value starting
  with / or $ counts only when listed with --allow-value;
* token shapes: base64 that decodes to 64 bytes (a keypair), hex with : or
  space separators and 0x.. byte lists of 32+ bytes, known token prefixes
  (ghp_, gho_, github_pat_, sk-, xox?-, AKIA, Telegram bot tokens), UUIDs
  assigned to key/token/secret/password names, and quoted values assigned
  to apiKey/token/secret/password names in code (placeholders and
  --allow-value values pass);
* /root/ paths, user@host SSH forms and *.ts.net names; bare host names
  (no scheme) outside --allow-domain.
The token-shape and bare-host checks skip lock files and --fixture files.
"""

import argparse
import base64
import fnmatch
import hashlib
import ipaddress
import json
import os
import re
import sys
import unicodedata

# Latin look-alikes (Cyrillic, Greek, a few IPA letters) -> Latin, lower case.
CONFUSABLE = str.maketrans({
    "а": "a", "в": "b", "е": "e", "к": "k", "м": "m", "н": "h", "о": "o", "р": "p",
    "с": "c", "т": "t", "у": "y", "х": "x", "і": "i", "ј": "j", "ѕ": "s", "ԁ": "d",
    "ԛ": "q", "ԝ": "w", "ɡ": "g", "ӏ": "l", "ү": "y", "һ": "h", "ɑ": "a", "ο": "o",
    "α": "a", "ε": "e", "ι": "i", "κ": "k", "ν": "v", "ρ": "p", "τ": "t", "υ": "u",
    "χ": "x", "ω": "w", "ѵ": "v", "ɩ": "i", "ı": "i", "ℓ": "l",
})
ZERO_WIDTH = dict.fromkeys(map(ord, "​‌‍⁠﻿­"))
CONCAT = re.compile(r"""["'`]\s*\+\s*["'`]""")
B58 = re.compile(r"(?<![1-9A-HJ-NP-Za-km-z])[1-9A-HJ-NP-Za-km-z]{80,}(?![1-9A-HJ-NP-Za-km-z])")
HEX = re.compile(r"(?<![0-9A-Fa-f])[0-9A-Fa-f]{64,}(?![0-9A-Fa-f])")
URL = re.compile(r"\b(?:https?|wss?)://[^\s\"'<>()\[\]{}`\\,]+")
SECRET_PARAM = re.compile(r"[?&;](?:api[-_]?key|dkey|token|secret|access[-_]?token|auth)=", re.I)
RPC_VENDOR = re.compile(r"drpc|helius|quiknode|quicknode|alchemy", re.I)
IPV4 = re.compile(r"\d+\.\d+\.\d+\.\d+")
IPV6 = re.compile(r"(?<![0-9A-Za-z_:.])(?:[0-9A-Fa-f]{0,4}:){2,7}[0-9A-Fa-f]{0,4}(?![0-9A-Za-z_:])")
ENV_LINE = re.compile(r"^\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$")
SECRET_NAME = re.compile(r"(?i:KEY|SECRET|TOKEN|PASSWORD)|PASSWD|MNEMONIC|SEED|PRIVATE")
PLACEHOLDER = re.compile(r"""^(|["']{2}|\$.*|<.*>|\.\.\.|.*\.\.\.|your.*|changeme|none|null|x+|example.*|[~.]?/\S*)$""", re.I)
RESERVED = re.compile(r"(^|\.)(example(\.(com|org|net))?|invalid|test|localhost)$", re.I)
LOCKS = {"Cargo.lock", "package-lock.json"}
B64_64 = re.compile(r"(?<![A-Za-z0-9+/_-])[A-Za-z0-9+/_-]{86}(?:==)?(?![A-Za-z0-9+/_=-])")
HEX_SEP = re.compile(r"(?<![0-9A-Fa-f:])(?:[0-9A-Fa-f]{2}[: ]){31,}[0-9A-Fa-f]{2}(?![0-9A-Fa-f:])")
BYTE_LIST = re.compile(r"(?:0x[0-9A-Fa-f]{1,2}\s*,\s*){31,}0x[0-9A-Fa-f]{1,2}(?![0-9A-Fa-f])")
TOKEN_PREFIX = re.compile(
    r"\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|sk-[A-Za-z0-9_-]{20,}"
    r"|xox[baprs]-[A-Za-z0-9-]{10,}|AKIA[0-9A-Z]{16})"
    r"|(?<![\w:])\d{8,10}:[A-Za-z0-9_-]{35}(?![\w-])")
UUID = r"[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}"
UUID_ASSIGN = re.compile(r"(?i)(?:key|token|secret|password)[\w-]*[\"'`]?\s*[:=]\s*[\"'`]?" + UUID)
SECRET_ASSIGN = re.compile(
    r"(?i)\b([\w-]*(?:api[_-]?key|token|secret|passw(?:or)?d)[\w-]*)[\"']?\s*[:=]\s*([\"'`])([^\"'`\n]*)\2")
CODE_PLACEHOLDER = re.compile(r"(?i)^(|<.*>|\$\{.*\}|\{\{.*\}\}|your[-_ ].*|x{3,}|\*+|\.\.\.|changeme|example.*|test|none|null|redacted)$")
ROOT_PATH = re.compile(r"(?<![\w.~-])/root/")
SSH_CMD = re.compile(r"\b(?:ssh|scp|sftp|rsync)\s[^\n]*?[\w.-]+@([\w.-]+)")
SSH_PATH = re.compile(r"(?<![\w.+/@-])[\w.-]+@([A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*):[\w~/]")
TS_NET = re.compile(r"(?i)\b[\w-]+(?:\.[\w-]+)*\.ts\.net\b")
BARE_HOST = re.compile(
    r"(?<![\w.@/:\[-])((?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+(?:com|net|org|io|dev|ai|xyz|cloud|tech|pro|gg|info))"
    r"(?![\w-]|\.[A-Za-z0-9])", re.I)


def fold(text):
    return unicodedata.normalize("NFKC", text).translate(ZERO_WIDTH).translate(CONFUSABLE).lower()


def b64_cores(s):
    """Base64 of s at each of the 3 alignments, minus the edge characters."""
    out = set()
    for pad in range(3):
        enc = base64.b64encode(b"\0" * pad + s.encode()).decode().rstrip("=")
        lead = (0, 2, 3)[pad]  # characters that mix in the padding bytes
        core = enc[lead:len(enc) - 2]
        if len(core) >= 6:
            out.add(core)
    return out


def ip_forms(p):
    try:
        ip = ipaddress.IPv4Address(p)
    except ValueError:
        return set()
    n = int(ip)
    return {str(n), f"0x{n:08x}"}


def public_v4(ip):
    o = ip.split(".")
    if any(int(x) > 255 or (len(x) > 1 and x[0] == "0") for x in o):
        return False
    a = ipaddress.IPv4Address(ip)
    doc = any(a in ipaddress.IPv4Network(n) for n in ("192.0.2.0/24", "198.51.100.0/24", "203.0.113.0/24"))
    return not (a.is_loopback or a.is_private or a.is_unspecified or doc or o[0] == "0")


def covered(line, start, end, phrases):
    for ph in phrases:
        i = line.find(ph)
        while i >= 0:
            j = i + len(ph)
            if i <= start and j >= end and not (i > 0 and line[i - 1].isalnum()) \
                    and not (j < len(line) and line[j].isalnum()):
                return True
            i = line.find(ph, i + 1)
    return False


def deny_hits(text, pats, phrases):
    for variant in (fold(text), fold(CONCAT.sub("", text))):
        for line in variant.split("\n"):
            for p in pats:
                i = line.find(p)
                while i >= 0:
                    if not covered(line, i, i + len(p), phrases):
                        return p
                    i = line.find(p, i + 1)
    return None


def json_keys(v):
    if isinstance(v, list):
        if len(v) == 64 and all(isinstance(x, int) and not isinstance(x, bool) and 0 <= x <= 255 for x in v):
            return True
        return any(json_keys(x) for x in v)
    if isinstance(v, dict):
        return any(json_keys(x) for x in v.values())
    return False


def host_of(url):
    rest = url.split("://", 1)[1]
    hostport = re.split(r"[/?#]", rest, maxsplit=1)[0]
    hostport = hostport.rsplit("@", 1)[-1]
    if hostport.startswith("["):
        return hostport[1:].split("]", 1)[0]
    return hostport.split(":", 1)[0].rstrip(".")


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("root")
    ap.add_argument("denylist")
    ap.add_argument("--allow-phrase", action="append", default=[])
    ap.add_argument("--allow-domain", action="append", default=[])
    ap.add_argument("--allow-token", action="append", default=[])
    ap.add_argument("--fixture", action="append", default=[])
    ap.add_argument("--allow-binary", action="append", default=[])
    ap.add_argument("--allow-value", action="append", default=[])
    ap.add_argument("--binary-cap", type=int, default=256 * 1024)
    a = ap.parse_args(argv)
    if not os.path.isdir(a.root) or not os.path.isfile(a.denylist):
        print("usage: check_public_strict.py <dir> <denylist> [options]", file=sys.stderr)
        return 2
    pats = []
    for raw in open(a.denylist, encoding="utf-8"):
        p = raw.split("#", 1)[0].strip()
        if p:
            pats.append(fold(p))
    raw_forms = set()
    for p in pats:
        raw_forms |= ip_forms(p) | b64_cores(p)
    phrases = [fold(p) for p in a.allow_phrase]
    domains = [d.lower().lstrip(".") for d in a.allow_domain]
    tokens = set(a.allow_token)
    values = set(a.allow_value)

    def allowed_host(host):
        host = host.lower().rstrip(".")
        return bool(RESERVED.search(host)) or any(host == d or host.endswith("." + d) for d in domains)

    hits = []
    cache = []

    def tree_hashes():
        """sha256 of every file in the tree: a hash list of shipped files is not a secret."""
        if not cache:
            got = set()
            for d, _, fs in os.walk(a.root):
                for f in fs:
                    with open(os.path.join(d, f), "rb") as fh:
                        got.add(hashlib.sha256(fh.read()).hexdigest())
            cache.append(got)
        return cache[0]

    def hit(rel, what):
        hits.append(f"  {what}: {rel}")

    for dirpath, dirs, files in os.walk(a.root):
        rel_dir = os.path.relpath(dirpath, a.root)
        dirs[:] = [d for d in dirs if d != ".git" and not (rel_dir == "." and d in ("target", "upstream"))]
        for fn in sorted(files):
            path = os.path.join(dirpath, fn)
            rel = os.path.normpath(os.path.join(rel_dir, fn))
            data = open(path, "rb").read()
            try:
                text = data.decode("utf-8")
                binary = "\0" in text
            except UnicodeDecodeError:
                binary = True
            fixture = any(fnmatch.fnmatchcase(rel, g) for g in a.fixture)
            if binary:
                if not any(fnmatch.fnmatchcase(rel, g) for g in a.allow_binary):
                    hit(rel, "binary file not on the allow list")
                elif len(data) > a.binary_cap:
                    hit(rel, f"binary over {a.binary_cap} bytes")
                low = data.lower()
                for p in pats:
                    if p.encode("utf-8", "ignore") in low:
                        hit(rel, f"denylisted (binary): {p}")
                        break
                continue
            p = deny_hits(text, pats, phrases) or deny_hits(fn, pats, [])
            if p:
                hit(rel, f"denylisted: {p}")
            for f in raw_forms:
                if f in text:
                    hit(rel, f"denylisted (encoded form): {f}")
            if fn not in LOCKS and not fixture:
                for m in B58.findall(text):
                    if m not in tokens:
                        hit(rel, f"base58 run of {len(m)} chars: {m[:12]}...")
                for m in HEX.findall(text):
                    if m not in tokens and m.lower() not in tree_hashes():
                        hit(rel, f"hex run of {len(m)} chars: {m[:12]}...")
            for url in URL.findall(text):
                host = host_of(url).lower()
                if RPC_VENDOR.search(host):
                    hit(rel, f"RPC vendor URL: {url}")
                    continue
                if any(c in host for c in "@${}%<>") or not host:
                    continue  # a placeholder
                is_ip = bool(re.fullmatch(r"[0-9.]+", host)) or ":" in host
                reserved = bool(RESERVED.search(host)) or "." not in host
                if SECRET_PARAM.search(url) and not reserved and not fixture:
                    hit(rel, f"URL with a key parameter: {url}")
                if fn not in LOCKS and not fixture and not is_ip and not reserved and not any(host == d or host.endswith("." + d) for d in domains):
                    hit(rel, f"host not on the allow list: {host}")
            for m in IPV4.finditer(text):
                s, e = m.start(), m.end()
                before = text[max(0, s - 2):s]
                after = text[e:e + 2]
                if (before[-1:].isdigit() or (before[-1:] == "." and before[:1].isdigit())
                        or after[:1].isdigit() or (after[:1] == "." and after[1:2].isdigit())):
                    continue  # part of a longer dotted number (a version)
                if public_v4(m.group()):
                    hit(rel, f"ipv4 literal: {m.group()}")
            for m in IPV6.finditer(text):
                c = m.group()
                if c.count(":") < 2 or not re.search(r"[0-9]", c):
                    continue
                try:
                    ip = ipaddress.IPv6Address(c)
                except ValueError:
                    continue
                if not (ip.is_loopback or ip.is_unspecified):
                    hit(rel, f"ipv6 literal: {c}")
            if fn.endswith(".json"):
                try:
                    if json_keys(json.loads(text)):
                        hit(rel, "JSON array of 64 bytes (keypair shape)")
                except ValueError:
                    pass
            for line in text.split("\n"):
                m = ENV_LINE.match(line)
                if m and SECRET_NAME.search(m.group(1)):
                    words = m.group(2).split()  # NAME=value command ...: judge value
                    v = words[0] if words else ""
                    if v in values:
                        continue
                    if not m.group(1).isupper() and (v[:1] in "{([" or v.replace("_", "").isdigit()):
                        continue  # code (onKeyDown={...}, seed = (...)), or a number
                    if v.startswith(("/", "$", '"/', '"$', "'/", "'$")) or not PLACEHOLDER.match(v):
                        hit(rel, f"env-style secret: {m.group(1)}=")
            for m in ROOT_PATH.finditer(text):
                hit(rel, "/root/ path")
                break
            for m in TS_NET.finditer(text):
                hit(rel, f"tailnet name: {m.group()}")
            for rx in (SSH_CMD, SSH_PATH):
                for m in rx.finditer(text):
                    if not allowed_host(m.group(1)):
                        hit(rel, f"user@host form: {m.group()}")
            for m in UUID_ASSIGN.finditer(text):
                hit(rel, f"UUID assigned to a secret name: {m.group()[:40]}")
            for m in (SECRET_ASSIGN.finditer(text) if fn not in LOCKS else ()):
                v = m.group(3)
                if v not in values and not CODE_PLACEHOLDER.match(v.strip()):
                    hit(rel, f"secret-named assignment: {m.group(1)} = {v[:12]!r}")
            if fn not in LOCKS and not fixture:
                for m in TOKEN_PREFIX.finditer(text):
                    if m.group() not in tokens:
                        hit(rel, f"token prefix: {m.group()[:12]}...")
                for m in B64_64.finditer(text):
                    t = m.group()
                    if t in tokens:
                        continue
                    try:
                        raw = base64.b64decode(t.replace("-", "+").replace("_", "/") + "=" * (-len(t) % 4))
                    except ValueError:
                        continue
                    if len(raw) == 64:
                        hit(rel, f"base64 of 64 bytes (keypair shape): {t[:12]}...")
                for rx, what in ((HEX_SEP, "separated hex"), (BYTE_LIST, "0x byte list")):
                    for m in rx.finditer(text):
                        flat = "".join(b[-2:].rjust(2, "0") for b in re.findall(r"(?:0x)?[0-9A-Fa-f]{1,2}", m.group()))
                        if flat.lower() not in tokens:
                            hit(rel, f"{what} of 32+ bytes: {m.group()[:20]}...")
                for m in BARE_HOST.finditer(text):
                    if not allowed_host(m.group(1)):
                        hit(rel, f"host name not on the allow list: {m.group(1)}")
    print(f"== check_public_strict: {a.root}")
    for h in hits:
        print(h)
    if hits:
        print(f"== FAIL ({len(hits)} hits)")
        return 1
    print("== OK")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

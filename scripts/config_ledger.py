#!/usr/bin/env python3
"""Config change ledger between two commits (docs/CALIBRATION.md, "Config change ledger").

Compares the `pub struct Config` fields and the `impl Default for Config` values of
crates/econsim-core/src/config.rs at two revisions and prints added/removed fields and changed defaults.

    python3 scripts/config_ledger.py 8910b23 2e966a5 > ledger.txt
"""
import re
import subprocess
import sys
from pathlib import Path

PATH = "crates/econsim-core/src/config.rs"
REPO = Path(__file__).resolve().parent.parent


def show(rev):
    return subprocess.run(["git", "-C", str(REPO), "show", f"{rev}:{PATH}"],
                          capture_output=True, text=True, check=True).stdout


def struct_fields(src):
    """Field name -> type. A `#[serde(rename = "...")]` names the JSON key the config is read by; the ledger reports that key
    (stage-3 fix round, R10: the JSON key is what a user types, so a field whose key differs is shown as `field (key)`)."""
    m = re.search(r"pub struct Config \{(.*?)\n\}", src, re.S)
    out = {}
    rename = None
    for line in m.group(1).splitlines():
        r = re.match(r'\s+#\[serde\(rename = "(\w+)"\)\]', line)
        if r:
            rename = r.group(1)
            continue
        f = re.match(r"\s+pub (\w+):\s*(.+?),\s*$", line)
        if f:
            name = f"{f.group(1)} (key {rename})" if rename and rename != f.group(1) else f.group(1)
            out[name] = f.group(2)
            rename = None
    return out


def defaults(src):
    start = src.index("impl Default for Config {")
    body = src[start:src.index("\n}\n", start)]
    vals = {}
    for line in body.splitlines():
        m = re.match(r"^\s{12}(\w+):\s*(.+?),?\s*$", line)
        if m:
            vals[m.group(1)] = m.group(2).rstrip(",")
    return vals


def main(old, new):
    so, sn = show(old), show(new)
    fo, fn = struct_fields(so), struct_fields(sn)
    do, dn = defaults(so), defaults(sn)
    print(f"# Config ledger {old} -> {new} ({PATH})")
    print(f"# struct fields: {len(fo)} -> {len(fn)}; default entries: {len(do)} -> {len(dn)}")
    for k in fn:
        if k not in fo:
            print(f"ADD {k}: {fn[k]} (default {dn.get(k, '-')})")
    for k in fo:
        if k not in fn:
            print(f"DEL {k}: {fo[k]}")
    for k in dn:
        if k in do and do[k] != dn[k]:
            print(f"CHG {k}: {do[k]} -> {dn[k]}")
    for k in dn:
        if k not in do:
            print(f"NEW-DEFAULT {k} = {dn[k]}")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])

#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0
"""dev/audit.py: check the crates in Cargo.lock against the advisories known
to osv.dev (RustSec's among them). Sends crate names and versions there,
nothing else. Exit code 1 when something was found.

Unmaintained crates are listed too, apart; they do not fail the check
(pass --strict to make them)."""
import json
import re
import sys
import urllib.request

API = "https://api.osv.dev/v1"


def ask(path, body=None):
    request = urllib.request.Request(
        f"{API}/{path}",
        json.dumps(body).encode() if body is not None else None,
        {"Content-Type": "application/json", "User-Agent": "cargo-lock-check"},
    )
    return json.load(urllib.request.urlopen(request, timeout=60))


def main():
    lock = open("Cargo.lock", encoding="utf-8").read()
    crates = []
    for block in lock.split("[[package]]")[1:]:
        source = re.search(r'source = "(.*?)"', block)
        if source and "crates.io" in source.group(1):
            crates.append(
                (re.search(r'name = "(.*?)"', block).group(1), re.search(r'version = "(.*?)"', block).group(1))
            )
    found, unmaintained = [], []
    for start in range(0, len(crates), 500):
        part = crates[start : start + 500]
        queries = [{"package": {"name": name, "ecosystem": "crates.io"}, "version": version} for name, version in part]
        for (name, version), result in zip(part, ask("querybatch", {"queries": queries})["results"]):
            for advisory in result.get("vulns", []):
                summary = ask(f"vulns/{advisory['id']}").get("summary", "")
                line = f"{name} {version}: {advisory['id']} {summary}"
                (unmaintained if "unmaintained" in summary.lower() or "no longer maintained" in summary.lower() else found).append(line)
    print(f"{len(crates)} crates checked.")
    for line in found:
        print("  " + line)
    if unmaintained:
        print("Unmaintained:")
        for line in unmaintained:
            print("  " + line)
    strict = "--strict" in sys.argv
    return 1 if found or (strict and unmaintained) else 0


if __name__ == "__main__":
    sys.exit(main())

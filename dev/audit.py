#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0
"""dev/audit.py: check the crates in Cargo.lock against the advisories known
to osv.dev (RustSec's among them). Sends crate names and versions there,
nothing else. Exit code 1 when something was found.

Unmaintained crates are listed too, apart; they do not fail the check
(pass --strict to make them). Neither do the advisories in ACCEPTED,
each looked at and with its reason beside it."""
import json
import re
import sys
import urllib.parse
import urllib.request

API = "https://api.osv.dev/v1"

# Advisories that were looked at and do not fail the check: the crate, the
# advisory (any of its names) and why. Take an entry out when its crate
# is updated; the check says when one is no longer found.
ACCEPTED = {
    ("libsqlite3-sys", "RUSTSEC-2026-0459"): "needs a planted -wal file in the account's own folder, which only"
    " the user can enter; the fixed version needs rusqlite 0.41, matrix-sdk-sqlite 0.19 wants 0.40",
}


def ask(path, body=None):
    request = urllib.request.Request(
        f"{API}/{path}",
        json.dumps(body).encode() if body is not None else None,
        {"Content-Type": "application/json", "User-Agent": "cargo-lock-check"},
    )
    return json.load(urllib.request.urlopen(request, timeout=60))


def advisories(crates):
    """The ids of the advisories for each crate, in the crates' order."""
    ids = []
    for start in range(0, len(crates), 500):
        queries = [
            {"package": {"name": name, "ecosystem": "crates.io"}, "version": version}
            for name, version in crates[start : start + 500]
        ]
        results = ask("querybatch", {"queries": queries}).get("results", [])
        # One result per query, in order: with fewer, crates would go
        # unchecked without a word.
        if len(results) != len(queries):
            sys.exit(f"osv.dev answered {len(results)} of {len(queries)} queries: nothing can be said.")
        for query, result in zip(queries, results):
            found = [advisory["id"] for advisory in result.get("vulns", [])]
            # A long list comes in pages.
            while result.get("next_page_token"):
                more = ask("querybatch", {"queries": [dict(query, page_token=result["next_page_token"])]})
                if len(more.get("results", [])) != 1:
                    sys.exit(f"osv.dev did not answer for the rest of {query['package']['name']}'s advisories.")
                result = more["results"][0]
                found += [advisory["id"] for advisory in result.get("vulns", [])]
            ids.append(found)
    return ids


def informational(advisory, name):
    """What kind of notice an advisory is for this crate ("unmaintained",
    "unsound", "notice"), or None for a vulnerability: RustSec's own
    field, as osv.dev passes it on with each affected package."""
    for affected in advisory.get("affected", []):
        if affected.get("package", {}).get("name") == name:
            kind = (affected.get("database_specific") or {}).get("informational")
            if kind:
                return kind
    return None


def main():
    lock = open("Cargo.lock", encoding="utf-8").read()
    crates = []
    for block in lock.split("[[package]]")[1:]:
        source = re.search(r'source = "(.*?)"', block)
        if source and "crates.io" in source.group(1):
            crates.append(
                (re.search(r'name = "(.*?)"', block).group(1), re.search(r'version = "(.*?)"', block).group(1))
            )
    found, unmaintained, accepted = [], [], []
    seen = set()
    for (name, version), ids in zip(crates, advisories(crates)):
        for advisory_id in ids:
            advisory = ask(f"vulns/{urllib.parse.quote(advisory_id, safe='')}")
            line = f"{name} {version}: {advisory_id} {advisory.get('summary', '')}"
            # The same flaw can come under several names (RUSTSEC, GHSA, CVE).
            names = {advisory_id, *advisory.get("aliases", [])}
            reasons = [(key, why) for key, why in ACCEPTED.items() if key[0] == name and key[1] in names]
            kind = informational(advisory, name)
            if reasons:
                seen.update(key for key, _ in reasons)
                accepted.append(f"{line}\n      {reasons[0][1]}")
            elif kind == "unmaintained":
                unmaintained.append(line)
            else:
                found.append(f"{line} ({kind})" if kind else line)
    print(f"{len(crates)} crates checked.")
    for line in found:
        print("  " + line)
    for title, lines in (("Unmaintained", unmaintained), ("Accepted (see ACCEPTED in dev/audit.py)", accepted)):
        if lines:
            print(title + ":")
            for line in lines:
                print("  " + line)
    for name, advisory_id in sorted(set(ACCEPTED) - seen):
        print(f"No longer found, take it out of ACCEPTED: {name} {advisory_id}")
    strict = "--strict" in sys.argv
    return 1 if found or (strict and unmaintained) else 0


if __name__ == "__main__":
    sys.exit(main())

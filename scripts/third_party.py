"""Writes THIRD-PARTY-LICENSES.txt: the licences of every crate in the Windows build of yubi.

Walks the normal (non-dev, non-build) dependencies for x86_64-pc-windows-msvc from
`cargo metadata`, picks the permissive option of each licence expression, and copies the
licence texts the crates ship. Identical texts are printed once, with the crates that use them.
Exits non-zero if a crate has no licence text it can find.
"""

import hashlib
import json
import os
import re
import subprocess
import sys

TARGET = "x86_64-pc-windows-msvc"
# Preference when an expression offers a choice; GPL / LGPL are never picked.
PREFER = [
    "MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "Zlib", "BSD-2-Clause",
    "BSD-3-Clause", "ISC", "0BSD", "Unlicense", "BSL-1.0", "Unicode-3.0",
    "OFL-1.1", "Ubuntu-font-1.0",
]
# Filename hints for each licence id.
HINTS = {
    "MIT": ["MIT"], "Apache-2.0": ["APACHE"], "Apache-2.0 WITH LLVM-exception": ["APACHE"],
    "Zlib": ["ZLIB"], "BSD-2-Clause": ["BSD"], "BSD-3-Clause": ["BSD"], "ISC": ["ISC"],
    "0BSD": ["0BSD", "BSD"], "Unlicense": ["UNLICENSE"], "BSL-1.0": ["BSL", "BOOST"],
    "Unicode-3.0": ["UNICODE"], "OFL-1.1": ["OFL"], "Ubuntu-font-1.0": ["UFL", "UBUNTU"],
}
# Content markers, used when the filename says nothing.
MARKS = {
    "MIT": "Permission is hereby granted, free of charge",
    "Apache-2.0": "Apache License", "Apache-2.0 WITH LLVM-exception": "Apache License",
    "Zlib": "This software is provided 'as-is'", "BSD-2-Clause": "Redistribution and use",
    "BSD-3-Clause": "Redistribution and use", "ISC": "Permission to use, copy, modify",
    "0BSD": "Permission to use, copy, modify", "Unlicense": "unencumbered software",
    "BSL-1.0": "Boost Software License", "Unicode-3.0": "UNICODE LICENSE",
    "OFL-1.1": "SIL OPEN FONT LICENSE", "Ubuntu-font-1.0": "UBUNTU FONT LICENCE",
}


def choose(expr):
    """The licence ids yubi takes a crate under (every AND part, the preferred OR option)."""
    expr = expr.replace("/", " OR ")
    parts = [p.strip().strip("()").strip() for p in re.split(r"\)\s+AND\s+|\s+AND\s+\(|\s+AND\s+", expr)]
    out = []
    for part in parts:
        options = [o.strip().strip("()").strip() for o in re.split(r"\s+OR\s+", part)]
        picked = next((p for p in PREFER if p in options), None)
        if picked is None:
            sys.exit(f"no permissive option in licence '{expr}'")
        out.append(picked)
    return out


def licence_files(pkg):
    d = os.path.dirname(pkg["manifest_path"])
    files = []
    for root, dirs, names in os.walk(d):
        rel = os.path.relpath(root, d)
        parts = [] if rel == "." else rel.split(os.sep)
        if len(parts) > 1 or any(x in parts for x in ("target", ".git", "tests", "examples")):
            dirs[:] = []
            continue
        for n in names:
            u = n.upper()
            if u.startswith(("LICENSE", "LICENCE", "COPYING", "UNLICENSE")) or u in ("OFL.TXT", "UFL.TXT") or "LICENSE" in u:
                files.append(os.path.join(root, n))
    if pkg.get("license_file"):
        files.append(os.path.join(d, pkg["license_file"]))
    return sorted(set(files))


def text_for(lic, files):
    def read(p):
        with open(p, encoding="utf-8", errors="replace") as f:
            return f.read()
    named = [p for p in files if any(h in os.path.basename(p).upper() for h in HINTS[lic])]
    def fits(t):
        t = t.lower()
        # The Boost licence opens like MIT's: never take it for MIT.
        return MARKS[lic].lower() in t and not (lic == "MIT" and "boost software license" in t)
    for p in named + files:
        t = read(p)
        if fits(t):
            return t
    return None


def standard_text(lic, pkg):
    """The licence's standard text, for a crate that declares it but ships no file. MIT gets the
    crate's authors as the copyright holders."""
    here = os.path.dirname(os.path.abspath(__file__))
    if lic == "MIT":
        holders = ", ".join(pkg.get("authors") or []) or f'the {pkg["name"]} authors'
        with open(os.path.join(here, "licenses", "MIT.txt"), encoding="utf-8") as f:
            return f.read().replace("{holders}", holders)
    path = {
        "Apache-2.0": os.path.join(here, "..", "LICENSE-APACHE"),
        "BSL-1.0": os.path.join(here, "licenses", "BSL-1.0.txt"),
    }.get(lic)
    if path is None:
        return None
    with open(path, encoding="utf-8") as f:
        return f.read()


def main():
    meta = json.loads(subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", TARGET],
        check=True, capture_output=True, text=True).stdout)
    pkgs = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    root = meta["resolve"]["root"]
    seen, stack = set(), [root]
    while stack:
        i = stack.pop()
        for dep in nodes[i]["deps"]:
            if any(k["kind"] is None for k in dep["dep_kinds"]) and dep["pkg"] not in seen:
                seen.add(dep["pkg"])
                stack.append(dep["pkg"])

    groups = {}  # hash -> (licence, text, [crates])
    missing = []
    fallback = []  # crates that ship no licence file: the standard text is used
    for pid in sorted(seen, key=lambda i: (pkgs[i]["name"], pkgs[i]["version"])):
        p = pkgs[pid]
        name = f'{p["name"]} {p["version"]}'
        if not p.get("license"):
            missing.append(f"{name}: no licence in Cargo.toml")
            continue
        files = licence_files(p)
        for lic in choose(p["license"]):
            t = text_for(lic, files)
            if t is None:
                t = standard_text(lic, p)
                if t is not None:
                    fallback.append(f"{name}: {lic}")
            if t is None:
                missing.append(f"{name}: no {lic} text")
                continue
            t = t.replace("\r\n", "\n").strip() + "\n"
            h = hashlib.sha256(t.encode()).hexdigest()
            groups.setdefault(h, (lic, t, []))[2].append(name)
    if missing:
        sys.exit("missing licence texts:\n  " + "\n  ".join(missing))

    out = [
        "Third-party licences for yubi",
        "=============================",
        "",
        f"yubi is built from the following {len(seen)} crates. Each is used under the licence",
        "shown with it (for crates offering a choice, the permissive option listed here).",
        "",
    ]
    for lic, text, crates in sorted(groups.values(), key=lambda g: (PREFER.index(g[0]), g[2][0])):
        out += ["-" * 72, f"{lic}", "Used by: " + ", ".join(crates), "-" * 72, "", text]
    with open("THIRD-PARTY-LICENSES.txt", "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(out))
    for f_ in fallback:
        print(f"  standard text (no file in the crate): {f_}")
    print(f"wrote THIRD-PARTY-LICENSES.txt: {len(seen)} crates, {len(groups)} texts")


if __name__ == "__main__":
    main()

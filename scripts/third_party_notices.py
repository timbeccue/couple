#!/usr/bin/env python3
"""Generate the third-party license notices Couple ships (docs/LICENSING.md §3).

Entry point: scripts/third-party-notices.sh (which runs this with `python3 -I`).

Covers the three dependency trees that end up inside a distributed build:

  rust   every crate reachable over NORMAL dependency edges from the workspace
         members (couple-core, couple-ffi, couple-cli, couple-node). Dev and
         build dependencies (proptest, cbindgen, bindgen, napi-build, ...) are
         not shipped and are not listed. All target platforms are included —
         the Electron shell ships on macOS, Windows and Linux — so a macOS
         bundle over-lists a few Windows/Linux-only crates; harmless.
  npm    the Electron shell's production dependencies (`npm ls --omit=dev`)
         plus the Electron runtime itself. electron-builder separately places
         LICENSE.electron.txt and LICENSES.chromium.html (Chromium, Node and
         their third-party components) at the resources root.
  swift  mac/Package.swift must declare no external packages (it links only
         Apple frameworks). The script fails if that changes, so the tree is
         handled deliberately rather than silently skipped.

Output (committed, deterministic — no timestamps):
  third-party/components.json   the component list the Acknowledgements views
                                render (name, version, license, tree, source)
  third-party/rust-crates.txt   per-component notices, one section each
  third-party/npm-packages.txt
  third-party/swift-packages.txt

Section format (parsed by both shells' Acknowledgements views — keep stable):
  ==== <name> <version>
  License: <declared SPDX expression>
  Used under: <the alternative chosen for an OR expression>   (omitted if same)
  Source: <registry URL>[ · <repository>]
  ----
  [<file name>]
  <license text>

Every license expression is evaluated against ALLOWED below. An unknown,
missing, or copyleft license (GPL, LGPL, MPL, ...) anywhere in a shipped tree
fails the run: adding such a dependency is a licensing decision, not a build
detail, and must be recorded in docs/LICENSING.md first. Apache-2.0 §4(d):
any NOTICE file a component ships is carried into its section verbatim.

Crates that ship no license text at all get the canonical text for the
license chosen (scripts/license-texts/<id>.txt), with the crate's authors on
the copyright line for MIT.

Usage: third_party_notices.py [--check]
  --check  regenerate into a temp dir and fail if it differs from third-party/
"""

from __future__ import annotations

import filecmp
import json
import os
import re
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT_DIR = os.path.join(ROOT, "third-party")
TEXTS_DIR = os.path.join(ROOT, "scripts", "license-texts")
ELECTRON_DIR = os.path.join(ROOT, "electron")

# Licenses a proprietary binary may include with attribution only. Anything
# else — including weak copyleft — is rejected until docs/LICENSING.md says
# otherwise.
ALLOWED = {
    "MIT",
    "MIT-0",
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Zlib",
    "0BSD",
    "Unlicense",
    "CC0-1.0",
    "BSL-1.0",
    "Unicode-3.0",
    "Unicode-DFS-2016",
}

# For `A OR B`, the alternative Couple uses: earliest in this list wins.
PREFERENCE = [
    "MIT",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "Zlib",
    "MIT-0",
    "0BSD",
    "BSL-1.0",
    "Unlicense",
    "CC0-1.0",
    "Unicode-3.0",
    "Unicode-DFS-2016",
    "Apache-2.0 WITH LLVM-exception",
]

# Keyword in a license file name -> the license it carries. A file with none
# of these (LICENSE, COPYING, ...) is "generic" and used for any license.
FILE_KEYWORDS = [
    ("APACHE", "Apache-2.0"),
    ("MIT", "MIT"),
    ("BSD", "BSD"),
    ("ZLIB", "Zlib"),
    ("ISC", "ISC"),
    ("UNLICENSE", "Unlicense"),
    ("CC0", "CC0-1.0"),
    ("BOOST", "BSL-1.0"),
    ("UNICODE", "Unicode-3.0"),
]

LICENSE_FILE_RE = re.compile(r"^(LICEN[CS]E|COPYING|UNLICENSE)", re.IGNORECASE)
NOTICE_FILE_RE = re.compile(r"^NOTICE", re.IGNORECASE)

SECTION_RULE = "=" * 72


class LicenseError(Exception):
    pass


# --- SPDX expression evaluation -------------------------------------------


def tokenize(expr: str) -> list[str]:
    # Legacy Cargo "MIT/Apache-2.0" means OR.
    expr = expr.replace("/", " OR ")
    return re.findall(r"\(|\)|[^\s()]+", expr)


def parse(tokens: list[str]) -> list[frozenset[str]]:
    """Returns the expression's satisfying options: each option is the set of
    licenses that must all be complied with (AND); the list is the OR."""
    pos = 0

    def atom() -> list[frozenset[str]]:
        nonlocal pos
        if tokens[pos] == "(":
            pos += 1
            inner = expr()
            if pos >= len(tokens) or tokens[pos] != ")":
                raise LicenseError("unbalanced parentheses")
            pos += 1
            return inner
        ident = tokens[pos]
        pos += 1
        if pos + 1 < len(tokens) and tokens[pos].upper() == "WITH":
            ident = f"{ident} WITH {tokens[pos + 1]}"
            pos += 2
        return [frozenset([ident])]

    def term() -> list[frozenset[str]]:  # AND binds tighter than OR
        nonlocal pos
        left = atom()
        while pos < len(tokens) and tokens[pos].upper() == "AND":
            pos += 1
            right = atom()
            left = [a | b for a in left for b in right]
        return left

    def expr() -> list[frozenset[str]]:
        nonlocal pos
        options = term()
        while pos < len(tokens) and tokens[pos].upper() == "OR":
            pos += 1
            options = options + term()
        return options

    result = expr()
    if pos != len(tokens):
        raise LicenseError("trailing tokens")
    return result


def choose(declared: str | None, what: str) -> list[str]:
    """The licenses Couple complies with for a declared expression, or raise."""
    if not declared:
        raise LicenseError(f"{what}: no license declared")
    try:
        options = parse(tokenize(declared))
    except (LicenseError, IndexError) as e:
        raise LicenseError(f"{what}: cannot parse license expression {declared!r} ({e})")
    ok = [o for o in options if all(lic in ALLOWED for lic in o)]
    if not ok:
        raise LicenseError(
            f"{what}: license {declared!r} is not in the allowed set "
            "(unknown or copyleft) — record a decision in docs/LICENSING.md "
            "before adding it to ALLOWED"
        )

    def rank(option: frozenset[str]) -> tuple[int, int]:
        return (sum(PREFERENCE.index(lic) for lic in option), len(option))

    best = min(ok, key=rank)
    return sorted(best, key=PREFERENCE.index)


# --- license text collection ---------------------------------------------


def canonical_text(lic: str, holder: str, what: str) -> str:
    path = os.path.join(TEXTS_DIR, f"{lic}.txt")
    if not os.path.isfile(path):
        raise LicenseError(
            f"{what}: ships no license text and scripts/license-texts/{lic}.txt "
            "does not exist — add the canonical text"
        )
    with open(path, encoding="utf-8") as f:
        text = f.read()
    if lic == "MIT":
        text = f"Copyright (c) {holder}\n\n{text}"
    return text


def file_license(name: str) -> str | None:
    upper = name.upper()
    for keyword, lic in FILE_KEYWORDS:
        if keyword in upper:
            return lic
    return None


def collect_texts(directory: str, chosen: list[str], holder: str, what: str) -> list[tuple[str, str]]:
    """(label, text) pairs: the license files covering each chosen license,
    then any NOTICE files, verbatim."""
    try:
        entries = sorted(os.listdir(directory))
    except OSError:
        entries = []
    license_files = [e for e in entries if LICENSE_FILE_RE.match(e) and os.path.isfile(os.path.join(directory, e))]
    notice_files = [e for e in entries if NOTICE_FILE_RE.match(e) and os.path.isfile(os.path.join(directory, e))]
    generic = [e for e in license_files if file_license(e) is None]

    out: list[tuple[str, str]] = []
    used: set[str] = set()
    for lic in chosen:
        family = lic.split("-")[0] if lic.startswith("BSD") else lic
        specific = [e for e in license_files if file_license(e) in (lic, family)]
        picked = specific or generic
        if not picked:
            out.append((f"{lic} (canonical text; the component ships none)", canonical_text(lic, holder, what)))
            continue
        for e in picked:
            if e in used:
                continue
            used.add(e)
            with open(os.path.join(directory, e), encoding="utf-8", errors="replace") as f:
                out.append((e, f.read()))
    for e in notice_files:
        with open(os.path.join(directory, e), encoding="utf-8", errors="replace") as f:
            out.append((e, f.read()))
    return out


# --- output ---------------------------------------------------------------


def section(name: str, version: str, declared: str, chosen: list[str], source: str, texts: list[tuple[str, str]]) -> str:
    lines = [f"==== {name} {version}", f"License: {declared}"]
    used = " AND ".join(chosen)
    if used != declared:
        lines.append(f"Used under: {used}")
    lines.append(f"Source: {source}")
    lines.append("----")
    for label, text in texts:
        lines.append(f"[{label}]")
        lines.append(text.rstrip("\n"))
        lines.append("")
    return "\n".join(lines) + "\n"


def header(title: str, body: str) -> str:
    return f"{title}\n{SECTION_RULE}\n{body.rstrip()}\n\nGenerated by scripts/third-party-notices.sh. Do not edit by hand.\n\n"


# --- rust -----------------------------------------------------------------


def rust_components() -> tuple[list[dict], str]:
    meta = json.loads(
        subprocess.run(
            ["cargo", "metadata", "--format-version", "1", "--locked"],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
        ).stdout
    )
    members = set(meta["workspace_members"])
    packages = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}

    seen: set[str] = set()
    stack = list(members)
    while stack:
        pid = stack.pop()
        if pid in seen:
            continue
        seen.add(pid)
        for dep in nodes[pid]["deps"]:
            if any(k["kind"] is None for k in dep["dep_kinds"]):  # normal deps only
                stack.append(dep["pkg"])

    shipped = sorted((packages[p] for p in seen if p not in members), key=lambda p: (p["name"], p["version"]))
    components = []
    sections = []
    for p in shipped:
        what = f"crate {p['name']} {p['version']}"
        declared = p.get("license")
        if not declared and p.get("license_file"):
            raise LicenseError(f"{what}: custom license file {p['license_file']!r} — review it and add a clarification")
        chosen = choose(declared, what)
        assert declared is not None
        authors = ", ".join(re.sub(r"\s*<[^>]*>", "", a).strip() for a in p.get("authors") or []) or f"the {p['name']} authors"
        source = f"https://crates.io/crates/{p['name']}"
        if p.get("repository"):
            source += f" · {p['repository']}"
        texts = collect_texts(os.path.dirname(p["manifest_path"]), chosen, authors, what)
        components.append(
            {"name": p["name"], "version": p["version"], "license": declared, "usedUnder": " AND ".join(chosen), "tree": "rust", "source": source}
        )
        sections.append(section(p["name"], p["version"], declared, chosen, source, texts))

    roots = ", ".join(sorted(packages[m]["name"] for m in members))
    text = header(
        "Third-party notices — Rust crates",
        f"Crates compiled into Couple's engine, FFI library, CLI and Electron\n"
        f"binding ({roots}), with the license each is\n"
        f"used under. Dev and build dependencies are not shipped and not listed.\n"
        f"Crates for every target platform are included, so a build for one\n"
        f"platform lists a few crates that only compile on another.",
    )
    return components, text + "".join(sections)


# --- npm ------------------------------------------------------------------


def npm_components() -> tuple[list[dict], str]:
    node_modules = os.path.join(ELECTRON_DIR, "node_modules")
    if not os.path.isdir(node_modules):
        raise LicenseError("electron/node_modules is missing — run `npm ci` in electron/ first")
    tree = json.loads(
        subprocess.run(
            ["npm", "ls", "--omit=dev", "--all", "--json", "--long"],
            cwd=ELECTRON_DIR,
            check=True,
            capture_output=True,
            text=True,
        ).stdout
    )
    found: dict[tuple[str, str], str] = {}

    def walk(node: dict) -> None:
        for name, dep in (node.get("dependencies") or {}).items():
            version = dep.get("version")
            if not version:
                raise LicenseError(f"npm package {name}: not installed (run `npm ci` in electron/)")
            path = dep.get("path") or os.path.join(node_modules, *name.split("/"))
            found.setdefault((name, version), path)
            walk(dep)

    walk(tree)
    # The Electron runtime is a devDependency (it is the build's target, not a
    # library the renderer imports) but it IS what ships.
    with open(os.path.join(node_modules, "electron", "package.json"), encoding="utf-8") as f:
        electron_version = json.load(f)["version"]
    found[("electron", electron_version)] = os.path.join(node_modules, "electron")

    components = []
    sections = []
    for (name, version), path in sorted(found.items()):
        what = f"npm package {name} {version}"
        with open(os.path.join(path, "package.json"), encoding="utf-8") as f:
            pkg = json.load(f)
        declared = pkg.get("license")
        if isinstance(declared, dict):
            declared = declared.get("type")
        chosen = choose(declared, what)
        assert declared is not None
        author = pkg.get("author")
        if isinstance(author, dict):
            author = author.get("name")
        holder = re.sub(r"\s*<[^>]*>", "", author or "").strip() or f"the {name} authors"
        source = f"https://www.npmjs.com/package/{name}"
        repo = pkg.get("repository")
        if isinstance(repo, dict):
            repo = repo.get("url")
        if isinstance(repo, str) and repo:
            source += f" · {repo}"
        texts = collect_texts(path, chosen, holder, what)
        components.append({"name": name, "version": version, "license": declared, "usedUnder": " AND ".join(chosen), "tree": "npm", "source": source})
        sections.append(section(name, version, declared, chosen, source, texts))

    text = header(
        "Third-party notices — npm packages",
        "Packages bundled into the Electron shell's renderer, plus the Electron\n"
        "runtime the shell runs in. Chromium, Node.js and their own third-party\n"
        "components are covered by LICENSE.electron.txt and LICENSES.chromium.html,\n"
        "which electron-builder places at the root of the application's resources\n"
        "folder.",
    )
    return components, text + "".join(sections)


# --- swift ----------------------------------------------------------------


def swift_components() -> tuple[list[dict], str]:
    manifest = os.path.join(ROOT, "mac", "Package.swift")
    with open(manifest, encoding="utf-8") as f:
        src = f.read()
    if ".package(" in src or os.path.exists(os.path.join(ROOT, "mac", "Package.resolved")):
        raise LicenseError(
            "mac/Package.swift declares external packages; this script does not read "
            "Package.resolved yet — extend swift_components() before shipping them"
        )
    text = header(
        "Third-party notices — Swift packages",
        "The macOS app (mac/Package.swift) depends on no third-party Swift\n"
        "packages. It links Couple's own Rust engine (notices in rust-crates.txt)\n"
        "and Apple system frameworks, which are covered by the macOS software\n"
        "license agreement.",
    )
    return [], text


# --- main -----------------------------------------------------------------


def generate(out_dir: str, quiet: bool = False) -> None:
    rust, rust_text = rust_components()
    npm, npm_text = npm_components()
    swift, swift_text = swift_components()
    os.makedirs(out_dir, exist_ok=True)
    manifest = {
        "note": "Generated by scripts/third-party-notices.sh; parsed by the Acknowledgements views. Do not edit.",
        "trees": [
            {"id": "rust", "title": "Rust crates", "file": "rust-crates.txt"},
            {"id": "npm", "title": "npm packages", "file": "npm-packages.txt"},
            {"id": "swift", "title": "Swift packages", "file": "swift-packages.txt"},
        ],
        "components": rust + npm + swift,
    }
    with open(os.path.join(out_dir, "components.json"), "w", encoding="utf-8") as f:
        json.dump(manifest, f, indent=2)
        f.write("\n")
    for name, text in (("rust-crates.txt", rust_text), ("npm-packages.txt", npm_text), ("swift-packages.txt", swift_text)):
        with open(os.path.join(out_dir, name), "w", encoding="utf-8") as f:
            f.write(text)
    if not quiet:
        print(f"third-party notices: {len(rust)} Rust crates, {len(npm)} npm packages, {len(swift)} Swift packages -> third-party/")


def main(argv: list[str]) -> int:
    check = "--check" in argv
    try:
        if not check:
            generate(OUT_DIR)
            return 0
        with tempfile.TemporaryDirectory() as tmp:
            generate(tmp, quiet=True)
            names = ["components.json", "rust-crates.txt", "npm-packages.txt", "swift-packages.txt"]
            stale = [n for n in names if not filecmp.cmp(os.path.join(tmp, n), os.path.join(OUT_DIR, n), shallow=False)]
            if stale:
                print(f"third-party/ is stale ({', '.join(stale)}); run scripts/third-party-notices.sh and commit", file=sys.stderr)
                return 1
            print("third-party/ is up to date")
            return 0
    except LicenseError as e:
        print(f"error: {e}", file=sys.stderr)
        return 2
    except subprocess.CalledProcessError as e:
        print(f"error: {' '.join(e.cmd)} failed:\n{e.stderr}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

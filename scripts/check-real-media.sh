#!/bin/sh
# Audit the gitignored REAL media this repo's local-only suites depend on
# (SPEC §10.1: media MAY be gitignored, ground truth MUST be committed).
# The corpus scorer counts absent media as "unscannable" and moves on — honest,
# but silent: a machine can lose example_data/ and every real-data gate (the
# §9.4 freeze checks, `dev.sh example`, the calibration/holdout splits) quietly
# stops measuring anything. This script makes that state loud.
#
# Checks:
#   1. example_data/ basics (the schumann pair `dev.sh example` + SPEC §1 use).
#   2. Every non-synthetic case in fixtures/matcher-corpus/*/manifest.json
#      (cases carrying a `gen` block regenerate their media and need nothing).
#
# Exit 0 = all present; exit 1 = something missing (listed with its manifest).
# Restore by copying the media back to the printed paths — filenames and
# ground truth are already committed; only the bytes are local.
set -e
cd "$(dirname "$0")/.."

python3 - <<'EOF'
import json, sys
from pathlib import Path

root = Path.cwd()
missing, present = {}, set()  # path -> [labels]; a file shared by cases lists once

def check(path: Path, label: str):
    if path.exists():
        present.add(path)
    else:
        missing.setdefault(path, []).append(label)

# 1. The example pair (dev.sh example / SPEC §1 ground truth).
for f in ("schumann-video.mp4", "schumann-audio.wav"):
    check(root / "example_data" / f, "example_data (SPEC §1 / dev.sh example)")

# 2. Real corpus cases across every committed manifest.
for manifest in sorted(root.glob("fixtures/matcher-corpus/*/manifest.json")):
    m = json.loads(manifest.read_text())
    media_root = root / m.get("mediaRoot", "")
    label = f"{manifest.parent.name} split ({manifest.relative_to(root)})"
    for case in m.get("cases", []):
        if case.get("gen"):
            continue  # synthetic: regenerated deterministically, no media needed
        for side in ("a", "b"):
            if case.get(side):
                check(media_root / case[side], f"{label} case {case['id']}")

print(f"present: {len(present)} file(s)")
if not missing:
    print("all real media accounted for")
    sys.exit(0)

print(f"MISSING: {len(missing)} file(s) — the suites below silently degrade to 'unscannable':\n")
for path in sorted(missing):
    print(f"  {path.relative_to(root)}")
    for label in dict.fromkeys(missing[path]):  # unique, insertion-ordered
        print(f"      needed by: {label}")
print("\nRestore the media bytes to these exact paths (they are gitignored on purpose;")
print("ground truth/manifests are already committed, only the recordings are local).")
sys.exit(1)
EOF

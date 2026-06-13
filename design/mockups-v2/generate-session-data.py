#!/usr/bin/env python3
"""Bake the real multifile session into session-data.json for the v2 mockup.

Method (the v1 mockup lineage, SPEC-V2 §8 Phase 8): everything the mockup
shows for real rows comes from the real engine via the CLI —
  couple pair   → rows, states, chips, candidates, margins, triangles
  couple sync   → per-row offset/drift/fit windows (sparkline + loupe Δs)
  couple peaks  → overview lanes (1600 cols) + per-anchor loupe windows
  ffmpeg        → real video thumbnails / hero posters

Run from the repo root:  python3 design/mockups-v2/generate-session-data.py
"""
import json, subprocess, sys, os
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SESS = ROOT / "example_data" / "multifile-session"
OUT = Path(__file__).resolve().parent
ASSETS = OUT / "assets"
COUPLE = ROOT / "target" / "release" / "couple"

# pairs that need full sync + loupe windows: the three selected rows plus
# bach 170's top-2 candidates (so picking either in the mockup shows real data)
SYNC_PAIRS = [
    ("bach 169.mov", "Gigue Recording.WAV"),
    ("bach 170.mov", "ZOOM0031.WAV"),
    ("bach 170.mov", "ZOOM0027.WAV"),
    ("bach 171.mov", "Prelude Recording.WAV"),
    ("bach 172.mov", "ZOOM0033.WAV"),
]
LOUPE_HALF_S = 0.35   # dump ±350 ms around each anchor (500 ms window + nudge slack)
LOUPE_COLS = 900
OVERVIEW_COLS = 1600


def couple(*args):
    r = subprocess.run([str(COUPLE), "--json", *args], capture_output=True, text=True)
    if r.returncode != 0:
        sys.exit(f"couple {' '.join(args)} failed:\n{r.stderr[-2000:]}")
    return json.loads(r.stdout)


def peaks(path, t0=None, t1=None, cols=OVERVIEW_COLS):
    args = ["peaks", str(path), "--columns", str(cols)]
    if t0 is not None:
        args += ["--t0", f"{max(0.0, t0):.6f}"]
    if t1 is not None:
        args += ["--t1", f"{t1:.6f}"]
    d = couple(*args)
    return {"t0": d["colT0"], "dt": d["colDt"], "mins": d["mins"], "maxs": d["maxs"]}


def main():
    ASSETS.mkdir(exist_ok=True)

    print("pair…")
    pair = couple("pair", str(SESS))

    durs = {v["file"]: v["durationS"] for v in pair["videos"]}
    durs |= {a["file"]: a["durationS"] for a in pair["audios"]}

    print("overviews…")
    overviews = {name: peaks(SESS / name) for name in sorted(durs)}

    syncs, loupes = {}, {}
    for vid, aud in SYNC_PAIRS:
        key = f"{vid}|{aud}"
        print("sync", key)
        s = couple("sync", str(SESS / vid), str(SESS / aud))
        syncs[key] = {k: s[k] for k in
                      ("offsetMs", "driftPpm", "driftMeasurable", "fitRmsMs",
                       "endToEndDriftMs", "fineNcc", "suggestedDriftMode", "windows")}
        acc = [w for w in s["windows"] if w["accepted"]]
        if len(acc) < 3:
            acc = sorted(s["windows"], key=lambda w: -w["ncc"])[:3]
        span0, span1 = acc[0]["t"], acc[-1]["t"]
        anchors = []
        for frac in (0.15, 0.5, 0.85):
            want = span0 + (span1 - span0) * frac
            anchors.append(min(acc, key=lambda w: abs(w["t"] - want)))
        # dedupe while keeping order
        seen, picked = set(), []
        for w in anchors:
            if w["t"] not in seen:
                seen.add(w["t"]); picked.append(w)
        off, ppm = s["offsetMs"], s["driftPpm"]
        entry = []
        for w in picked:
            t = w["t"]
            u = t * (1 + ppm * 1e-6) + off / 1000.0   # §2.1 mapping, video → audio time
            entry.append({
                "t": t, "windowLagMs": w["lagMs"], "ncc": w["ncc"],
                "cam": peaks(SESS / vid, t - LOUPE_HALF_S, t + LOUPE_HALF_S, LOUPE_COLS),
                "ext": peaks(SESS / aud, u - LOUPE_HALF_S, u + LOUPE_HALF_S, LOUPE_COLS),
            })
        loupes[key] = entry

    print("thumbnails…")
    thumbs = {}
    for v in pair["videos"]:
        name, dur = v["file"], v["durationS"]
        stem = name.replace(" ", "-").replace(".mov", "")
        for label, width in (("thumb", 320), ("hero", 1024)):
            out = ASSETS / f"{label}-{stem}.jpg"
            subprocess.run(
                ["ffmpeg", "-y", "-ss", f"{dur * 0.4:.2f}", "-i", str(SESS / name),
                 "-frames:v", "1", "-vf", f"scale={width}:-2", "-q:v", "4", str(out)],
                capture_output=True, check=True)
        thumbs[name] = {"thumb": f"assets/thumb-{stem}.jpg", "hero": f"assets/hero-{stem}.jpg"}

    # tray hints: best z over the matrix for audio files no row selects
    bestz = {}
    for c in pair["matrix"]:
        bestz[c["audio"]] = max(bestz.get(c["audio"], 0.0), c["z"])
    tray = {a: round(bestz.get(a, 0.0), 1) for a in pair["unusedAudio"]}

    data = {
        "generated": "couple pair/sync/peaks on example_data/multifile-session",
        "constants": pair["constants"],
        "rows": pair["rows"],
        "videos": pair["videos"],
        "audios": pair["audios"],
        "unusedAudio": pair["unusedAudio"],
        "trayBestZ": tray,
        "durations": durs,
        "overviews": overviews,
        "syncs": syncs,
        "loupes": loupes,
        "thumbs": thumbs,
    }
    out = OUT / "session-data.json"
    out.write_text(json.dumps(data))
    print(f"wrote {out} ({out.stat().st_size//1024} KB)")


if __name__ == "__main__":
    main()

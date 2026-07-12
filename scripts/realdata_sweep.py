#!/usr/bin/env python3
"""realdata_sweep.py — aggregator behind scripts/realdata-sweep.sh
(F4; PLAN-REALDATA-CALIBRATION.md).

Drives the release `couple` CLI over every discovered session directory and
writes a calibration report comparing measured distributions against the
CURRENT [TUNE] verdict lines. READ-ONLY by design: this file must never write
to any params/threshold source — it reports, the owner decides (SPEC §0).

The SYNC_LINES mirror below is DISPLAY-ONLY (source of truth:
core/src/sync.rs `SyncParams::default()`); resolver constants are read LIVE
from `couple pair --json`'s `constants` block.
"""
import argparse
import json
import math
import os
import statistics
import subprocess
import sys
from datetime import datetime, timezone

MEDIA_EXT = {".mp4", ".mov", ".m4v", ".3gp", ".mkv",
             ".m4a", ".wav", ".aif", ".aiff", ".flac", ".mp3"}
SKIP_DIRS = {"peaks", "handsync"}
MAX_DEPTH = 3

# Sync-verdict lines (SPEC §5.6 / Appendix A), parsed LIVE from the source of
# truth — core/src/sync.rs `SyncParams::default()` — so report annotations can
# never go stale against a re-freeze. Display-only; this script never writes
# any params source.
def sync_lines_from_source():
    src = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                       "..", "core", "src", "sync.rs")
    fields = {"synced_max_rms_ms": "syncedMaxRmsMs",
              "synced_min_windows": "syncedMinWindows",
              "synced_fine_ncc": "syncedFineNcc",
              "suggest_resample_ms": "suggestResampleMs"}
    import re
    text = open(src, encoding="utf-8").read()
    lines = {}
    for rust, key in fields.items():
        m = re.search(rf"^\s*{rust}:\s*([0-9]+(?:\.[0-9]+)?)\s*,\s*$", text, re.M)
        if not m:
            sys.exit(f"realdata-sweep: cannot parse `{rust}` from {src} — "
                     "SyncParams::default() moved or was renamed; fix this parser.")
        v = m.group(1)
        lines[key] = int(v) if "." not in v else float(v)
    return lines


SYNC_LINES = sync_lines_from_source()
GOOD_STATES = {"confident", "partial", "split", "twoSources"}
NEGATIVE_CLASSES = {"nomatch", "insufficientoverlap"}


def log(msg):
    print(f"realdata-sweep: {msg}", file=sys.stderr)


def run_json(cmd):
    """Run a couple CLI command; JSON is printed even on nonzero exits
    (e.g. `couple sync` exits 5/6 for failed status but still emits SyncResult)."""
    p = subprocess.run(cmd, capture_output=True, text=True)
    out = None
    if p.stdout.strip():
        try:
            out = json.loads(p.stdout)
        except json.JSONDecodeError:
            out = None
    tail = " | ".join(p.stderr.strip().splitlines()[-4:])
    return out, p.returncode, tail


def find_sessions(root):
    root = os.path.abspath(root)
    sessions = []
    for dirpath, dirnames, filenames in os.walk(root):
        rel = os.path.relpath(dirpath, root)
        depth = 0 if rel == "." else rel.count(os.sep) + 1
        dirnames[:] = sorted(d for d in dirnames
                             if depth < MAX_DEPTH and not d.startswith(".")
                             and d not in SKIP_DIRS)
        media = [f for f in filenames if not f.startswith(".")
                 and os.path.splitext(f)[1].lower() in MEDIA_EXT]
        if len(media) >= 2:
            sessions.append(dirpath)
    return sorted(sessions)


def pctl(vals, p):  # nearest-rank
    s = sorted(vals)
    return s[max(1, math.ceil(p / 100.0 * len(s))) - 1]


def dist(vals):
    vals = [v for v in vals if isinstance(v, (int, float))]
    if not vals:
        return None
    return {"n": len(vals), "min": min(vals), "median": statistics.median(vals),
            "p95": pctl(vals, 95), "max": max(vals)}


def fmt(d, unit=""):
    if not d:
        return "(no data)"
    return (f"n={d['n']}  min={d['min']:.4g}{unit}  median={d['median']:.4g}{unit}  "
            f"p95={d['p95']:.4g}{unit}  max={d['max']:.4g}{unit}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--bin", required=True)
    ap.add_argument("--data-dir", required=True)
    ap.add_argument("--out-md", required=True)
    ap.add_argument("--suites", required=True)
    ap.add_argument("--cache-dir", default=None)
    ap.add_argument("--self-check", action="store_true")
    a = ap.parse_args()

    cache = ["--cache-dir", a.cache_dir] if a.cache_dir else []
    version = subprocess.run([a.bin, "--version"], capture_output=True,
                             text=True).stdout.strip()

    sessions = find_sessions(a.data_dir)
    if not sessions:
        log(f"NO session directories with >= 2 media files found under {a.data_dir}")
        log("nothing to measure — check the directory layout (one folder per session)")
        sys.exit(2)

    couples, cross_matches, errors, margins, sess_rows = [], [], [], [], []
    sel_cells, non_cells, tri_residuals = [], [], []
    constants = None

    for i, sess in enumerate(sessions, 1):
        name = os.path.relpath(sess, a.data_dir)
        log(f"[{i}/{len(sessions)}] pair {name}")
        rep, code, tail = run_json([a.bin, "--json", "--quiet", *cache, "pair", sess])
        if not rep or "rows" not in rep:
            errors.append(f"{name}: `couple pair` failed (exit {code}): {tail}")
            continue
        constants = constants or rep.get("constants")
        rows = rep["rows"]
        states = {}
        for r in rows:
            states[r["state"]] = states.get(r["state"], 0) + 1
        sess_rows.append({"session": name, "videos": len(rep.get("videos", [])),
                          "audios": len(rep.get("audios", [])), "states": states})

        selected = {r["video"]: r["selected"]["audio"] for r in rows if r.get("selected")}

        for r in rows:
            for t in (r.get("resolution") or {}).get("triangles", []):
                tri_residuals.append(t.get("residualMs"))

        by_video = {}
        for c in rep.get("matrix", []):
            by_video.setdefault(c["video"], []).append(c.get("z"))
        for v in sorted(by_video):
            zs = sorted((z for z in by_video[v] if isinstance(z, (int, float))),
                        reverse=True)
            if len(zs) >= 2:
                margins.append({"session": name, "video": v, "zBest": zs[0],
                                "zSecond": zs[1], "margin": zs[0] - zs[1]})

        for row_cells in rep.get("evidenceMatrix") or []:
            for cell in row_cells:
                rec = {k: cell.get(k) for k in
                       ("video", "audio", "classification", "score", "scoreMargin",
                        "support", "inlierFraction", "temporalCoverage",
                        "residualP95Ms")}
                rec["session"] = name
                if selected.get(cell["video"]) == cell["audio"]:
                    sel_cells.append(rec)
                else:
                    non_cells.append(rec)
                    if str(rec["classification"]).lower() == "match":
                        cross_matches.append(rec)

        for r in rows:
            sel = r.get("selected")
            if not sel:
                continue
            v, au = r["video"], sel["audio"]
            log(f"    sync {v} + {au}")
            s, code, tail = run_json([a.bin, "--json", "--quiet", *cache, "sync",
                                      os.path.join(sess, v), os.path.join(sess, au)])
            if not s or "offsetMs" not in s:
                errors.append(f"{name}/{v}: `couple sync` gave no result "
                              f"(exit {code}): {tail}")
                continue
            wins = s.get("windows", [])
            couples.append({
                "session": name, "video": v, "audio": au, "rowState": r["state"],
                "coverage": (sel.get("coverage") or {}).get("fraction"),
                "status": s.get("status"), "offsetMs": s.get("offsetMs"),
                "driftPpm": s.get("driftPpm"), "fitRmsMs": s.get("fitRmsMs"),
                "acceptedWindows": sum(1 for w in wins if w.get("accepted")),
                "windows": len(wins), "fineNcc": s.get("fineNcc"),
                "endToEndDriftMs": s.get("endToEndDriftMs"),
                "suggestedDriftMode": s.get("suggestedDriftMode"),
            })

    good = [c for c in couples if c["rowState"] in GOOD_STATES]
    L = SYNC_LINES
    flags = {
        "notSynced": [c for c in good if c["status"] != "synced"],
        "rmsOverLine": [c for c in good if isinstance(c["fitRmsMs"], (int, float))
                        and c["fitRmsMs"] > L["syncedMaxRmsMs"]],
        "nccUnderLine": [c for c in good if isinstance(c["fineNcc"], (int, float))
                         and c["fineNcc"] < L["syncedFineNcc"]],
        "windowsUnderLine": [c for c in good
                             if c["acceptedWindows"] < L["syncedMinWindows"]],
    }
    negatives = [c for c in non_cells
                 if str(c["classification"]).lower() in NEGATIVE_CLASSES]
    handsync = os.path.isdir(os.path.join(a.data_dir, "handsync")) and \
        any(not f.startswith(".") for f in
            os.listdir(os.path.join(a.data_dir, "handsync")))
    with open(a.suites) as f:
        suites = f.read().rstrip() or "NOT-RUN"

    now = datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%MZ")
    md = []
    md.append("# Couple real-data calibration report")
    md.append(f"\nGenerated {now} · `{version}` · dir `{a.data_dir}`")
    md.append("\nMeasurement only — no threshold was or will be changed by this tool. "
              "Next step: PLAN-REALDATA-CALIBRATION.md (owner sign-off required).")

    md.append("\n## 1. Sessions scanned\n")
    md.append("| session | videos | audios | row states |")
    md.append("|---|---|---|---|")
    for s in sess_rows:
        st = ", ".join(f"{k}: {v}" for k, v in sorted(s["states"].items())) or "—"
        md.append(f"| {s['session']} | {s['videos']} | {s['audios']} | {st} |")
    if errors:
        md.append("\n**Errors (sessions/couples not measured):**")
        for e in errors:
            md.append(f"- {e}")

    md.append("\n## 2. Sync verdicts vs the current [TUNE] lines (SPEC §5.6, core/src/sync.rs)\n")
    md.append("| session | video | audio | row | status | offset ms | drift ppm | "
              "fit RMS ms | windows | fine NCC | e2e drift ms |")
    md.append("|---|---|---|---|---|---|---|---|---|---|---|")
    for c in couples:
        rms = "—" if c["fitRmsMs"] is None else f"{c['fitRmsMs']:.3f}"
        md.append(f"| {c['session']} | {c['video']} | {c['audio']} | {c['rowState']} "
                  f"| {c['status']} | {c['offsetMs']:.1f} | {c['driftPpm']:.1f} | {rms} "
                  f"| {c['acceptedWindows']}/{c['windows']} | {c['fineNcc']:.3f} "
                  f"| {c['endToEndDriftMs']:.2f} |")
    md.append(f"\nTrue-couple distributions ({len(good)} couples in states "
              f"{sorted(GOOD_STATES)}):")
    md.append(f"- fit RMS ms: {fmt(dist([c['fitRmsMs'] for c in good]))}   "
              f"— line `synced_max_rms_ms = {L['syncedMaxRmsMs']}`; "
              f"**{len(flags['rmsOverLine'])}/{len(good)} correct couples over the line**")
    md.append(f"- |drift ppm|: {fmt(dist([abs(c['driftPpm']) for c in good]))}")
    md.append(f"- fine NCC: {fmt(dist([c['fineNcc'] for c in good]))}   "
              f"— line `synced_fine_ncc = {L['syncedFineNcc']}`; "
              f"{len(flags['nccUnderLine'])}/{len(good)} under")
    md.append(f"- accepted windows: {fmt(dist([c['acceptedWindows'] for c in good]))}   "
              f"— line `synced_min_windows = {L['syncedMinWindows']}`; "
              f"{len(flags['windowsUnderLine'])}/{len(good)} under")
    md.append(f"- e2e drift ms: {fmt(dist([c['endToEndDriftMs'] for c in good]))}   "
              f"— resample suggested over `suggest_resample_ms = {L['suggestResampleMs']}`")
    md.append(f"- status != synced on a correct couple: "
              f"**{len(flags['notSynced'])}/{len(good)}**")

    md.append("\n## 3. Identity & margin distributions (SPEC-V2 §3.5 / V2-F2)\n")
    if constants:
        md.append(f"Live resolver constants: zMin={constants.get('zMin')} "
                  f"zMargin={constants.get('zMargin')} "
                  f"triangleTolMs={constants.get('triangleTolMs')} "
                  f"coverageFull={constants.get('coverageFull')}\n")
    md.append(f"- selected-cell score: {fmt(dist([c['score'] for c in sel_cells]))}")
    md.append(f"- selected-cell scoreMargin: "
              f"{fmt(dist([c['scoreMargin'] for c in sel_cells]))}")
    md.append(f"- non-selected-cell score: {fmt(dist([c['score'] for c in non_cells]))}")
    md.append(f"- legacy z margin (zBest − zSecond) per video: "
              f"{fmt(dist([m['margin'] for m in margins]))}   — line Z_MARGIN")
    md.append(f"- non-selected classifications: " + (", ".join(
        f"{k}: {v}" for k, v in sorted(
            __import__('collections').Counter(
                str(c['classification']) for c in non_cells).items())) or "—"))
    if cross_matches:
        md.append(f"\n**⚠ {len(cross_matches)} non-selected cell(s) classify as "
                  f"`match` — the §3.5 different-take false-accept hazard. "
                  f"Review these by ear:**")
        for c in cross_matches:
            md.append(f"- {c['session']}: {c['video']} × {c['audio']} "
                      f"(score {c['score']:.3g}, margin {c['scoreMargin']:.3g}, "
                      f"support {c['support']})")
    else:
        md.append("\nNo non-selected cell classified as `match` "
                  "(no different-take false accept observed in this corpus).")

    md.append("\n## 4. Coverage & triangle closure\n")
    md.append(f"- selected coverage fraction: "
              f"{fmt(dist([c['coverage'] for c in good]))}   — line COVERAGE_FULL")
    md.append(f"- triangle residual ms: {fmt(dist(tri_residuals))}   "
              f"— line TRIANGLE_TOL_MS")

    md.append("\n## 5. Frozen-pair regression suites (SPEC §9.4)\n```")
    md.append(suites)
    md.append("```")

    md.append("\n## 6. Hand-sync material (SPEC §9.5)\n")
    md.append("- `handsync/` material present — the §9.5 gate can be activated "
              "(see PLAN-REALDATA-CALIBRATION.md §7)." if handsync else
              "- No `handsync/` material — the §9.5 gate stays dormant.")

    md.append("\n## 7. What next\n")
    md.append("Send this report (or the numbers above) through "
              "PLAN-REALDATA-CALIBRATION.md: it defines how the [TUNE] lines get "
              "re-frozen from these distributions (CALIBRATION.md §6–7 "
              "methodology) and the SPEC-CHANGE entries that need owner sign-off. "
              "No constant changes without that sign-off.")
    md_text = "\n".join(md) + "\n"

    with open(a.out_md, "w") as f:
        f.write(md_text)
    out_json = os.path.splitext(a.out_md)[0] + ".json"
    with open(out_json, "w") as f:
        json.dump({"generated": now, "version": version, "dir": a.data_dir,
                   "syncLines": SYNC_LINES, "constants": constants,
                   "sessions": sess_rows, "couples": couples, "margins": margins,
                   "selectedCells": sel_cells, "nonSelectedCells": non_cells,
                   "crossMatches": cross_matches,
                   "triangleResiduals": tri_residuals,
                   "suites": suites, "errors": errors}, f, indent=2, sort_keys=True)
    log(f"report written: {a.out_md}")
    log(f"raw data:       {out_json}")

    if a.self_check:
        fails = []

        def check(cond, what):
            (log(f"  ok    {what}") if cond else
             (fails.append(what), log(f"  FAIL  {what}")))

        log("self-check assertions:")
        check(len(couples) == 2, "exactly 2 couples measured")
        check(all(c["status"] == "synced" for c in couples),
              "both couples status=synced")
        check(all(c["rowState"] == "confident" for c in couples),
              "both rows confident")
        check(len(negatives) >= 2,
              "cross cells produce >= 2 negatives (noMatch/insufficientOverlap)")
        check(not cross_matches, "no cross-cell false match on synthetic negatives")
        check(not errors, "no tool errors")
        check(all(f"## {n}." in md_text for n in range(1, 8)),
              "report has all 7 sections")
        if fails:
            log(f"self-check FAILED ({len(fails)} assertion(s))")
            sys.exit(1)
        log("self-check OK")


if __name__ == "__main__":
    main()

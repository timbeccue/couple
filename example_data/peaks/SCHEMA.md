# Waveform peak data format

> **Note for AI/agent sessions:** the JSON files are large (~22 MB each, with
> multi-megabyte single-line arrays) — do not read them directly. Everything
> needed to write code against them is in this document; the files are meant
> to be fetched and parsed by the browser at runtime. If you must inspect one,
> use a targeted query, e.g.
> `jq '{source, duration_seconds, normalization, levels: [.levels[].samples_per_bin]}' camera.json`

One JSON file per track (`camera.json`, `external.json`), each containing a
min/max peak pyramid extracted from the track's audio decoded as **mono,
48 kHz, float**. The tracks are NOT aligned or trimmed: time `0` in each file
is the exact start of that source recording, so the real-world offset between
the camera and the external recorder is preserved in the data.

## Top-level object

```json
{
  "source": "schumann-video.mp4",     // original filename
  "duration_seconds": 1435.093333,    // decoded audio duration
  "sample_rate": 48000,               // decode rate (samples per second)
  "normalization": 1.46314096,        // see "Values" below
  "levels": [ { "samples_per_bin": ..., "peaks": [...] }, ... ]
}
```

## Levels

`levels` is sorted by `samples_per_bin` ascending — finest first:
`24, 96, 480, 2400, 9600, 48000` (0.5 ms/bin up to 1 s/bin).

Each level covers the whole track. Bin `i` summarizes samples
`[i * samples_per_bin, (i+1) * samples_per_bin)`, i.e. seconds
`[i, i+1) * samples_per_bin / sample_rate`. The last bin may cover fewer
samples. Bin count = `peaks.length / 2`.

## Values

`peaks` is a flat interleaved int array: `[min0, max0, min1, max1, ...]`,
one `[min, max]` pair per bin. Values are int8 in `-127..127`. To recover an
approximate float sample value:

```
float_value = peak_value / 127 * normalization
```

`normalization` is the track's own absolute peak, so each track uses the full
int8 range; amplitudes are therefore **not comparable across tracks** without
multiplying by each track's `normalization`. (Note: the camera track's AAC
decode peaks above 1.0, hence its normalization of ~1.46.)

Memory tip: after `JSON.parse`, copy each level's `peaks` into an
`Int8Array` (`new Int8Array(level.peaks)`) — plain JS number arrays cost ~8x
more RAM at this size (~7.5M and ~9.4M ints per file).

## Reading a time range (vanilla JS)

```js
const track = await (await fetch('peaks/camera.json')).json();
track.levels.forEach(l => l.peaks = new Int8Array(l.peaks));

// Peaks for [t0, t1] seconds, coarsest level that still yields >= minBins bins.
function peaksForRange(track, t0, t1, minBins) {
  const sr = track.sample_rate;
  const level = track.levels.findLast(l => (t1 - t0) * sr / l.samples_per_bin >= minBins)
             ?? track.levels[0];
  const spb = level.samples_per_bin, binCount = level.peaks.length / 2;
  const b0 = Math.max(0, Math.floor(t0 * sr / spb));
  const b1 = Math.min(binCount, Math.ceil(t1 * sr / spb));
  // pair j: bin b0+j spans [(b0+j) * spb / sr, (b0+j+1) * spb / sr) seconds
  return { level, firstBin: b0, peaks: level.peaks.subarray(2 * b0, 2 * b1) };
}
```

Typical use: `minBins` = the pixel width of the waveform viewport, so you get
at least one bin per pixel and never overfetch detail.

## Files

| file | source | duration | size |
|---|---|---|---|
| `camera.json` | schumann-video.mp4 (camera audio) | 1435.09 s | ~24 MB |
| `external.json` | schumann-audio.wav (external recorder) | 1798.88 s | ~22 MB |

//! PLAN-SPECTRAL-FINGERPRINT-MATCHING §8.4 — fingerprint cache lifecycle gates, exercised
//! END-TO-END through the real import/cache/track machinery (not just the pure codec).
//!
//! These tests own the §8.3 cache CONTRACT against the live infrastructure: a loaded track
//! lazily provides its versioned cached fingerprint reader, old entries upgrade in place, a
//! corrupt or format-incompatible sidecar rebuilds WITHOUT harming PCM/pyramid, concurrent
//! first-callers build at most once, failures leave no `.tmp`, and the whole thing rides on the
//! existing `CACHE_META_VERSION` entry layout without invalidating it. Cache GC's whole-entry
//! removal is verified to include the `.cpf`.
//!
//! §8.4 checklist (each mapped to a test below):
//! - build/write/mmap round trip ................ `build_write_mmap_round_trip`
//! - old cache WITHOUT fingerprint upgrades ..... `old_entry_without_fingerprint_upgrades`
//! - corrupt `.cpf` rebuild (PCM/pyramid live) .. `corrupt_fingerprint_rebuilds_pcm_pyramid_survive`
//! - incompatible params hash rebuild ........... `incompatible_params_hash_rebuilds`
//! - two concurrent callers, build once ......... `concurrent_callers_build_once`
//! - builder/write error leaves no `.tmp` ....... `no_tmp_left_behind`
//! - existing CACHE_META_VERSION readable ....... `existing_meta_version_entry_stays_readable`
//! - GC accounts the `.cpf` (whole-entry) ....... `gc_removes_fingerprint_with_entry`
//!
//! Determinism (§3 invariant 2): the source signal is a pure function of its seed; a track's
//! fingerprint bytes are reproducible across loads and across processes.

mod common;

use couple_core::cache::CacheEntry;
use couple_core::fingerprint::{build_artifact_bytes, FingerprintFormatV1, FingerprintReader};
use couple_core::TrackRole;
use std::path::Path;
use std::sync::Arc;

const SR: usize = 48_000;

/// Deterministic broadband 48 kHz signal with sharp transients the peak picker locks onto, keyed
/// to the absolute sample index (pure / reproducible). Yields a non-empty landmark set.
fn signal(seed: u64, n: usize) -> Vec<f32> {
    let sr = SR as f64;
    let mut h = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(0xABCD);
    let mut rnd = || {
        h ^= h >> 30;
        h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        h ^= h >> 27;
        h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
        h ^= h >> 31;
        (h >> 11) as f64 / (1u64 << 53) as f64
    };
    let dur = n as f64 / sr;
    let mut evs: Vec<(f64, [f64; 5], f64, f64)> = Vec::new();
    let mut t = 0.0;
    loop {
        t += -(rnd().max(1e-9)).ln() / 4.0;
        if t >= dur {
            break;
        }
        let mut f = [0f64; 5];
        for x in &mut f {
            *x = 300.0 + rnd() * 5000.0;
        }
        evs.push((t, f, 0.3 + 0.6 * rnd(), 0.03 + 0.12 * rnd()));
    }
    (0..n)
        .map(|i| {
            let tt = i as f64 / sr;
            let mut s = 0.0;
            for &(t0, f, amp, tau) in &evs {
                let dt = tt - t0;
                if dt >= 0.0 && dt < 8.0 * tau {
                    let env = amp * (-dt / tau).exp();
                    let mut burst = 0.0;
                    for &fr in &f {
                        burst += (2.0 * std::f64::consts::PI * fr * dt).sin();
                    }
                    s += env * burst / (5f64).sqrt();
                }
            }
            s.clamp(-0.999, 0.999) as f32
        })
        .collect()
}

/// Import a freshly-authored WAV and return (ctx, track_id, content_key).
fn import_signal(
    dir: &Path,
    name: &str,
    seed: u64,
    n: usize,
) -> (Arc<couple_core::Ctx>, u64, String) {
    let wav = dir.join(name);
    common::write_wav_mono_48k(&wav, &signal(seed, n));
    let ctx = common::test_ctx(&dir.join("cache"));
    let id = common::import(&ctx, &wav, TrackRole::External).unwrap();
    let key = ctx.track(id).unwrap().meta.content_key.clone();
    (ctx, id, key)
}

fn cpf_path(cache_dir: &Path, key: &str) -> std::path::PathBuf {
    CacheEntry::new(cache_dir, key).fingerprint_path_v1()
}

/// BUILD / WRITE / MMAP ROUND TRIP (§8.4): import generates the `.cpf`; the loaded track returns a
/// valid reader whose bytes match a clean build from the same canonical PCM.
#[test]
fn build_write_mmap_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let (ctx, id, key) = import_signal(dir.path(), "rt.wav", 1, SR * 4);

    // Import wrote the sidecar in the decode pass (§8.3.1).
    assert!(cpf_path(&cache, &key).is_file(), "import must write fingerprint-v1.cpf");

    let track = ctx.track(id).unwrap();
    let fp = track.fingerprint_v1().expect("loaded track yields a fingerprint");
    assert!(!fp.is_empty(), "broadband signal must produce landmarks");
    assert_eq!(fp.duration_frames, track.frames());
    assert_eq!(fp.params_hash, FingerprintFormatV1::fixed().params_hash());

    // The on-disk bytes equal a clean build of the same PCM (determinism + mmap round trip), and
    // re-parse to the same landmark set. Build expected from the exact canonical PCM the cache
    // holds (read straight off disk).
    let on_disk = std::fs::read(cpf_path(&cache, &key)).unwrap();
    let pcm_bytes = std::fs::read(CacheEntry::new(&cache, &key).pcm_path()).unwrap();
    let pcm: &[f32] = unsafe {
        std::slice::from_raw_parts(pcm_bytes.as_ptr() as *const f32, pcm_bytes.len() / 4)
    };
    let expected = build_artifact_bytes(pcm, track.frames());
    assert_eq!(on_disk, expected, "stored .cpf must equal a clean PCM build");

    // Calling again returns the same memoized reader (Arc identity).
    let fp2 = track.fingerprint_v1().unwrap();
    assert!(Arc::ptr_eq(&fp, &fp2), "second call must return the memoized reader");
}

/// OLD CACHE WITHOUT FINGERPRINT LAZILY UPGRADES (§8.4): an entry that predates fingerprints (no
/// `.cpf`) stays valid and gains one on first request — built from the mmap PCM, no re-decode.
#[test]
fn old_entry_without_fingerprint_upgrades() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let (ctx, id, key) = import_signal(dir.path(), "old.wav", 2, SR * 4);

    // Simulate a pre-fingerprint entry: delete the sidecar the import wrote. The entry must remain
    // a VALID cache hit (load_meta does not consider the .cpf).
    let cpf = cpf_path(&cache, &key);
    std::fs::remove_file(&cpf).unwrap();
    assert!(!cpf.exists());
    let entry = CacheEntry::new(&cache, &key);
    assert!(entry.load_meta(&key).is_some(), "entry without .cpf must still be a valid hit");

    // Re-open the track fresh (a cache HIT on the same key returns a new handle with an empty
    // OnceLock, and a hit does NOT re-run the decode-pass sidecar generation — so the sidecar
    // stays absent until requested, exactly the lazy-upgrade path).
    ctx.track_free(id);
    let id2 = common::import(&ctx, &dir.path().join("old.wav"), TrackRole::External).unwrap();
    let track = ctx.track(id2).unwrap();
    assert!(!cpf.exists(), "cache-hit re-import must not regenerate the sidecar");

    let fp = track.fingerprint_v1().expect("lazy build succeeds");
    assert!(cpf.is_file(), "first request installs the .cpf");
    assert!(!fp.is_empty());
    assert_eq!(fp.duration_frames, track.frames());
    // PCM/pyramid/onsets untouched and entry still valid.
    assert!(entry.load_meta(&key).is_some(), "entry remains valid after upgrade");
}

/// CORRUPT `.cpf` → REBUILD, and PCM/PYRAMID SURVIVE (§8.4 + §8.3.6): a garbage sidecar is deleted
/// and rebuilt on request; the other artifacts are byte-for-byte unchanged.
#[test]
fn corrupt_fingerprint_rebuilds_pcm_pyramid_survive() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let (ctx, id, key) = import_signal(dir.path(), "corrupt.wav", 3, SR * 4);
    let entry = CacheEntry::new(&cache, &key);

    // Snapshot PCM + pyramid (+ meta + onsets) BEFORE corruption.
    let pcm_before = std::fs::read(entry.pcm_path()).unwrap();
    let pyr_before = std::fs::read(entry.pyramid_path()).unwrap();
    let meta_before = std::fs::read(entry.meta_path()).unwrap();
    let onsets_before = std::fs::read(entry.onsets_path()).unwrap();

    // Corrupt the sidecar with bytes that cannot be a CPF1 artifact.
    std::fs::write(entry.fingerprint_path_v1(), b"\x00\x01\x02 corrupt cpf bytes").unwrap();

    // Fresh handle so the rebuild goes through disk, then request.
    ctx.track_free(id);
    let id2 = common::import(&ctx, &dir.path().join("corrupt.wav"), TrackRole::External).unwrap();
    let track = ctx.track(id2).unwrap();
    // Re-corrupt (the cache-hit import does not rebuild the sidecar) so the lazy path is what fixes it.
    std::fs::write(entry.fingerprint_path_v1(), b"\x00\x01\x02 corrupt cpf bytes").unwrap();

    let fp = track.fingerprint_v1().expect("corrupt sidecar rebuilds, not errors");
    assert!(!fp.is_empty());

    // The rebuilt .cpf now parses cleanly and equals a clean PCM build.
    let pcm: &[f32] = unsafe {
        std::slice::from_raw_parts(pcm_before.as_ptr() as *const f32, pcm_before.len() / 4)
    };
    assert_eq!(
        std::fs::read(entry.fingerprint_path_v1()).unwrap(),
        build_artifact_bytes(pcm, track.frames()),
        "rebuilt sidecar must equal a clean build"
    );

    // PCM/pyramid/meta/onsets are bit-for-bit unchanged by the fingerprint rebuild.
    assert_eq!(std::fs::read(entry.pcm_path()).unwrap(), pcm_before, "PCM must survive");
    assert_eq!(std::fs::read(entry.pyramid_path()).unwrap(), pyr_before, "pyramid must survive");
    assert_eq!(std::fs::read(entry.meta_path()).unwrap(), meta_before, "meta must survive");
    assert_eq!(std::fs::read(entry.onsets_path()).unwrap(), onsets_before, "onsets must survive");
}

/// INCOMPATIBLE PARAMS HASH → REBUILD (§8.4): a sidecar written under a different analysis format
/// (different params hash) is rejected and rebuilt under the current frozen format.
#[test]
fn incompatible_params_hash_rebuilds() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let (ctx, id, key) = import_signal(dir.path(), "params.wav", 4, SR * 4);
    let entry = CacheEntry::new(&cache, &key);

    let pcm_before = std::fs::read(entry.pcm_path()).unwrap();
    let pcm: &[f32] = unsafe {
        std::slice::from_raw_parts(pcm_before.as_ptr() as *const f32, pcm_before.len() / 4)
    };

    // Write a VALID CPF1 file built under a DIFFERENT format (changed hop ⇒ different params hash).
    let mut other = FingerprintFormatV1::fixed();
    other.hop_size = 128;
    let mut b = couple_core::fingerprint::FingerprintBuilder::with_format(other.clone());
    b.push(pcm);
    let other_lms = b.finalize_landmarks();
    let frames = ctx.track(id).unwrap().frames();
    let stale = couple_core::fingerprint::file::encode(&other, frames, &other_lms);
    // Sanity: stale is a structurally valid CPF1 (parses under its own format) but not under ours.
    assert!(FingerprintReader::parse(&stale, &other).is_ok());
    assert!(FingerprintReader::parse(&stale, &FingerprintFormatV1::fixed()).is_err());
    std::fs::write(entry.fingerprint_path_v1(), &stale).unwrap();

    ctx.track_free(id);
    let id2 = common::import(&ctx, &dir.path().join("params.wav"), TrackRole::External).unwrap();
    let track = ctx.track(id2).unwrap();
    std::fs::write(entry.fingerprint_path_v1(), &stale).unwrap(); // re-stage for the lazy path

    let fp = track.fingerprint_v1().expect("incompatible hash rebuilds, not errors");
    assert_eq!(fp.params_hash, FingerprintFormatV1::fixed().params_hash());
    assert_eq!(
        std::fs::read(entry.fingerprint_path_v1()).unwrap(),
        build_artifact_bytes(pcm, frames),
        "rebuilt sidecar must use the current frozen format"
    );
}

/// TWO CONCURRENT CALLERS RECEIVE THE SAME ARTIFACT, BUILD HAPPENS ONCE (§8.4). Contention is
/// REAL: many threads request `fingerprint_v1()` on the SAME handle simultaneously, gated on a
/// barrier so they collide on the empty OnceLock. They must all receive the identical `Arc`
/// (pointer-equal ⇒ exactly one build was published).
#[test]
fn concurrent_callers_build_once() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let (ctx, id, key) = import_signal(dir.path(), "race.wav", 5, SR * 4);

    // Start from NO sidecar so the first request actually builds (maximizes the contended window).
    std::fs::remove_file(cpf_path(&cache, &key)).unwrap();

    let track = ctx.track(id).unwrap();
    const N: usize = 16;
    let barrier = Arc::new(std::sync::Barrier::new(N));
    let mut handles = Vec::with_capacity(N);
    for _ in 0..N {
        let track = Arc::clone(&track);
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            barrier.wait(); // release all threads at once into the same empty OnceLock
            track.fingerprint_v1().expect("concurrent build succeeds")
        }));
    }
    let readers: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

    // Every caller got the SAME Arc (one build, one stored value).
    let first = &readers[0];
    for r in &readers[1..] {
        assert!(Arc::ptr_eq(first, r), "all concurrent callers must share one built artifact");
    }
    assert!(cpf_path(&cache, &key).is_file());
}

/// ANY BUILDER OR WRITE ERROR LEAVES NO `.tmp` (§8.4). We force the atomic write to fail by making
/// the entry directory read-only, so `write(<cpf>.tmp)` cannot create the temp file. The request
/// fails, and crucially NO `fingerprint-v1.cpf.tmp` is left in the entry.
#[test]
#[cfg(unix)]
fn no_tmp_left_behind() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let (ctx, id, key) = import_signal(dir.path(), "ro.wav", 6, SR * 4);
    let entry = CacheEntry::new(&cache, &key);

    // Remove the import-written sidecar so a request must (try to) build.
    std::fs::remove_file(entry.fingerprint_path_v1()).unwrap();

    // Make the entry dir read-only so creating <cpf>.tmp fails.
    let entry_dir = entry.dir.clone();
    let orig = std::fs::metadata(&entry_dir).unwrap().permissions();
    let mut ro = orig.clone();
    ro.set_mode(0o500); // r-x------ : no write ⇒ cannot create the temp file
    std::fs::set_permissions(&entry_dir, ro).unwrap();

    let track = ctx.track(id).unwrap();
    let result = track.fingerprint_v1();

    // Restore permissions before asserting/cleanup so tempdir can drop.
    std::fs::set_permissions(&entry_dir, orig).unwrap();

    assert!(result.is_err(), "a write failure must surface as an error, not silent success");
    let tmp = {
        let mut p = entry.fingerprint_path_v1().into_os_string();
        p.push(".tmp");
        std::path::PathBuf::from(p)
    };
    assert!(!tmp.exists(), "no .cpf.tmp may be left after a failed build/write");
    assert!(!entry.fingerprint_path_v1().exists(), "no partial .cpf may be left either");
}

/// EXISTING `CACHE_META_VERSION` ENTRIES REMAIN READABLE (§8.4 / §3 invariant): an entry whose
/// `meta.json` carries the current `CACHE_META_VERSION` and which has NO fingerprint sidecar is a
/// valid hit and re-imports without rebuild. (No cache-version bump was introduced by this
/// boundary.)
#[test]
fn existing_meta_version_entry_stays_readable() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let (ctx, id, key) = import_signal(dir.path(), "ver.wav", 7, SR * 3);
    let entry = CacheEntry::new(&cache, &key);

    // The meta carries the current version; assert that and that the entry validates without a .cpf.
    let meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(entry.meta_path()).unwrap()).unwrap();
    assert_eq!(
        meta["version"].as_u64().unwrap() as u32,
        couple_core::cache::CACHE_META_VERSION,
        "boundary 3 must NOT bump CACHE_META_VERSION"
    );

    std::fs::remove_file(entry.fingerprint_path_v1()).unwrap();
    assert!(entry.load_meta(&key).is_some(), "version-current entry without .cpf must be a hit");

    // Re-import: a cache hit (same key) producing an equivalent track. PCM unchanged.
    let pcm_before = std::fs::read(entry.pcm_path()).unwrap();
    ctx.track_free(id);
    let id2 = common::import(&ctx, &dir.path().join("ver.wav"), TrackRole::External).unwrap();
    assert_eq!(ctx.track(id2).unwrap().meta.content_key, key);
    assert_eq!(std::fs::read(entry.pcm_path()).unwrap(), pcm_before, "cache-hit must not rewrite PCM");
}

/// CACHE GC ACCOUNTS THE `.cpf` VIA WHOLE-ENTRY REMOVAL (§8.3.7). The fingerprint lives inside the
/// entry dir, so `list_entries` counts its bytes and GC's `remove_dir_all` deletes it along with
/// the entry — no fingerprint-specific GC path. We verify the `.cpf` bytes are included in the
/// reported entry size and that GC removes the whole entry (sidecar included) once unpinned.
#[test]
fn gc_removes_fingerprint_with_entry() {
    let dir = tempfile::tempdir().unwrap();
    let cache = dir.path().join("cache");
    let (ctx, id, key) = import_signal(dir.path(), "gc.wav", 8, SR * 3);
    let entry = CacheEntry::new(&cache, &key);

    // Ensure a fingerprint exists (import writes it).
    assert!(entry.fingerprint_path_v1().is_file());
    let cpf_len = std::fs::metadata(entry.fingerprint_path_v1()).unwrap().len();
    assert!(cpf_len > 0);

    // The entry's reported byte total includes the .cpf (sum over all files in the dir).
    let entries = couple_core::cache::list_entries(&cache);
    let me = entries.iter().find(|e| e.key == key).expect("entry listed");
    let sum_files: u64 = std::fs::read_dir(&entry.dir)
        .unwrap()
        .flatten()
        .map(|f| f.metadata().map(|m| m.len()).unwrap_or(0))
        .sum();
    assert_eq!(me.bytes, sum_files, "GC byte-accounting must walk the whole entry dir");
    assert!(me.bytes >= cpf_len, "entry bytes must include the fingerprint sidecar");

    // Free the handle (unpin) and GC to 0: the whole entry — sidecar included — is removed.
    ctx.track_free(id);
    ctx.cache_gc(0);
    assert!(!entry.dir.exists(), "GC must remove the whole entry");
    assert!(!entry.fingerprint_path_v1().exists(), "the .cpf goes with the entry");
}

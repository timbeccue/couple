//! Content-addressed cache: key, directory layout, lifecycle, GC (§6.3).

use crate::error::{CoupleError, Result};
use crate::media::TrackStats;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const KEY_SAMPLE_BYTES: u64 = 8 * 1024 * 1024;

/// BLAKE3(first 8 MiB ‖ middle 8 MiB ‖ last 8 MiB ‖ u64 size), hex-truncated to 128 bits (§6.3).
/// Middle slice: len = min(size, 8 MiB), starting at (size − len) / 2 — centered, floor division.
/// Slices may overlap (any file < 24 MiB); a file < 8 MiB is simply hashed three times, matching
/// the existing head/tail clamp idiom. Pure function of (content, size): SPEC-CHANGE 2026-07-12
/// (F3b) — a middle-only edit previously reused stale caches.
pub fn content_key(path: &Path) -> Result<String> {
    let mut f = fs::File::open(path).map_err(|_| CoupleError::FileNotFound {
        path: path.to_path_buf(),
    })?;
    let size = f
        .metadata()
        .map_err(|e| CoupleError::internal(format!("stat: {e}")))?
        .len();
    let mut hasher = blake3::Hasher::new();
    let head_len = size.min(KEY_SAMPLE_BYTES);
    let mut buf = vec![0u8; head_len as usize];
    f.read_exact(&mut buf)
        .map_err(|e| CoupleError::internal(format!("read head: {e}")))?;
    hasher.update(&buf);
    let mid_len = size.min(KEY_SAMPLE_BYTES);
    let mid_start = (size - mid_len) / 2;
    f.seek(SeekFrom::Start(mid_start))
        .map_err(|e| CoupleError::internal(format!("seek middle: {e}")))?;
    let mut buf = vec![0u8; mid_len as usize];
    f.read_exact(&mut buf)
        .map_err(|e| CoupleError::internal(format!("read middle: {e}")))?;
    hasher.update(&buf);
    let tail_len = size.min(KEY_SAMPLE_BYTES);
    f.seek(SeekFrom::Start(size - tail_len))
        .map_err(|e| CoupleError::internal(format!("seek tail: {e}")))?;
    let mut buf = vec![0u8; tail_len as usize];
    f.read_exact(&mut buf)
        .map_err(|e| CoupleError::internal(format!("read tail: {e}")))?;
    hasher.update(&buf);
    hasher.update(&size.to_le_bytes());
    let hash = hasher.finalize();
    Ok(hex128(hash.as_bytes()))
}

fn hex128(bytes: &[u8; 32]) -> String {
    bytes[..16].iter().map(|b| format!("{b:02x}")).collect()
}

/// Platform default cache root (`~/Library/Caches/Couple/v1` on macOS).
pub fn default_cache_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    if cfg!(target_os = "macos") {
        home.join("Library/Caches/Couple/v1")
    } else if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
        PathBuf::from(xdg).join("couple/v1")
    } else {
        home.join(".cache/couple/v1")
    }
}

/// Sidecar `meta.json` (§6.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheMeta {
    pub version: u32,
    pub content_key: String,
    pub sample_rate: u32,
    pub frames: u64,
    pub epoch_s: f64,
    pub source_duration_s: f64,
    pub normalization: f64,
    pub stats: TrackStats,
    pub created_by: String,
}

pub const CACHE_META_VERSION: u32 = 1;

pub struct CacheEntry {
    pub dir: PathBuf,
}

impl CacheEntry {
    pub fn new(cache_root: &Path, key: &str) -> Self {
        Self {
            dir: cache_root.join(key),
        }
    }
    pub fn pyramid_path(&self) -> PathBuf {
        self.dir.join("pyramid.cpk")
    }
    pub fn pcm_path(&self) -> PathBuf {
        self.dir.join("pcm48f32.raw")
    }
    pub fn onsets_path(&self) -> PathBuf {
        self.dir.join("onsets.json")
    }
    pub fn meta_path(&self) -> PathBuf {
        self.dir.join("meta.json")
    }
    /// Derived spectral-fingerprint sidecar `fingerprint-v1.cpf`
    /// (PLAN-SPECTRAL-FINGERPRINT-MATCHING §8.3). It has an INDEPENDENT format/version lifecycle
    /// from the PCM and pyramid and is always rebuildable from `pcm48f32.raw`, so it is
    /// DELIBERATELY NOT part of [`load_meta`]'s validity check: a valid entry without this file is
    /// still a valid entry (§3 invariant 7 — old caches stay readable; a missing fingerprint is
    /// not corruption). It lives inside the entry dir, so whole-entry GC accounts for and removes
    /// it automatically — no GC special-casing.
    pub fn fingerprint_path_v1(&self) -> PathBuf {
        self.dir.join("fingerprint-v1.cpf")
    }

    /// Valid hit: meta parses, key matches, artifact files present with consistent sizes.
    pub fn load_meta(&self, key: &str) -> Option<CacheMeta> {
        let meta: CacheMeta = serde_json::from_slice(&fs::read(self.meta_path()).ok()?).ok()?;
        if meta.version != CACHE_META_VERSION || meta.content_key != key {
            return None;
        }
        let pcm_len = fs::metadata(self.pcm_path()).ok()?.len();
        if pcm_len != meta.frames * 4 {
            return None;
        }
        if !self.pyramid_path().is_file() || !self.onsets_path().is_file() {
            return None;
        }
        Some(meta)
    }

    /// Bump LRU recency (mtime of meta.json records lastUsed).
    pub fn touch(&self) {
        let p = self.meta_path();
        if let Ok(bytes) = fs::read(&p) {
            let _ = fs::write(&p, bytes);
        }
    }

    pub fn evict(&self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Free bytes on the volume holding `path`, or None if the platform query failed.
#[cfg(unix)]
fn available_bytes_impl(path: &Path) -> Option<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let cpath = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(cpath.as_ptr(), &mut st) } == 0 {
        Some((st.f_bavail as u64).saturating_mul(st.f_frsize as u64))
    } else {
        None
    }
}

/// Free bytes on the volume holding `path`, or None if the platform query failed.
#[cfg(windows)]
fn available_bytes_impl(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut free: u64 = 0;
    // SAFETY: NUL-terminated wide path; out-params are plain u64s; null totals allowed.
    let ok = unsafe {
        GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut())
    };
    (ok != 0).then_some(free)
}

/// Free bytes on the volume holding `path`. A failed query is loudly assumed
/// infinite: the DiskFull preflight (§6.3/§8.3) is a guard, not a gate, and
/// blocking imports on an unqueryable volume would be worse — but it must
/// never be disabled silently.
pub fn available_bytes(path: &Path) -> u64 {
    match available_bytes_impl(path) {
        Some(b) => b,
        None => {
            eprintln!(
                "couple-core: free-disk query failed for {}; assuming enough space (disk-full preflight skipped)",
                path.display()
            );
            u64::MAX
        }
    }
}

/// Import preflight (§6.3): free disk ≥ 1.5× projected cache size.
pub fn preflight(cache_root: &Path, projected_bytes: u64) -> Result<()> {
    fs::create_dir_all(cache_root)
        .map_err(|e| CoupleError::internal(format!("create cache dir: {e}")))?;
    let avail = available_bytes(cache_root);
    let needed = projected_bytes + projected_bytes / 2;
    if avail < needed {
        return Err(CoupleError::DiskFull {
            needed_bytes: needed,
            available_bytes: avail,
        });
    }
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheEntryInfo {
    pub key: String,
    pub bytes: u64,
    pub last_used_unix: i64,
}

pub fn list_entries(cache_root: &Path) -> Vec<CacheEntryInfo> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(cache_root) else {
        return out;
    };
    for e in rd.flatten() {
        let dir = e.path();
        if !dir.is_dir() {
            continue;
        }
        let mut bytes = 0u64;
        if let Ok(files) = fs::read_dir(&dir) {
            for f in files.flatten() {
                bytes += f.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
        let last_used = fs::metadata(dir.join("meta.json"))
            .and_then(|m| m.modified())
            .map(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        out.push(CacheEntryInfo {
            key: e.file_name().to_string_lossy().to_string(),
            bytes,
            last_used_unix: last_used,
        });
    }
    out.sort_by_key(|e| e.last_used_unix);
    out
}

/// LRU GC down to `max_bytes` (§6.3). Keys in `pinned` are never evicted
/// (SPEC-V2 §4 / V2-F3: tracks referenced by the open session are exempt; the
/// caller — `Ctx::cache_gc` — derives the set from open track handles). Pinned
/// bytes still count toward the cap, so unpinned entries make room first.
/// Returns bytes freed.
pub fn gc(cache_root: &Path, max_bytes: u64, pinned: &std::collections::HashSet<String>) -> u64 {
    let entries = list_entries(cache_root);
    let mut total: u64 = entries.iter().map(|e| e.bytes).sum();
    let mut freed = 0;
    for e in &entries {
        if total <= max_bytes {
            break;
        }
        if pinned.contains(&e.key) {
            continue;
        }
        let _ = fs::remove_dir_all(cache_root.join(&e.key));
        total -= e.bytes;
        freed += e.bytes;
    }
    freed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Write a deterministic patterned file of `size` bytes into a fresh tempdir.
    /// Returns (tempdir kept alive by caller, path).
    fn patterned_file(size: usize) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("media.bin");
        let mut f = fs::File::create(&path).unwrap();
        // Simple deterministic pattern; distinct across the whole span.
        let mut buf = vec![0u8; size];
        for (i, b) in buf.iter_mut().enumerate() {
            *b = (i as u64 % 251) as u8;
        }
        f.write_all(&buf).unwrap();
        f.flush().unwrap();
        (dir, path)
    }

    fn flip_byte(path: &Path, offset: u64) {
        let mut f = fs::OpenOptions::new().read(true).write(true).open(path).unwrap();
        f.seek(SeekFrom::Start(offset)).unwrap();
        let mut one = [0u8; 1];
        f.read_exact(&mut one).unwrap();
        one[0] ^= 0xff;
        f.seek(SeekFrom::Start(offset)).unwrap();
        f.write_all(&one).unwrap();
        f.flush().unwrap();
    }

    const MIB: usize = 1024 * 1024;

    #[test]
    fn middle_edit_changes_key() {
        // 25 MiB: disjoint head/middle/tail slices.
        let (_dir, path) = patterned_file(25 * MIB);
        let base = content_key(&path).unwrap();
        flip_byte(&path, (25 * MIB / 2) as u64);
        let after = content_key(&path).unwrap();
        assert_ne!(base, after, "flipping the middle byte must change the key");
    }

    #[test]
    fn head_and_tail_edits_still_change_key() {
        let (_dir, path) = patterned_file(25 * MIB);
        let base = content_key(&path).unwrap();

        flip_byte(&path, 0);
        let head = content_key(&path).unwrap();
        flip_byte(&path, 0); // restore

        flip_byte(&path, (25 * MIB - 1) as u64);
        let tail = content_key(&path).unwrap();

        assert_ne!(base, head);
        assert_ne!(base, tail);
        assert_ne!(head, tail);
    }

    #[test]
    fn small_file_key_matches_composition() {
        // 1 MiB < 8 MiB: all three slices are the full file, hashed three times, no dedup.
        let (_dir, path) = patterned_file(MIB);
        let k1 = content_key(&path).unwrap();
        let k2 = content_key(&path).unwrap();
        assert_eq!(k1, k2, "key must be stable across computations");

        let bytes = fs::read(&path).unwrap();
        let size = bytes.len() as u64;
        let mut hasher = blake3::Hasher::new();
        hasher.update(&bytes); // head
        hasher.update(&bytes); // middle
        hasher.update(&bytes); // tail
        hasher.update(&size.to_le_bytes());
        let expected = hex128(hasher.finalize().as_bytes());
        assert_eq!(k1, expected, "small-file triple-hash composition must match");
    }

    #[test]
    fn overlapping_regimes_stable_and_middle_sensitive() {
        for &size in &[10 * MIB, 20 * MIB] {
            let (_dir, path) = patterned_file(size);
            let k1 = content_key(&path).unwrap();
            let k2 = content_key(&path).unwrap();
            assert_eq!(k1, k2, "key must be stable across computations (size {size})");
            flip_byte(&path, (size / 2) as u64);
            let after = content_key(&path).unwrap();
            assert_ne!(k1, after, "middle flip must change the key (size {size})");
        }
    }
}

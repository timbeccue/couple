//! Content-addressed cache: key, directory layout, lifecycle, GC (§6.3).

use crate::error::{CoupleError, Result};
use crate::media::TrackStats;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const KEY_SAMPLE_BYTES: u64 = 8 * 1024 * 1024;

/// BLAKE3(first 8 MiB ‖ last 8 MiB ‖ u64 size), hex-truncated to 128 bits (§6.3).
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

/// Free bytes on the volume holding `path`.
pub fn available_bytes(path: &Path) -> u64 {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let Some(cpath) = CString::new(path.as_os_str().as_bytes()).ok() else {
        return u64::MAX;
    };
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(cpath.as_ptr(), &mut st) } == 0 {
        (st.f_bavail as u64).saturating_mul(st.f_frsize as u64)
    } else {
        u64::MAX
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

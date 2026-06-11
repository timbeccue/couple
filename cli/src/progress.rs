//! CLI progress rendering (§10.1), SIGINT → job cancellation (§10.3), PNG eyeball dump.

use couple_core::{Band, CancelToken, Ctx, Phase, ProgressFn};
use indicatif::{ProgressBar, ProgressStyle};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

static SIGINT_FLAG: AtomicBool = AtomicBool::new(false);
static SIGINT_TOKEN: OnceLock<CancelToken> = OnceLock::new();

extern "C" fn on_sigint(_: libc::c_int) {
    SIGINT_FLAG.store(true, Ordering::SeqCst);
    if let Some(t) = SIGINT_TOKEN.get() {
        t.cancel();
    }
}

/// SIGINT honors job cancellation and cleans temps (§10.3, exit code 8).
pub fn install_sigint_cancel() -> CancelToken {
    let token = CancelToken::new();
    let _ = SIGINT_TOKEN.set(token.clone());
    unsafe {
        libc::signal(libc::SIGINT, on_sigint as *const () as libc::sighandler_t);
    }
    token
}

/// Maps per-phase fractions onto one 0–100% bar using the §4.6 ranges.
fn overall(phase: Phase, fraction: f64) -> f64 {
    let (lo, hi) = match phase {
        Phase::Probe => (0.0, 0.02),
        Phase::Decode => (0.02, 0.80),
        Phase::Pyramid => (0.80, 0.95),
        Phase::Finalize => (0.95, 1.0),
        Phase::SyncCoarse => (0.0, 0.70),
        Phase::SyncRefine => (0.70, 0.75),
        Phase::SyncWindows => (0.75, 0.98),
        Phase::SyncFit => (0.98, 1.0),
        Phase::ExportRender => (0.0, 0.70),
        Phase::ExportMux => (0.70, 1.0),
    };
    lo + (hi - lo) * fraction.clamp(0.0, 1.0)
}

pub struct Reporter {
    bar: Option<Arc<ProgressBar>>,
    ndjson: bool,
    state: Arc<Mutex<()>>,
}

impl Reporter {
    pub fn new(quiet: bool, ndjson: bool, label: &Path) -> Self {
        let tty = unsafe { libc::isatty(libc::STDERR_FILENO) } == 1;
        let bar = if quiet || ndjson || !tty {
            None
        } else {
            let b = ProgressBar::new(1000);
            b.set_style(
                ProgressStyle::with_template("{msg:24} [{bar:40}] {percent:>3}%")
                    .unwrap()
                    .progress_chars("=> "),
            );
            b.set_message(
                label
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default(),
            );
            Some(Arc::new(b))
        };
        Self {
            bar,
            ndjson,
            state: Arc::new(Mutex::new(())),
        }
    }

    pub fn progress_fn(&self) -> ProgressFn {
        let bar = self.bar.clone();
        let ndjson = self.ndjson;
        let lock = Arc::clone(&self.state);
        Arc::new(move |p: &couple_core::Progress| {
            let _g = lock.lock().unwrap();
            if let Some(b) = &bar {
                b.set_position((overall(p.phase, p.fraction) * 1000.0) as u64);
                b.set_message(p.phase.label().to_string());
            }
            if ndjson {
                let mut err = std::io::stderr().lock();
                let _ = writeln!(
                    err,
                    "{}",
                    serde_json::json!({ "phase": p.phase, "fraction": p.fraction })
                );
            }
        })
    }

    pub fn finish(&self) {
        if let Some(b) = &self.bar {
            b.finish_and_clear();
        }
    }
}

/// `couple peaks --png`: stepped min/max columns rendered through ffmpeg's PNG encoder
/// (no image crate; ffmpeg is already a dependency).
pub fn render_band_png(ctx: &Ctx, band: &Band, out: &PathBuf) -> couple_core::Result<()> {
    let w = band.mins.len();
    let h = 256usize;
    let mid = (h / 2) as i32;
    let mut img = vec![0u8; w * h]; // gray8
    for (x, (&mn, &mx)) in band.mins.iter().zip(&band.maxs).enumerate() {
        let y0 = (mid - mx as i32).clamp(0, h as i32 - 1);
        let y1 = (mid - mn as i32).clamp(0, h as i32 - 1);
        for y in y0..=y1 {
            img[y as usize * w + x] = 230;
        }
    }
    let ffmpeg = ctx.ffmpeg()?;
    let mut child = std::process::Command::new(ffmpeg)
        .args(["-nostdin", "-v", "error", "-y", "-f", "rawvideo", "-pix_fmt", "gray"])
        .args(["-s", &format!("{w}x{h}"), "-i", "pipe:0", "-frames:v", "1"])
        .arg(out)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| couple_core::CoupleError::internal(format!("spawn ffmpeg png: {e}")))?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&img)
        .map_err(|e| couple_core::CoupleError::internal(format!("write png pipe: {e}")))?;
    let status = child
        .wait()
        .map_err(|e| couple_core::CoupleError::internal(format!("wait ffmpeg png: {e}")))?;
    if !status.success() {
        return Err(couple_core::CoupleError::FfmpegFailed {
            stage: "png".into(),
            exit_code: status.code(),
            stderr_tail: String::new(),
        });
    }
    Ok(())
}

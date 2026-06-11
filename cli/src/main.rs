//! couple — CLI over couple-core (§10). The portable artifact and test harness.

mod progress;

use clap::{Parser, Subcommand};
use couple_core::{CancelToken, Ctx, CtxConfig, TrackRole};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "couple", version, about = "Couple — sync external audio to camera video")]
struct Cli {
    /// Machine output: single JSON object on stdout (§10.1)
    #[arg(long, global = true)]
    json: bool,
    /// Cache directory (default: platform cache dir)
    #[arg(long, global = true)]
    cache_dir: Option<PathBuf>,
    /// Path to ffmpeg binary (or its directory); ffprobe resolved alongside
    #[arg(long, global = true)]
    ffmpeg: Option<PathBuf>,
    #[arg(long, global = true)]
    quiet: bool,
    /// NDJSON {phase, fraction} progress on stderr
    #[arg(long, global = true)]
    progress_json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Probe a media file (ffprobe → MediaInfo JSON)
    Probe { file: PathBuf },
    /// Import a file: decode, build peak pyramid + PCM cache + onsets
    Import {
        file: PathBuf,
        #[arg(long, default_value = "auto")]
        role: String,
    },
    /// Dump band() columns over a range; optional PNG for eyeballing
    Peaks {
        file: PathBuf,
        #[arg(long, default_value_t = 0.0, allow_negative_numbers = true)]
        t0: f64,
        /// Defaults to track end
        #[arg(long)]
        t1: Option<f64>,
        #[arg(long, default_value_t = 800)]
        columns: u32,
        #[arg(long)]
        png: Option<PathBuf>,
    },
    /// Query ranked onsets
    Onsets {
        file: PathBuf,
        /// Range query: --in t0 t1
        #[arg(long = "in", num_args = 2, value_names = ["T0", "T1"])]
        in_range: Option<Vec<f64>>,
        /// Snap query center: --near t
        #[arg(long)]
        near: Option<f64>,
        /// Half-span for --near (seconds)
        #[arg(long, default_value_t = 4.0)]
        span: f64,
        #[arg(long, default_value_t = 10)]
        limit: u32,
    },
    /// Cache maintenance
    Cache {
        #[command(subcommand)]
        cmd: CacheCmd,
    },
    /// Auto-sync a camera/external pair (§5). Roles: first arg = camera, second = external.
    Sync {
        video: PathBuf,
        audio: PathBuf,
        /// Stages 1–5 only; skip the drift pass
        #[arg(long)]
        offset_only: bool,
        /// Override SyncParams, e.g. --params z-min=25
        #[arg(long = "params", value_name = "K=V")]
        params: Vec<String>,
        /// Write a session file consumable by render/export
        #[arg(long)]
        write_state: Option<PathBuf>,
    },
    /// Measure the local lag at a video time (§5.4)
    Measure {
        video: PathBuf,
        audio: PathBuf,
        /// Window center, video time seconds
        #[arg(long)]
        at: f64,
        /// Lag search center in ms (default: 0)
        #[arg(long, default_value_t = 0.0, allow_negative_numbers = true)]
        center: f64,
        /// Lag search half-span in ms
        #[arg(long, default_value_t = 80.0)]
        span: f64,
        /// Window half-width in seconds
        #[arg(long, default_value_t = 9.0)]
        halfwin: f64,
    },
    /// Least-squares fit over (t, lagMs) points (§4.4)
    Fit {
        /// JSON file: [[t_s, lag_ms], ...]
        #[arg(long)]
        points: PathBuf,
    },
    /// Render the aligned external audio to a WAV on the video timeline (§8.5)
    Render {
        video: PathBuf,
        audio: PathBuf,
        /// Session file from `couple sync --write-state`
        #[arg(long)]
        state: PathBuf,
        #[arg(long)]
        out: PathBuf,
        /// Overwrite an existing output
        #[arg(short = 'f', long)]
        force: bool,
        #[arg(long)]
        allow_silence: bool,
    },
    /// Export the muxed video with aligned audio (§8.3)
    Export {
        video: PathBuf,
        audio: PathBuf,
        /// Session file from `couple sync --write-state`
        #[arg(long)]
        state: PathBuf,
        #[arg(long)]
        out: PathBuf,
        /// Keep the original camera audio as a second, non-default track
        #[arg(long)]
        keep_camera_audio: bool,
        /// auto | mov | mp4 (§8.4 matrix)
        #[arg(long, default_value = "auto")]
        container: String,
        /// +faststart (rewrites the file; off by default)
        #[arg(long)]
        faststart: bool,
        #[arg(long)]
        allow_silence: bool,
        /// Overwrite an existing output
        #[arg(short = 'f', long)]
        force: bool,
    },
    /// Session state tooling
    State {
        #[command(subcommand)]
        cmd: StateCmd,
    },
    /// Dev tools
    Fixture {
        #[command(subcommand)]
        cmd: FixtureCmd,
    },
}

#[derive(Subcommand)]
enum StateCmd {
    /// Validate a session file (§4.5 wrapper + invariants)
    Validate { file: PathBuf },
}

#[derive(Subcommand)]
enum FixtureCmd {
    /// Generate a synthetic fixture pair with known ground truth (§9.1)
    Gen {
        #[arg(long)]
        out: PathBuf,
        #[arg(long, default_value_t = 0.0, allow_negative_numbers = true)]
        offset_ms: f64,
        #[arg(long, default_value_t = 0.0, allow_negative_numbers = true)]
        ppm: f64,
        /// Derive offset so the overlap is this many seconds (external entering at the camera tail)
        #[arg(long, conflicts_with = "offset_ms")]
        overlap_s: Option<f64>,
        #[arg(long, default_value_t = 60.0)]
        dur_cam: f64,
        #[arg(long, default_value_t = 75.0)]
        dur_ext: f64,
        #[arg(long, default_value_t = 48000)]
        rate_cam: u32,
        #[arg(long, default_value_t = 48000)]
        rate_ext: u32,
        #[arg(long, default_value_t = 24)]
        bits: u32,
        /// Additive-noise SNR in dB (omit for no noise)
        #[arg(long, allow_negative_numbers = true)]
        snr_db: Option<f64>,
        #[arg(long, default_value_t = 0.0, allow_negative_numbers = true)]
        gain_db: f64,
        #[arg(long)]
        no_agc: bool,
        #[arg(long)]
        aac_roundtrip: bool,
        #[arg(long)]
        with_video: bool,
        /// Author an audio start_time epoch via -itsoffset remux (seconds)
        #[arg(long, default_value_t = 0.0)]
        itsoffset: f64,
        #[arg(long, default_value_t = 1)]
        seed: u64,
    },
}

#[derive(Subcommand)]
enum CacheCmd {
    Ls,
    Gc {
        #[arg(long, default_value_t = 20.0)]
        max_gb: f64,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let ctx = Ctx::new(CtxConfig {
        ffmpeg_path: cli.ffmpeg.clone(),
        ffprobe_path: cli.ffmpeg.clone(),
        cache_dir: cli.cache_dir.clone(),
    });
    let cancel = progress::install_sigint_cancel();

    match run(&cli, &ctx, &cancel) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            if cli.json {
                println!("{}", serde_json::json!({ "error": &e }));
            } else {
                eprintln!("error: {e}");
            }
            ExitCode::from(e.exit_code() as u8)
        }
    }
}

fn run(cli: &Cli, ctx: &Arc<Ctx>, cancel: &CancelToken) -> couple_core::Result<()> {
    match &cli.cmd {
        Cmd::Probe { file } => {
            let info = ctx.probe(file)?;
            emit(cli, &info)
        }
        Cmd::Import { file, role } => {
            let id = import_one(cli, ctx, file, role, cancel)?;
            let track = ctx.track(id)?;
            emit(cli, &track.meta)
        }
        Cmd::Peaks {
            file,
            t0,
            t1,
            columns,
            png,
        } => {
            let id = import_one(cli, ctx, file, "auto", cancel)?;
            let track = ctx.track(id)?;
            let t1 = t1.unwrap_or(track.meta.duration_s);
            let band = couple_core::band(&track.pyramid, *t0, t1, *columns)?;
            if let Some(png_path) = png {
                progress::render_band_png(ctx, &band, png_path)?;
                if !cli.quiet {
                    eprintln!("wrote {}", png_path.display());
                }
            }
            emit(cli, &band)
        }
        Cmd::Onsets {
            file,
            in_range,
            near,
            span,
            limit,
        } => {
            let id = import_one(cli, ctx, file, "auto", cancel)?;
            let track = ctx.track(id)?;
            let list = if let Some(r) = in_range {
                couple_core::onsets::in_range(&track.onsets, r[0], r[1], *limit as usize)
            } else if let Some(t) = near {
                couple_core::onsets::best_near(&track.onsets, *t, *span)
                    .into_iter()
                    .collect()
            } else {
                couple_core::onsets::in_range(&track.onsets, 0.0, track.meta.duration_s, *limit as usize)
            };
            emit(cli, &list)
        }
        Cmd::Sync {
            video,
            audio,
            offset_only,
            params,
            write_state,
        } => run_sync(cli, ctx, video, audio, *offset_only, params, write_state.as_deref(), cancel),
        Cmd::Measure {
            video,
            audio,
            at,
            center,
            span,
            halfwin,
        } => {
            let cam_id = import_one(cli, ctx, video, "camera", cancel)?;
            let ext_id = import_one(cli, ctx, audio, "external", cancel)?;
            let cam = ctx.track(cam_id)?;
            let ext = ctx.track(ext_id)?;
            let m = couple_core::measure::measure_local(
                &cam.pyramid,
                &ext.pyramid,
                *at,
                *halfwin,
                *center,
                *span,
            );
            emit(cli, &m)
        }
        Cmd::Fit { points } => {
            let raw = std::fs::read(points)
                .map_err(|e| couple_core::CoupleError::invalid(format!("read points: {e}")))?;
            let pts: Vec<(f64, f64)> = serde_json::from_slice(&raw)
                .map_err(|e| couple_core::CoupleError::invalid(format!("points JSON: {e}")))?;
            let fit = couple_core::fit::fit_ls(&pts);
            emit(cli, &fit)
        }
        Cmd::Render {
            video,
            audio,
            state,
            out,
            force,
            allow_silence,
        } => {
            let opts = couple_core::export::ExportOptions {
                kind: couple_core::export::ExportKind::Wav,
                output_path: out.clone(),
                keep_camera_audio: false,
                container: couple_core::export::Container::Auto,
                faststart: false,
                overwrite: *force,
                allow_silence: *allow_silence,
            };
            run_export(cli, ctx, video, audio, state, opts, cancel)
        }
        Cmd::Export {
            video,
            audio,
            state,
            out,
            keep_camera_audio,
            container,
            faststart,
            allow_silence,
            force,
        } => {
            let container = match container.as_str() {
                "auto" => couple_core::export::Container::Auto,
                "mov" => couple_core::export::Container::Mov,
                "mp4" => couple_core::export::Container::Mp4,
                other => {
                    return Err(couple_core::CoupleError::invalid(format!(
                        "--container must be auto|mov|mp4, got {other}"
                    )))
                }
            };
            let opts = couple_core::export::ExportOptions {
                kind: couple_core::export::ExportKind::Mux,
                output_path: out.clone(),
                keep_camera_audio: *keep_camera_audio,
                container,
                faststart: *faststart,
                overwrite: *force,
                allow_silence: *allow_silence,
            };
            run_export(cli, ctx, video, audio, state, opts, cancel)
        }
        Cmd::State { cmd } => match cmd {
            StateCmd::Validate { file } => {
                let raw = std::fs::read(file)
                    .map_err(|e| couple_core::CoupleError::invalid(format!("read state: {e}")))?;
                let parsed: couple_core::session::SessionFile = serde_json::from_slice(&raw)
                    .map_err(|e| couple_core::CoupleError::invalid(format!("state JSON: {e}")))?;
                if parsed.schema != couple_core::session::SESSION_SCHEMA {
                    return Err(couple_core::CoupleError::invalid(format!(
                        "unknown schema {}",
                        parsed.schema
                    )));
                }
                couple_core::session::validate_state(&parsed.alignment)?;
                emit(cli, &serde_json::json!({ "valid": true }))
            }
        },
        Cmd::Fixture { cmd } => match cmd {
            FixtureCmd::Gen {
                out,
                offset_ms,
                ppm,
                overlap_s,
                dur_cam,
                dur_ext,
                rate_cam,
                rate_ext,
                bits,
                snr_db,
                gain_db,
                no_agc,
                aac_roundtrip,
                with_video,
                itsoffset,
                seed,
            } => {
                let delta = 1.0 + ppm * 1e-6;
                let offset_ms = match overlap_s {
                    Some(w) => -(dur_cam - w) * delta * 1000.0,
                    None => *offset_ms,
                };
                let spec = couple_core::fixture::FixtureSpec {
                    offset_ms,
                    ppm: *ppm,
                    dur_cam_s: *dur_cam,
                    dur_ext_s: *dur_ext,
                    rate_cam: *rate_cam,
                    rate_ext: *rate_ext,
                    bits_ext: *bits,
                    snr_db: *snr_db,
                    gain_db: *gain_db,
                    agc: !no_agc,
                    aac_roundtrip: *aac_roundtrip,
                    with_video: *with_video,
                    itsoffset_s: *itsoffset,
                    seed: *seed,
                };
                let truth = couple_core::fixture::generate(out, &spec, &ctx.ffmpeg()?)?;
                emit(cli, &truth)
            }
        },
        Cmd::Cache { cmd } => match cmd {
            CacheCmd::Ls => {
                let entries = couple_core::cache::list_entries(ctx.cache_root());
                emit(cli, &entries)
            }
            CacheCmd::Gc { max_gb } => {
                let freed = ctx.cache_gc((max_gb * 1e9) as u64);
                emit(cli, &serde_json::json!({ "freedBytes": freed }))
            }
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn run_sync(
    cli: &Cli,
    ctx: &Arc<Ctx>,
    video: &PathBuf,
    audio: &PathBuf,
    offset_only: bool,
    params: &[String],
    write_state: Option<&std::path::Path>,
    cancel: &CancelToken,
) -> couple_core::Result<()> {
    let mut sp = couple_core::sync::SyncParams::default();
    for kv in params {
        let (k, v) = kv
            .split_once('=')
            .ok_or_else(|| couple_core::CoupleError::invalid("--params expects k=v"))?;
        sp.set(k, v)?;
    }
    // Roles by position (§10.2): import implied, cache-hit fast.
    let cam_id = import_one(cli, ctx, video, "camera", cancel)?;
    let ext_id = import_one(cli, ctx, audio, "external", cancel)?;
    let cam = ctx.track(cam_id)?;
    let ext = ctx.track(ext_id)?;

    let reporter = progress::Reporter::new(cli.quiet, cli.progress_json, video);
    let pf = reporter.progress_fn();
    let mut prog = couple_core::job::ProgressEmitter::new(ctx.next_id(), pf);
    let result = if offset_only {
        couple_core::sync::offset_only_sync(&cam.pyramid, &ext.pyramid, &sp, &mut prog, cancel)?
    } else {
        couple_core::sync::auto_sync_tracks(&cam.pyramid, &ext.pyramid, &sp, &mut prog, cancel)?
    };
    reporter.finish();

    if let Some(state_path) = write_state {
        write_session_file(&cam.meta, &ext.meta, &result, state_path)?;
    }
    emit(cli, &result)?;
    // §10.3: 5 = sync failed (NoPlausiblePeak), 6 = insufficient overlap.
    if let couple_core::sync::SyncStatus::Failed(reason) = &result.status {
        let code = match reason {
            couple_core::sync::FailReason::NoPlausiblePeak { .. } => 5,
            couple_core::sync::FailReason::InsufficientOverlap { .. } => 6,
        };
        std::process::exit(code);
    }
    Ok(())
}

fn run_export(
    cli: &Cli,
    ctx: &Arc<Ctx>,
    video: &PathBuf,
    audio: &PathBuf,
    state: &PathBuf,
    opts: couple_core::export::ExportOptions,
    cancel: &CancelToken,
) -> couple_core::Result<()> {
    let raw = std::fs::read(state)
        .map_err(|e| couple_core::CoupleError::invalid(format!("read state: {e}")))?;
    let session: couple_core::session::SessionFile = serde_json::from_slice(&raw)
        .map_err(|e| couple_core::CoupleError::invalid(format!("state JSON: {e}")))?;
    couple_core::session::validate_state(&session.alignment)?;
    let a = &session.alignment;
    let mapping = couple_core::export::ExportMapping {
        offset_ms: a.offset_ms,
        drift_ppm: a.drift_ppm,
        resample: a.drift_mode == couple_core::session::DriftMode::Resample,
    };
    let reporter = progress::Reporter::new(cli.quiet, cli.progress_json, &opts.output_path);
    let pf = reporter.progress_fn();
    let mut prog = couple_core::job::ProgressEmitter::new(ctx.next_id(), pf);
    let report = couple_core::export::export(
        &ctx.ffmpeg()?,
        &ctx.ffprobe()?,
        video,
        audio,
        &mapping,
        &opts,
        &mut prog,
        cancel,
    )?;
    reporter.finish();
    emit(cli, &report)
}

fn write_session_file(
    cam: &couple_core::TrackMeta,
    ext: &couple_core::TrackMeta,
    result: &couple_core::sync::SyncResult,
    path: &std::path::Path,
) -> couple_core::Result<()> {
    use couple_core::session::*;
    let mut alignment = AlignmentState::default();
    if !matches!(result.status, couple_core::sync::SyncStatus::Failed(_)) {
        alignment.offset_ms = result.offset_ms;
        alignment.saved_drift_ppm = result.drift_ppm;
        alignment.drift_mode = result.suggested_drift_mode;
        alignment.drift_ppm = match result.suggested_drift_mode {
            DriftMode::Resample => result.drift_ppm,
            DriftMode::None => 0.0,
        };
        alignment.drift_windows = result
            .windows
            .iter()
            .filter(|w| w.accepted)
            .map(|w| DriftWindowState {
                t: w.t,
                lag_ms: w.lag_ms,
                ncc: w.ncc,
            })
            .collect();
    }
    let file = SessionFile {
        schema: SESSION_SCHEMA.into(),
        saved_at: chrono_like_now(),
        camera: SessionFileTrack {
            path: cam.info.path.clone(),
            content_key: cam.content_key.clone(),
            duration_s: cam.duration_s,
        },
        external: SessionFileTrack {
            path: ext.info.path.clone(),
            content_key: ext.content_key.clone(),
            duration_s: ext.duration_s,
        },
        alignment,
    };
    std::fs::write(path, serde_json::to_vec_pretty(&file).unwrap())
        .map_err(|e| couple_core::CoupleError::internal(format!("write state: {e}")))?;
    Ok(())
}

/// ISO 8601 UTC without a chrono dependency.
fn chrono_like_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    // civil_from_days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y,
        m,
        d,
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60
    )
}

fn parse_role(role: &str, info: &couple_core::MediaInfo) -> couple_core::Result<TrackRole> {
    match role {
        "camera" => Ok(TrackRole::Camera),
        "external" => Ok(TrackRole::External),
        "auto" => Ok(couple_core::ffmpeg::classify(info)),
        other => Err(couple_core::CoupleError::invalid(format!(
            "role must be camera|external|auto, got {other}"
        ))),
    }
}

fn import_one(
    cli: &Cli,
    ctx: &Arc<Ctx>,
    file: &PathBuf,
    role: &str,
    cancel: &CancelToken,
) -> couple_core::Result<couple_core::TrackId> {
    let info = ctx.probe(file)?;
    let role = parse_role(role, &info)?;
    let reporter = progress::Reporter::new(cli.quiet, cli.progress_json, file);
    let job = ctx.next_id();
    let id = ctx.import_blocking(file, role, reporter.progress_fn(), cancel, job)?;
    reporter.finish();
    Ok(id)
}

fn emit<T: serde::Serialize>(cli: &Cli, value: &T) -> couple_core::Result<()> {
    if cli.json {
        println!("{}", serde_json::to_string(value).expect("serialize"));
    } else {
        println!("{}", serde_json::to_string_pretty(value).expect("serialize"));
    }
    Ok(())
}

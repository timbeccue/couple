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
        #[arg(long, default_value_t = 0.0)]
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

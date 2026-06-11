//! Synthetic fixture generator (§9.1, dev subcommand `couple fixture gen`).
//!
//! Deterministic (ChaCha8). Internal master signal on a 96 kHz grid: pink-noise bed at
//! −38 dBFS + Poisson transients (~0.5/s, band-passed attacks, exp decay τ 80–400 ms) +
//! occasional vibrato tones 2–6 s (periodic-content hazard) + multi-second silences.
//! External = master sampled at rate_ext. Camera = master warped through the ground-truth
//! mapping u(t), then degraded (gain, AGC limiter 5 ms/200 ms @ −12 dBFS, 16 kHz LP,
//! noise to target SNR, optional AAC round-trip / mpeg4 video mux / -itsoffset epoch).
//! The master timeline IS the external timeline; camera hears master(u(t)).

use crate::error::{CoupleError, Result};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

const GRID_RATE: f64 = 96_000.0;
const SINC_HALF_TAPS: i64 = 16; // 32-tap windowed sinc

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixtureSpec {
    pub offset_ms: f64,
    pub ppm: f64,
    pub dur_cam_s: f64,
    pub dur_ext_s: f64,
    pub rate_cam: u32,
    pub rate_ext: u32,
    pub bits_ext: u32,
    /// None = no additive noise.
    pub snr_db: Option<f64>,
    pub gain_db: f64,
    pub agc: bool,
    pub aac_roundtrip: bool,
    pub with_video: bool,
    /// Nonzero authors an audio start_time epoch via `-itsoffset … -c copy` remux (F1).
    pub itsoffset_s: f64,
    pub seed: u64,
}

impl Default for FixtureSpec {
    fn default() -> Self {
        Self {
            offset_ms: 0.0,
            ppm: 0.0,
            dur_cam_s: 60.0,
            dur_ext_s: 75.0,
            rate_cam: 48_000,
            rate_ext: 48_000,
            bits_ext: 24,
            snr_db: Some(20.0),
            gain_db: 0.0,
            agc: true,
            aac_roundtrip: false,
            with_video: false,
            itsoffset_s: 0.0,
            seed: 1,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixtureTruth {
    pub spec: FixtureSpec,
    pub camera_file: String,
    pub external_file: String,
    /// Overlap on the camera/video timeline implied by the mapping.
    pub overlap_t0: f64,
    pub overlap_t1: f64,
    pub overlap_s: f64,
}

/// u(t) = t·(1 + ppm·1e−6) + offset_ms/1000 (UX §2.1) — external time of video time t.
fn u_of_t(t: f64, offset_ms: f64, ppm: f64) -> f64 {
    t * (1.0 + ppm * 1e-6) + offset_ms / 1000.0
}

// ---------- master signal ----------

/// Pink noise: Paul Kellett's 3-pole filter over ChaCha8 white noise.
struct Pink {
    rng: ChaCha8Rng,
    b: [f64; 3],
}

impl Pink {
    fn new(seed: u64) -> Self {
        Self {
            rng: ChaCha8Rng::seed_from_u64(seed),
            b: [0.0; 3],
        }
    }
    fn next(&mut self) -> f64 {
        let white: f64 = self.rng.gen_range(-1.0..1.0);
        self.b[0] = 0.99765 * self.b[0] + white * 0.0990460;
        self.b[1] = 0.96300 * self.b[1] + white * 0.2965164;
        self.b[2] = 0.57000 * self.b[2] + white * 1.0526913;
        (self.b[0] + self.b[1] + self.b[2] + white * 0.1848) * 0.18
    }
}

/// Band-passed broadband attack: 8 partials in 800–6000 Hz with random phases.
/// Broadband fine structure makes the bin-span envelope track the energy envelope at
/// sub-ms resolution (a pure sine burst's span profile is too smooth to time).
#[derive(Debug, Clone, Copy)]
struct Transient {
    t0: f64,
    freqs: [f64; 8],
    phases: [f64; 8],
    amp: f64,
    tau: f64,
}

#[derive(Debug, Clone, Copy)]
struct Tone {
    t0: f64,
    dur: f64,
    f0: f64,
    fv: f64, // vibrato rate
    depth: f64,
    amp: f64,
}

#[derive(Debug, Clone, Copy)]
struct Silence {
    t0: f64,
    dur: f64,
}

fn poisson_times(rng: &mut ChaCha8Rng, u_min: f64, u_max: f64, rate_per_s: f64) -> Vec<f64> {
    let mut out = Vec::new();
    let mut t = u_min;
    loop {
        let gap = -(rng.gen_range(1e-12..1.0f64)).ln() / rate_per_s;
        t += gap;
        if t >= u_max {
            break;
        }
        out.push(t);
    }
    out
}

/// Deterministic event plan over the master domain [u_min, u_max].
struct Plan {
    transients: Vec<Transient>,
    tones: Vec<Tone>,
    silences: Vec<Silence>,
}

fn plan(seed: u64, u_min: f64, u_max: f64) -> Plan {
    let mut rt = ChaCha8Rng::seed_from_u64(seed.wrapping_add(0x7261_6e74)); // transients
    let transients = poisson_times(&mut rt, u_min, u_max, 0.5)
        .into_iter()
        .map(|t0| {
            let mut freqs = [0f64; 8];
            let mut phases = [0f64; 8];
            for k in 0..8 {
                freqs[k] = rt.gen_range(800.0..6000.0);
                phases[k] = rt.gen_range(0.0..std::f64::consts::TAU);
            }
            Transient {
                t0,
                freqs,
                phases,
                amp: rt.gen_range(0.25..0.9),
                tau: rt.gen_range(0.080..0.400),
            }
        })
        .collect();

    let mut rn = ChaCha8Rng::seed_from_u64(seed.wrapping_add(0x746f_6e65)); // tones
    let tones = poisson_times(&mut rn, u_min, u_max, 1.0 / 40.0)
        .into_iter()
        .map(|t0| Tone {
            t0,
            dur: rn.gen_range(2.0..6.0),
            f0: rn.gen_range(200.0..2000.0),
            fv: rn.gen_range(5.0..7.0),
            depth: 0.03,
            amp: rn.gen_range(0.08..0.2),
        })
        .collect();

    let mut rs = ChaCha8Rng::seed_from_u64(seed.wrapping_add(0x7369_6c65)); // silences
    let silences = poisson_times(&mut rs, u_min, u_max, 1.0 / 75.0)
        .into_iter()
        .map(|t0| Silence {
            t0,
            dur: rs.gen_range(2.0..8.0),
        })
        .collect();

    Plan {
        transients,
        tones,
        silences,
    }
}

/// Streaming master-on-grid generator with a sliding window buffer for interpolation.
/// Both output passes regenerate the master identically (deterministic components).
struct Master {
    plan: Plan,
    pink: Pink,
    /// grid sample index of buf[0]
    base: i64,
    /// next grid index to generate
    next: i64,
    buf: Vec<f64>,
    t_idx: usize, // first possibly-active transient
    tone_idx: usize,
    sil_idx: usize,
}

impl Master {
    fn new(seed: u64, u_min: f64, u_max: f64) -> Self {
        let start = (u_min * GRID_RATE).floor() as i64 - 2 * SINC_HALF_TAPS;
        Self {
            plan: plan(seed, u_min - 1.0, u_max + 1.0),
            pink: Pink::new(seed.wrapping_add(0x70_696e_6b)),
            base: start,
            next: start,
            buf: Vec::new(),
            t_idx: 0,
            tone_idx: 0,
            sil_idx: 0,
        }
    }

    fn gen_one(&mut self, n: i64) -> f64 {
        let u = n as f64 / GRID_RATE;
        let bed_amp = 10f64.powf(-38.0 / 20.0) * 2.0; // ≈ −38 dBFS RMS bed
        let mut s = self.pink.next() * bed_amp;

        // transients: active window (τ ≤ 0.4 s → audible ≤ ~3 s)
        while self.t_idx < self.plan.transients.len()
            && self.plan.transients[self.t_idx].t0 < u - 3.0
        {
            self.t_idx += 1;
        }
        for e in self.plan.transients[self.t_idx..]
            .iter()
            .take_while(|e| e.t0 <= u)
        {
            let dt = u - e.t0;
            let attack = 1.0 - (-dt / 0.0005).exp();
            let env = e.amp * attack * (-dt / e.tau).exp();
            let mut burst = 0.0;
            for k in 0..8 {
                burst += (std::f64::consts::TAU * e.freqs[k] * dt + e.phases[k]).sin();
            }
            s += env * burst * (1.0 / (8f64).sqrt());
        }

        // vibrato tones (analytic FM phase integral — stateless, two-pass identical)
        while self.tone_idx < self.plan.tones.len()
            && self.plan.tones[self.tone_idx].t0 + self.plan.tones[self.tone_idx].dur < u - 0.5
        {
            self.tone_idx += 1;
        }
        for tone in self.plan.tones[self.tone_idx..]
            .iter()
            .take_while(|e| e.t0 <= u)
        {
            let dt = u - tone.t0;
            if dt > tone.dur {
                continue;
            }
            let env = (dt / 0.2).min(1.0).min(((tone.dur - dt) / 0.3).max(0.0)).clamp(0.0, 1.0);
            let tau = std::f64::consts::TAU;
            let phase = tau
                * (tone.f0 * dt
                    + tone.f0 * tone.depth / (tau * tone.fv) * (1.0 - (tau * tone.fv * dt).cos()));
            s += tone.amp * env * phase.sin();
        }

        // silences: raised-cosine gate (50 ms edges)
        while self.sil_idx < self.plan.silences.len()
            && self.plan.silences[self.sil_idx].t0 + self.plan.silences[self.sil_idx].dur + 0.1 < u
        {
            self.sil_idx += 1;
        }
        for sil in self.plan.silences[self.sil_idx..]
            .iter()
            .take_while(|e| e.t0 - 0.05 <= u)
        {
            let edge = 0.05;
            let g = if u < sil.t0 || u > sil.t0 + sil.dur {
                1.0
            } else if u < sil.t0 + edge {
                let x = (u - sil.t0) / edge;
                0.5 * (1.0 + (std::f64::consts::PI * x).cos())
            } else if u > sil.t0 + sil.dur - edge {
                let x = (sil.t0 + sil.dur - u) / edge;
                0.5 * (1.0 + (std::f64::consts::PI * x).cos())
            } else {
                0.0
            };
            s *= g;
        }
        s
    }

    /// Ensure grid samples up to index `hi` exist; drop below `lo`.
    fn ensure(&mut self, lo: i64, hi: i64) {
        while self.next <= hi {
            let n = self.next;
            let v = self.gen_one(n);
            self.buf.push(v);
            self.next += 1;
        }
        let drop = (lo - self.base).max(0) as usize;
        if drop > 4096 {
            self.buf.drain(..drop);
            self.base += drop as i64;
        }
    }

    /// Band-limited read at arbitrary time u: 32-tap Blackman-windowed sinc via a
    /// precomputed polyphase table (linear phase interpolation).
    fn sample(&mut self, u: f64) -> f64 {
        let x = u * GRID_RATE;
        let xi = x.floor() as i64;
        // Exact-grid fast path (external at rates dividing 96 kHz).
        if (x - xi as f64).abs() < 1e-9 {
            self.ensure(xi - 4 * SINC_HALF_TAPS, xi + 1);
            return self.buf[(xi - self.base) as usize];
        }
        let frac = x - xi as f64;
        let k0 = xi - SINC_HALF_TAPS + 1;
        self.ensure(k0 - 4 * SINC_HALF_TAPS, xi + SINC_HALF_TAPS);
        let table = sinc_table();
        let p = frac * SINC_PHASES as f64;
        let pi0 = p.floor() as usize;
        let pf = p - pi0 as f64;
        let row0 = &table[pi0 * TAPS..(pi0 + 1) * TAPS];
        let row1 = &table[(pi0 + 1) * TAPS..(pi0 + 2) * TAPS];
        let base_idx = (k0 - self.base) as usize;
        let src = &self.buf[base_idx..base_idx + TAPS];
        let mut acc = 0.0;
        for j in 0..TAPS {
            let h = row0[j] + (row1[j] - row0[j]) * pf;
            acc += src[j] * h;
        }
        acc
    }
}

const TAPS: usize = (2 * SINC_HALF_TAPS) as usize;
const SINC_PHASES: usize = 1024;

/// H[p][j] = windowed-sinc evaluated at d = (frac + HALF−1) − j for frac = p/PHASES,
/// where j indexes taps k0..k0+TAPS and d = x − k.
fn sinc_table() -> &'static [f64] {
    use std::sync::OnceLock;
    static TABLE: OnceLock<Vec<f64>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t = vec![0f64; (SINC_PHASES + 2) * TAPS];
        for p in 0..=SINC_PHASES + 1 {
            let frac = p as f64 / SINC_PHASES as f64;
            for j in 0..TAPS {
                let d = (frac + (SINC_HALF_TAPS - 1) as f64) - j as f64;
                let sinc = if d.abs() < 1e-12 {
                    1.0
                } else {
                    (std::f64::consts::PI * d).sin() / (std::f64::consts::PI * d)
                };
                let w_arg = (d / SINC_HALF_TAPS as f64).clamp(-1.0, 1.0);
                let w = 0.42
                    + 0.5 * (std::f64::consts::PI * w_arg).cos()
                    + 0.08 * (2.0 * std::f64::consts::PI * w_arg).cos();
                t[p * TAPS + j] = sinc * w;
            }
        }
        t
    })
}

// ---------- camera degradation chain ----------

/// 2nd-order Butterworth low-pass biquad.
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: f64,
    z2: f64,
}

impl Biquad {
    fn lowpass(fc: f64, fs: f64) -> Self {
        let k = (std::f64::consts::PI * fc / fs).tan();
        let q = std::f64::consts::FRAC_1_SQRT_2;
        let norm = 1.0 / (1.0 + k / q + k * k);
        Self {
            b0: k * k * norm,
            b1: 2.0 * k * k * norm,
            b2: k * k * norm,
            a1: 2.0 * (k * k - 1.0) * norm,
            a2: (1.0 - k / q + k * k) * norm,
            z1: 0.0,
            z2: 0.0,
        }
    }
    fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// Feedback limiter: 5 ms attack / 200 ms release, threshold −12 dBFS (§9.1).
struct Agc {
    threshold: f64,
    attack: f64,
    release: f64,
    env: f64,
    gain: f64,
}

impl Agc {
    fn new(fs: f64) -> Self {
        Self {
            threshold: 10f64.powf(-12.0 / 20.0),
            attack: (-1.0 / (0.005 * fs)).exp(),
            release: (-1.0 / (0.200 * fs)).exp(),
            env: 0.0,
            gain: 1.0,
        }
    }
    fn process(&mut self, x: f64) -> f64 {
        let y = x * self.gain; // feedback: detector sees the output
        let a = y.abs();
        self.env = if a > self.env {
            self.attack * self.env + (1.0 - self.attack) * a
        } else {
            self.release * self.env + (1.0 - self.release) * a
        };
        let desired = if self.env > self.threshold {
            self.threshold / self.env
        } else {
            1.0
        };
        let coef = if desired < self.gain { self.attack } else { self.release };
        self.gain = coef * self.gain + (1.0 - coef) * desired;
        y
    }
}

// ---------- output writing ----------

fn spawn_wav_writer(
    ffmpeg: &Path,
    rate: u32,
    codec: &str,
    out: &Path,
) -> Result<std::process::Child> {
    Command::new(ffmpeg)
        .args(["-nostdin", "-v", "error", "-y", "-f", "f64le", "-ar"])
        .arg(rate.to_string())
        .args(["-ac", "1", "-i", "pipe:0", "-c:a", codec])
        .arg(out)
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| CoupleError::internal(format!("spawn ffmpeg writer: {e}")))
}

fn finish_child(mut child: std::process::Child, what: &str) -> Result<()> {
    let st = child
        .wait()
        .map_err(|e| CoupleError::internal(format!("wait {what}: {e}")))?;
    if !st.success() {
        return Err(CoupleError::FfmpegFailed {
            stage: format!("fixture {what}"),
            exit_code: st.code(),
            stderr_tail: String::new(),
        });
    }
    Ok(())
}

fn push_f64(w: &mut impl Write, v: f64) -> Result<()> {
    w.write_all(&v.to_le_bytes())
        .map_err(|e| CoupleError::internal(format!("fixture pipe: {e}")))
}

/// Generate a fixture pair + truth.json into `out_dir`. Returns the truth.
pub fn generate(out_dir: &Path, spec: &FixtureSpec, ffmpeg: &Path) -> Result<FixtureTruth> {
    std::fs::create_dir_all(out_dir)
        .map_err(|e| CoupleError::internal(format!("fixture out dir: {e}")))?;
    let delta = 1.0 + spec.ppm * 1e-6;
    let off_s = spec.offset_ms / 1000.0;
    let u_at = |t: f64| u_of_t(t, spec.offset_ms, spec.ppm);
    let u_min = 0f64.min(u_at(0.0));
    let u_max = spec.dur_ext_s.max(u_at(spec.dur_cam_s));

    // ---- pass 1: external = master sampled at rate_ext ----
    {
        let mut master = Master::new(spec.seed, u_min, u_max);
        let codec = if spec.bits_ext <= 16 { "pcm_s16le" } else { "pcm_s24le" };
        let ext_path = out_dir.join("external.wav");
        let mut child = spawn_wav_writer(ffmpeg, spec.rate_ext, codec, &ext_path)?;
        let mut stdin = std::io::BufWriter::with_capacity(1 << 20, child.stdin.take().unwrap());
        let n = (spec.dur_ext_s * spec.rate_ext as f64).round() as u64;
        for k in 0..n {
            let v = master.sample(k as f64 / spec.rate_ext as f64);
            push_f64(&mut stdin, v.clamp(-0.999, 0.999))?;
        }
        drop(stdin);
        finish_child(child, "external")?;
    }

    // ---- pass 2: camera = degraded master(u(t)), via temp raw for exact SNR ----
    let cam_rate = spec.rate_cam as f64;
    let n_cam = (spec.dur_cam_s * cam_rate).round() as u64;
    let tmp_raw = out_dir.join("camera.tmp.f64");
    let signal_rms;
    {
        let mut master = Master::new(spec.seed, u_min, u_max);
        let gain = 10f64.powf(spec.gain_db / 20.0);
        let mut lp = Biquad::lowpass(16_000.0_f64.min(cam_rate * 0.45), cam_rate);
        let mut agc = spec.agc.then(|| Agc::new(cam_rate));
        let mut f = std::io::BufWriter::with_capacity(
            1 << 20,
            std::fs::File::create(&tmp_raw)
                .map_err(|e| CoupleError::internal(format!("camera tmp: {e}")))?,
        );
        let mut sum_sq = 0f64;
        for nidx in 0..n_cam {
            let t = nidx as f64 / cam_rate;
            let mut s = master.sample(u_at(t)) * gain;
            s = lp.process(s);
            if let Some(a) = &mut agc {
                s = a.process(s);
            }
            sum_sq += s * s;
            push_f64(&mut f, s)?;
        }
        f.flush().map_err(|e| CoupleError::internal(format!("camera tmp flush: {e}")))?;
        signal_rms = (sum_sq / n_cam.max(1) as f64).sqrt();
    }

    // Noise to target SNR (white, deterministic), then encode.
    let noise_amp = spec
        .snr_db
        .map(|snr| signal_rms / 10f64.powf(snr / 20.0))
        .unwrap_or(0.0);
    let (cam_name, plain_cam_path) = if spec.with_video {
        ("camera.mp4", out_dir.join("camera.mp4"))
    } else if spec.aac_roundtrip {
        ("camera.m4a", out_dir.join("camera.m4a"))
    } else {
        ("camera.wav", out_dir.join("camera.wav"))
    };
    {
        let mut child = if spec.with_video {
            // mpeg4-encoded testsrc2 video (F12: no x264 in the LGPL build).
            Command::new(ffmpeg)
                .args(["-nostdin", "-v", "error", "-y", "-f", "f64le"])
                .args(["-ar", &spec.rate_cam.to_string(), "-ac", "1", "-i", "pipe:0"])
                .args(["-f", "lavfi", "-i"])
                .arg(format!("testsrc2=size=320x240:rate=30:duration={}", spec.dur_cam_s))
                .args(["-map", "1:v:0", "-map", "0:a:0", "-c:v", "mpeg4", "-q:v", "10"])
                .args(["-c:a", "aac", "-b:a", "256k", "-shortest"])
                .arg(&plain_cam_path)
                .stdin(Stdio::piped())
                .spawn()
                .map_err(|e| CoupleError::internal(format!("spawn camera mux: {e}")))?
        } else {
            let codec = if spec.aac_roundtrip { "aac" } else { "pcm_s16le" };
            let mut cmd = Command::new(ffmpeg);
            cmd.args(["-nostdin", "-v", "error", "-y", "-f", "f64le"])
                .args(["-ar", &spec.rate_cam.to_string(), "-ac", "1", "-i", "pipe:0"])
                .args(["-c:a", codec]);
            if spec.aac_roundtrip {
                cmd.args(["-b:a", "256k"]);
            }
            cmd.arg(&plain_cam_path);
            cmd.stdin(Stdio::piped())
                .spawn()
                .map_err(|e| CoupleError::internal(format!("spawn camera writer: {e}")))?
        };
        let mut stdin = std::io::BufWriter::with_capacity(1 << 20, child.stdin.take().unwrap());
        let mut nrng = ChaCha8Rng::seed_from_u64(spec.seed.wrapping_add(0x6e6f_6973));
        let raw = std::fs::File::open(&tmp_raw)
            .map_err(|e| CoupleError::internal(format!("camera tmp open: {e}")))?;
        let mut rdr = std::io::BufReader::with_capacity(1 << 20, raw);
        let mut bytes = [0u8; 8];
        use std::io::Read;
        loop {
            match rdr.read_exact(&mut bytes) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(CoupleError::internal(format!("camera tmp read: {e}"))),
            }
            let mut v = f64::from_le_bytes(bytes);
            if noise_amp > 0.0 {
                v += nrng.gen_range(-1.0f64..1.0) * noise_amp * 1.732; // uniform → RMS≈amp
            }
            push_f64(&mut stdin, v.clamp(-0.999, 0.999))?;
        }
        drop(stdin);
        finish_child(child, "camera")?;
        let _ = std::fs::remove_file(&tmp_raw);
    }

    // Optional epoch remux (F1 fixture): audio start_time via -itsoffset … -c copy.
    let camera_file = if spec.itsoffset_s != 0.0 {
        let shifted = if spec.with_video {
            out_dir.join("camera_epoch.mp4")
        } else {
            out_dir.join("camera_epoch.mov")
        };
        let st = if spec.with_video {
            Command::new(ffmpeg)
                .args(["-nostdin", "-v", "error", "-y", "-i"])
                .arg(&plain_cam_path)
                .args(["-itsoffset", &spec.itsoffset_s.to_string(), "-i"])
                .arg(&plain_cam_path)
                .args(["-map", "0:v:0", "-map", "1:a:0", "-c", "copy"])
                .arg(&shifted)
                .status()
        } else {
            Command::new(ffmpeg)
                .args(["-nostdin", "-v", "error", "-y", "-itsoffset"])
                .arg(spec.itsoffset_s.to_string())
                .arg("-i")
                .arg(&plain_cam_path)
                .args(["-c", "copy"])
                .arg(&shifted)
                .status()
        }
        .map_err(|e| CoupleError::internal(format!("epoch remux: {e}")))?;
        if !st.success() {
            return Err(CoupleError::FfmpegFailed {
                stage: "fixture epoch remux".into(),
                exit_code: st.code(),
                stderr_tail: String::new(),
            });
        }
        shifted.file_name().unwrap().to_string_lossy().to_string()
    } else {
        cam_name.to_string()
    };

    // Overlap on the camera timeline.
    let t_ext_start = (0.0 - off_s) / delta;
    let t_ext_end = (spec.dur_ext_s - off_s) / delta;
    let overlap_t0 = t_ext_start.max(0.0);
    let overlap_t1 = t_ext_end.min(spec.dur_cam_s);
    let truth = FixtureTruth {
        spec: spec.clone(),
        camera_file,
        external_file: "external.wav".into(),
        overlap_t0,
        overlap_t1,
        overlap_s: (overlap_t1 - overlap_t0).max(0.0),
    };
    std::fs::write(
        out_dir.join("truth.json"),
        serde_json::to_vec_pretty(&truth).unwrap(),
    )
    .map_err(|e| CoupleError::internal(format!("truth.json: {e}")))?;
    Ok(truth)
}

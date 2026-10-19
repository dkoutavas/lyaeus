//! euios: guitar feedback at bedroom volume.
//!
//! Real feedback is the speaker shaking the string at one of its modes
//! until the loop gain passes one; a harmonic swells in and takes over.
//! There is no acoustic loop inside a plugin chain, so this follows the
//! DigiTech FreqOut idea instead: detect the pitch of the note being held,
//! wait an Onset time, then swell in a sine at a chosen harmonic of it and
//! let the distortion downstream (bromios, lyaeus) do what a loud room
//! would. A fed-back string mode is close to a sine anyway, and the amp
//! adds the rest.
//!
//! The generator only runs while a note is held, so there is no idle hiss
//! and nothing for a gate to do. Once feedback is established it keeps
//! going until the strings are muted, like the real thing: the threshold to
//! start is higher than the threshold to stop.
//!
//! Put it first in the chain, on the clean guitar, where pitch is easy to
//! read.

use nih_plug::prelude::*;
use std::f64::consts::PI;
use std::sync::Arc;

use crate::filters::OnePole;

/// A note has to sit above this level to start the Onset clock.
const START_DB: f32 = -45.0;
/// Established feedback only stops once the input falls below this: a
/// muted string, not a decaying one.
const STOP_DB: f32 = -65.0;
/// How long the detector may lose the pitch (pick attacks, string noise)
/// before the note counts as gone.
const GRACE_MS: f32 = 120.0;
/// A pitch change beyond this is a new note: a semitone is 5.9 %.
const NEW_NOTE_RATIO: f32 = 0.06;
/// Feedback fades over this after the strings are muted.
const RELEASE_MS: f32 = 300.0;
/// The envelope runs in dB from this floor up to 0: exponential growth in
/// amplitude, which is how a feedback loop with gain over one behaves.
const FLOOR_DB: f32 = -60.0;
/// Sine level at Gain = 100 %, in full scale: about a hot pickup.
const MAX_LEVEL: f32 = 0.3;
/// Highest feedback tone. Higher harmonics of high notes fold down to this.
const MAX_FREQ_HZ: f32 = 4000.0;
/// Wobble at 100 %: a 5.5 Hz vibrato of 8 cents and a 0.37 Hz drift of 4.
const WOBBLE_FAST_HZ: f64 = 5.5;
const WOBBLE_FAST_CENTS: f64 = 8.0;
const WOBBLE_SLOW_HZ: f64 = 0.37;
const WOBBLE_SLOW_CENTS: f64 = 4.0;

/// YIN pitch detector (de Cheveigne and Kawahara, 2002).
///
/// For each lag tau it takes the difference function
/// `d(tau) = sum_j (x[j] - x[j + tau])^2` over a window: zero when the
/// signal repeats itself after tau samples. Plain autocorrelation would
/// also peak at multiples of the period and at lag zero; YIN's cumulative
/// mean normalisation `d'(tau) = d(tau) * tau / sum_{j<=tau} d(j)` fixes
/// lag zero to 1 and makes the first real dip stand out, so the detector
/// takes the first dip under a threshold rather than the deepest one. That
/// is what avoids octave errors. A parabola through the three points around
/// the dip gives the period to a fraction of a sample.
///
/// ponytail: the whole analysis runs in one go every HOP samples, on the
/// audio thread. About 0.7 M multiply-adds, fine at 64-sample buffers on
/// this machine; spread it over blocks if it ever glitches.
pub struct Yin {
    sample_rate: f32,
    tau_min: usize,
    tau_max: usize,
    /// Ring of the last WINDOW + tau_max samples.
    ring: Vec<f32>,
    write: usize,
    since_analysis: usize,
    frame: Vec<f32>,
    diff: Vec<f32>,
    pitch: Option<f32>,
}

const WINDOW: usize = 1024;
const HOP: usize = 1024;
const YIN_THRESHOLD: f32 = 0.15;
/// Deepest dip above this means no clear period: unvoiced.
const YIN_UNVOICED: f32 = 0.5;
const PITCH_MIN_HZ: f32 = 70.0;
const PITCH_MAX_HZ: f32 = 1000.0;

impl Yin {
    pub fn new(sample_rate: f32) -> Self {
        let tau_min = (sample_rate / PITCH_MAX_HZ) as usize;
        let tau_max = (sample_rate / PITCH_MIN_HZ) as usize;
        Self {
            sample_rate,
            tau_min,
            tau_max,
            ring: vec![0.0; WINDOW + tau_max],
            write: 0,
            since_analysis: 0,
            frame: vec![0.0; WINDOW + tau_max],
            diff: vec![0.0; tau_max + 1],
            pitch: None,
        }
    }

    /// Feed one sample. Returns the current estimate, refreshed every HOP.
    pub fn push(&mut self, x: f32) -> Option<f32> {
        self.ring[self.write] = x;
        self.write = (self.write + 1) % self.ring.len();
        self.since_analysis += 1;
        if self.since_analysis >= HOP {
            self.since_analysis = 0;
            self.analyse();
        }
        self.pitch
    }

    fn analyse(&mut self) {
        // Unroll the ring into a linear frame, oldest first.
        let n = self.ring.len();
        for (i, slot) in self.frame.iter_mut().enumerate() {
            *slot = self.ring[(self.write + i) % n];
        }
        let frame = &self.frame;

        // Difference function for every lag up to tau_max; the small lags
        // below tau_min are needed by the normalisation's running sum.
        self.diff[0] = 0.0;
        for tau in 1..=self.tau_max {
            let mut acc = 0.0;
            for j in 0..WINDOW {
                let d = frame[j] - frame[j + tau];
                acc += d * d;
            }
            self.diff[tau] = acc;
        }

        // Cumulative mean normalised difference, in place.
        let mut running = 0.0;
        for tau in 1..=self.tau_max {
            running += self.diff[tau];
            self.diff[tau] = if running > 0.0 {
                self.diff[tau] * tau as f32 / running
            } else {
                1.0
            };
        }

        // First dip under the threshold, followed to its bottom; else the
        // deepest dip, if it is deep enough to mean anything.
        let d = &self.diff;
        let mut best = None;
        let mut tau = self.tau_min;
        while tau < self.tau_max {
            if d[tau] < YIN_THRESHOLD {
                while tau + 1 < self.tau_max && d[tau + 1] < d[tau] {
                    tau += 1;
                }
                best = Some(tau);
                break;
            }
            tau += 1;
        }
        let tau = match best {
            Some(t) => t,
            None => {
                let (t, v) = (self.tau_min..self.tau_max)
                    .map(|t| (t, d[t]))
                    .fold((0, f32::INFINITY), |m, c| if c.1 < m.1 { c } else { m });
                if v > YIN_UNVOICED {
                    self.pitch = None;
                    return;
                }
                t
            }
        };

        // Parabolic refinement around the dip.
        let refined = if tau > 1 && tau < self.tau_max {
            let (a, b, c) = (d[tau - 1], d[tau], d[tau + 1]);
            let denom = a - 2.0 * b + c;
            if denom.abs() > 1e-12 {
                tau as f32 + 0.5 * (a - c) / denom
            } else {
                tau as f32
            }
        } else {
            tau as f32
        };
        self.pitch = Some(self.sample_rate / refined);
    }

    pub fn reset(&mut self) {
        self.ring.iter_mut().for_each(|v| *v = 0.0);
        self.write = 0;
        self.since_analysis = 0;
        self.pitch = None;
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Stage {
    Idle,
    /// Note detected, counting toward the Onset time.
    Holding,
    /// Feedback swelling or established.
    Feeding,
}

pub struct Controls {
    pub harmonic: i32,
    pub onset_s: f32,
    pub swell_s: f32,
    pub gain: f32,
    pub wobble: f32,
    pub active: bool,
}

pub struct Feedbacker {
    sample_rate: f32,
    yin: Yin,
    level: OnePole,
    level_release: OnePole,
    stage: Stage,
    /// Pitch of the note the clock started on.
    note_pitch: f32,
    held: usize,
    unvoiced: usize,
    env_db: f32,
    osc_freq: f32,
    phase: f64,
    wobble_t: f64,
    freq_slew: f32,
}

impl Feedbacker {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            sample_rate,
            yin: Yin::new(sample_rate),
            // Level follower: 1 ms attack, 50 ms release, done as two
            // one-poles picked per sample by direction.
            level: OnePole::new(1.0 / (2.0 * std::f32::consts::PI * 0.001), sample_rate),
            level_release: OnePole::new(1.0 / (2.0 * std::f32::consts::PI * 0.05), sample_rate),
            stage: Stage::Idle,
            note_pitch: 0.0,
            held: 0,
            unvoiced: 0,
            env_db: FLOOR_DB,
            osc_freq: 0.0,
            phase: 0.0,
            wobble_t: 0.0,
            // Oscillator follows bends with a 50 ms time constant.
            freq_slew: 1.0 - (-1.0 / (0.05 * sample_rate)).exp(),
        }
    }

    pub fn process(&mut self, x: f32, c: &Controls) -> f32 {
        let pitch = self.yin.push(x);

        // Envelope of the input in dB, fast up and slow down.
        let rect = x.abs();
        let env = if rect > self.level.peek() {
            let v = self.level.lowpass(rect);
            self.level_release.set(v);
            v
        } else {
            let v = self.level_release.lowpass(rect);
            self.level.set(v);
            v
        };
        let env_db = 20.0 * env.max(1e-9).log10();
        let loud = env_db > START_DB;
        let muted = env_db < STOP_DB;

        match pitch {
            Some(_) => self.unvoiced = 0,
            None => self.unvoiced += 1,
        }
        let lost = self.unvoiced as f32 > GRACE_MS * 0.001 * self.sample_rate;
        let new_note = match pitch {
            Some(p) if self.note_pitch > 0.0 => (p / self.note_pitch - 1.0).abs() > NEW_NOTE_RATIO,
            _ => false,
        };

        self.stage = match self.stage {
            Stage::Idle => match pitch {
                Some(p) if loud && c.active => {
                    self.note_pitch = p;
                    self.held = 0;
                    Stage::Holding
                }
                _ => Stage::Idle,
            },
            Stage::Holding => {
                if !loud || lost || new_note || !c.active {
                    Stage::Idle
                } else {
                    self.held += 1;
                    if self.held as f32 >= c.onset_s * self.sample_rate {
                        Stage::Feeding
                    } else {
                        Stage::Holding
                    }
                }
            }
            Stage::Feeding => {
                if muted || lost || new_note || !c.active {
                    Stage::Idle
                } else {
                    Stage::Feeding
                }
            }
        };

        // Exponential swell (linear in dB) while feeding, release otherwise.
        let swell_rate = -FLOOR_DB / (c.swell_s.max(0.01) * self.sample_rate);
        let release_rate = -FLOOR_DB / (RELEASE_MS * 0.001 * self.sample_rate);
        self.env_db = if self.stage == Stage::Feeding {
            (self.env_db + swell_rate).min(0.0)
        } else {
            (self.env_db - release_rate).max(FLOOR_DB)
        };
        if self.env_db <= FLOOR_DB {
            // Nothing sounding: exact silence, oscillator parked.
            self.phase = 0.0;
            self.osc_freq = 0.0;
            return x;
        }

        // Target tone: harmonic of the current pitch, folded under the cap.
        // Snap when the tone is still inaudible, slew once it is heard so
        // bends and vibrato follow smoothly.
        let base = pitch.unwrap_or(self.note_pitch);
        let mut target = base * c.harmonic as f32;
        while target > MAX_FREQ_HZ && target > base {
            target /= 2.0;
        }
        if self.osc_freq == 0.0 || self.env_db < FLOOR_DB + 6.0 {
            self.osc_freq = target;
        } else {
            self.osc_freq += self.freq_slew * (target - self.osc_freq);
        }

        self.wobble_t += 1.0 / self.sample_rate as f64;
        let cents = c.wobble as f64
            * (WOBBLE_FAST_CENTS * (2.0 * PI * WOBBLE_FAST_HZ * self.wobble_t).sin()
                + WOBBLE_SLOW_CENTS * (2.0 * PI * WOBBLE_SLOW_HZ * self.wobble_t).sin());
        let freq = self.osc_freq as f64 * (2f64).powf(cents / 1200.0);
        self.phase = (self.phase + freq / self.sample_rate as f64) % 1.0;
        let amp = c.gain * MAX_LEVEL * 10f32.powf(self.env_db / 20.0);
        x + amp * (2.0 * PI * self.phase).sin() as f32
    }

    pub fn reset(&mut self) {
        self.yin.reset();
        self.level.reset();
        self.level_release.reset();
        self.stage = Stage::Idle;
        self.note_pitch = 0.0;
        self.held = 0;
        self.unvoiced = 0;
        self.env_db = FLOOR_DB;
        self.osc_freq = 0.0;
        self.phase = 0.0;
        self.wobble_t = 0.0;
    }
}

pub struct Euios {
    params: Arc<EuiosParams>,
    channels: Vec<Feedbacker>,
}

#[derive(Params)]
struct EuiosParams {
    #[id = "harmonic"]
    harmonic: IntParam,
    #[id = "onset"]
    onset: FloatParam,
    #[id = "swell"]
    swell: FloatParam,
    #[id = "gain"]
    gain: FloatParam,
    #[id = "wobble"]
    wobble: FloatParam,
    #[id = "active"]
    active: BoolParam,
}

impl Default for Euios {
    fn default() -> Self {
        Self {
            params: Arc::new(EuiosParams::default()),
            channels: Vec::new(),
        }
    }
}

fn seconds(name: &str, default: f32, min: f32, max: f32) -> FloatParam {
    FloatParam::new(
        name,
        default,
        FloatRange::Skewed {
            min,
            max,
            factor: FloatRange::skew_factor(-1.0),
        },
    )
    .with_unit(" s")
    .with_value_to_string(formatters::v2s_f32_rounded(2))
}

fn percent(name: &str, default: f32) -> FloatParam {
    FloatParam::new(name, default, FloatRange::Linear { min: 0.0, max: 1.0 })
        .with_smoother(SmoothingStyle::Linear(20.0))
        .with_unit("%")
        .with_value_to_string(formatters::v2s_f32_percentage(0))
        .with_string_to_value(formatters::s2v_f32_percentage())
}

impl Default for EuiosParams {
    fn default() -> Self {
        Self {
            // 2 = the octave, the classic feedback note. 3 is octave plus a
            // fifth, 5 two octaves plus a major third.
            harmonic: IntParam::new("Harmonic", 2, IntRange::Linear { min: 1, max: 5 }),
            onset: seconds("Onset", 0.8, 0.1, 3.0),
            swell: seconds("Swell", 0.6, 0.1, 2.0),
            gain: percent("Gain", 0.5),
            wobble: percent("Wobble", 0.3),
            active: BoolParam::new("Active", true),
        }
    }
}

impl Plugin for Euios {
    const NAME: &'static str = "euios";
    const VENDOR: &'static str = "dkoutavas";
    const URL: &'static str = "https://github.com/dkoutavas/lyaeus";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(1),
            main_output_channels: NonZeroU32::new(1),
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            ..AudioIOLayout::const_default()
        },
    ];

    const SAMPLE_ACCURATE_AUTOMATION: bool = true;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn initialize(
        &mut self,
        audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        let n = audio_io_layout
            .main_output_channels
            .map(|c| c.get())
            .unwrap_or(1) as usize;
        self.channels = (0..n)
            .map(|_| Feedbacker::new(buffer_config.sample_rate))
            .collect();
        true
    }

    fn reset(&mut self) {
        for ch in &mut self.channels {
            ch.reset();
        }
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        for frame in buffer.iter_samples() {
            let c = Controls {
                harmonic: self.params.harmonic.value(),
                onset_s: self.params.onset.value(),
                swell_s: self.params.swell.value(),
                gain: self.params.gain.smoothed.next(),
                wobble: self.params.wobble.smoothed.next(),
                active: self.params.active.value(),
            };
            for (i, sample) in frame.into_iter().enumerate() {
                *sample = self.channels[i].process(*sample, &c);
            }
        }
        ProcessStatus::Normal
    }
}

impl ClapPlugin for Euios {
    const CLAP_ID: &'static str = "com.dkoutavas.euios";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Guitar feedback at any volume");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Mono,
        ClapFeature::Stereo,
    ];
}

impl Vst3Plugin for Euios {
    const VST3_CLASS_ID: [u8; 16] = *b"EuiosFeedbacker1";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Fx];
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{rms, sine, spectrum_db};

    const FS: f32 = 48000.0;

    fn detect(signal: &[f32]) -> Option<f32> {
        let mut yin = Yin::new(FS);
        let mut last = None;
        for &x in signal {
            last = yin.push(x);
        }
        last
    }

    #[test]
    fn yin_reads_sines_across_the_guitar_range() {
        for f in [82.4, 110.0, 196.0, 329.6, 440.0, 659.3, 880.0] {
            let got = detect(&sine(f, FS as f64, 0.2, 8192)).expect("unvoiced");
            let err = (got / f as f32 - 1.0).abs();
            assert!(err < 0.01, "{f} Hz read as {got} Hz");
        }
    }

    #[test]
    fn yin_reads_a_plucked_string() {
        // Karplus-Strong: a noise burst through a delay line with a
        // two-point average. Period is the line length plus half a sample.
        let n = 244usize;
        let expected = FS / (n as f32 + 0.5);
        let mut line: Vec<f32> = Vec::with_capacity(n);
        let mut seed: u32 = 12345;
        for _ in 0..n {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            line.push((seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5);
        }
        let mut out = Vec::with_capacity(12000);
        let mut i = 0;
        for _ in 0..12000 {
            let j = (i + 1) % n;
            let v = 0.5 * (line[i] + line[j]) * 0.999;
            out.push(line[i]);
            line[i] = v;
            i = j;
        }
        let got = detect(&out).expect("unvoiced");
        let err = (got / expected - 1.0).abs();
        assert!(err < 0.01, "expected {expected} Hz, read {got} Hz");
    }

    #[test]
    fn yin_calls_noise_unvoiced() {
        let mut seed: u32 = 777;
        let noise: Vec<f32> = (0..8192)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
            })
            .collect();
        assert!(detect(&noise).is_none());
    }

    fn controls() -> Controls {
        Controls {
            harmonic: 2,
            onset_s: 0.3,
            swell_s: 0.3,
            gain: 1.0,
            wobble: 0.0,
            active: true,
        }
    }

    #[test]
    fn silence_in_silence_out() {
        let mut fb = Feedbacker::new(FS);
        let c = controls();
        for _ in 0..48000 {
            assert_eq!(fb.process(0.0, &c), 0.0);
        }
    }

    #[test]
    fn feedback_swells_at_the_octave_then_dies_when_muted() {
        let mut fb = Feedbacker::new(FS);
        let c = controls();
        let held = sine(220.0, FS as f64, 0.1, 2 * FS as usize);
        let out: Vec<f32> = held.iter().map(|&x| fb.process(x, &c)).collect();
        // Before the onset: nothing but the dry note.
        let early = &out[4800..9600];
        let dry = &held[4800..9600];
        let diff: f32 = early.iter().zip(dry).map(|(a, b)| (a - b).abs()).sum();
        assert!(diff < 1e-3, "feedback leaked before the onset");
        // Well after onset plus swell: a 440 Hz line near the full level.
        let spec = spectrum_db(&out[out.len() - 4800..]);
        let at_440 = spec[44];
        let expected = 20.0 * (MAX_LEVEL as f64).log10();
        assert!((at_440 - expected).abs() < 1.5, "440 Hz at {at_440:.1} dB, expected {expected:.1}");
        // Mute: the tone must be gone within the release time plus margin.
        let tail: Vec<f32> = (0..FS as usize).map(|_| fb.process(0.0, &c)).collect();
        let after = (RELEASE_MS * 0.001 * FS) as usize + 4800;
        assert!(rms(&tail[after..after + 4800]) < 1e-3, "feedback did not release");
    }

    #[test]
    fn a_new_note_restarts_the_onset_clock() {
        let mut fb = Feedbacker::new(FS);
        let c = controls();
        let a = sine(220.0, FS as f64, 0.1, FS as usize);
        for &x in &a {
            fb.process(x, &c);
        }
        assert_eq!(fb.stage, Stage::Feeding);
        // Jump a fourth: feeding stops, the clock restarts.
        let b = sine(293.7, FS as f64, 0.1, 4800);
        for &x in &b {
            fb.process(x, &c);
        }
        assert_ne!(fb.stage, Stage::Feeding);
        assert!(fb.env_db < -6.0, "old feedback still sounding at {} dB", fb.env_db);
    }
}

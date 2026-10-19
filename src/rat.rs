//! bromios: a ProCo RAT style distortion pedal, meant to sit in front of
//! lyaeus the way the real pedal sits in front of a real amp.
//!
//! Four things make a RAT sound like a RAT, and each one is a block here:
//!
//! 1. A huge, frequency-dependent gain stage (`GainNetwork`). The LM308
//!    op-amp runs non-inverting with the Distortion pot as feedback and two
//!    RC legs to ground. Gain climbs to 66 dB above 720 Hz but far less for
//!    bass, which is why the pedal stays tight at full tilt.
//! 2. The LM308's slow slew rate, 0.3 V/us. At high gain the output cannot
//!    follow the edges, so square waves become ramps: the famous "LM308 is
//!    the sound" softening.
//! 3. A 1 kOhm resistor into two 1N914 diodes to ground: hard silicon
//!    clipping near +-0.6 V with a logarithmic creep above it.
//! 4. The Filter knob: a one-pole low-pass from 32 kHz down to 475 Hz,
//!    clockwise is darker, as on the pedal.
//!
//! Signal path, base rate in square brackets, oversampled rate in braces:
//!
//! ```text
//! [in -> 20 Hz high-pass -> gain network -> feedback-cap low-pass] -> up
//!   {op-amp: rate-limited one-pole within the rails -> diode clipper}
//! -> down -> [Filter low-pass -> 5 Hz high-pass -> Volume -> out]
//! ```

use nih_plug::prelude::*;
use std::sync::Arc;

use crate::filters::OnePole;
use crate::oversample::{Oversampler, FACTOR, LATENCY};

/// The model works in volts. One full-scale sample is taken as one volt,
/// which puts a hot guitar pickup around 0.1 to 0.3. This is the one
/// calibration constant: raise it if your interface runs cold.
const VOLTS_PER_FS: f32 = 1.0;

/// Distortion pot, 100k audio taper.
const RF_MAX: f32 = 100e3;
/// Ground legs of the gain stage: 47 Ohm + 4.7 uF (corner 720 Hz) and
/// 560 Ohm + 2.2 uF (corner 130 Hz).
const R_HOT: f32 = 47.0;
const C_HOT: f32 = 4.7e-6;
const R_WARM: f32 = 560.0;
const C_WARM: f32 = 2.2e-6;
/// 100 pF across the Distortion pot.
const C_FEEDBACK: f32 = 100e-12;
/// LM308 with its 30 pF compensation cap: about 1 MHz gain-bandwidth and
/// 0.3 V/us slew rate.
const GAIN_BANDWIDTH_HZ: f32 = 1.0e6;
const SLEW_V_PER_US: f32 = 0.3;
/// Op-amp output swing on a 9 V supply, roughly.
const RAIL_V: f32 = 4.0;
/// Series resistor into the diode pair.
const R_DIODE: f64 = 1000.0;
/// 1N914: saturation current and ideality factor (Yeh, DAFx 2007), thermal
/// voltage at room temperature.
const DIODE_IS: f64 = 2.52e-9;
const DIODE_NVT: f64 = 1.752 * 0.02585;
/// Filter knob range: 1.5k + 100k pot into 3.3 nF.
const FILTER_MAX_HZ: f32 = 32000.0;
const FILTER_MIN_HZ: f32 = 475.0;

fn corner_hz(r: f32, c: f32) -> f32 {
    1.0 / (2.0 * std::f32::consts::PI * r * c)
}

/// One-pole coefficient for a corner at a sample rate, same pole placement
/// as `OnePole`, clamped under Nyquist.
fn one_pole_g(corner: f32, sample_rate: f32) -> f32 {
    let corner = corner.min(0.45 * sample_rate);
    1.0 - (-2.0 * std::f32::consts::PI * corner / sample_rate).exp()
}

/// Distortion knob 0..=1 to feedback resistance. The pot is audio taper, so
/// the knob covers two decades: half way is about 9 kOhm, not 50.
fn feedback_ohms(knob: f32) -> f32 {
    RF_MAX * (10f32.powf(2.0 * knob) - 1.0) / 99.0
}

/// Non-inverting gain stage with two RC legs to ground. For a feedback
/// resistor Rf and a leg of R and C, gain is `1 + Rf * sC / (1 + sRC)`,
/// which is `1 + (Rf / R) * highpass(x)` with the high-pass corner at
/// `1 / (2 pi R C)`. Two legs just add. Linear, so it runs at base rate.
#[derive(Clone)]
struct GainNetwork {
    hot: OnePole,
    warm: OnePole,
}

impl GainNetwork {
    fn new(sample_rate: f32) -> Self {
        Self {
            hot: OnePole::new(corner_hz(R_HOT, C_HOT), sample_rate),
            warm: OnePole::new(corner_hz(R_WARM, C_WARM), sample_rate),
        }
    }

    fn process(&mut self, x: f32, rf: f32) -> f32 {
        x + (rf / R_HOT) * self.hot.highpass(x) + (rf / R_WARM) * self.warm.highpass(x)
    }

    fn reset(&mut self) {
        self.hot.reset();
        self.warm.reset();
    }
}

/// The 100 pF across the Distortion pot shunts the feedback at high
/// frequencies: a pole at `1 / (2 pi Rf C)`, 16 kHz at full Distortion and
/// above the audio band below about half. Linear, so it runs at base rate.
fn feedback_cap_hz(rf: f32, sample_rate: f32) -> f32 {
    let hz = if rf > 0.0 { corner_hz(rf, C_FEEDBACK) } else { f32::INFINITY };
    hz.clamp(20.0, 0.45 * sample_rate)
}

/// Closed-loop bandwidth of the op-amp: gain-bandwidth product divided by
/// the gain it is asked for. At full Distortion that is about 470 Hz. See
/// `OpAmp` for why this pole and the slew rate are one block.
fn closed_loop_hz(rf: f32) -> f32 {
    GAIN_BANDWIDTH_HZ / (1.0 + rf / R_HOT)
}

/// Filter knob 0..=1, clockwise darker: 32 kHz down to 475 Hz on a log
/// scale, clamped under Nyquist.
fn filter_hz(knob: f32, sample_rate: f32) -> f32 {
    (FILTER_MAX_HZ * (FILTER_MIN_HZ / FILTER_MAX_HZ).powf(knob)).min(0.45 * sample_rate)
}

/// Node voltage of 1 kOhm into two antiparallel 1N914 diodes to ground,
/// for an input `vin`. It solves `(vin - v) / R = 2 Is sinh(v / nVt)`,
/// which has no closed form, with Newton's method. Starting just under the
/// knee the steps converge in a handful of iterations; the clamp keeps a
/// wild step from blowing up `exp`. Only used to build the table below.
fn diode_node_voltage(vin: f64) -> f64 {
    let mut v = vin.clamp(-0.6, 0.6);
    for _ in 0..40 {
        let e = (v / DIODE_NVT).exp();
        let sinh = 0.5 * (e - 1.0 / e);
        let cosh = 0.5 * (e + 1.0 / e);
        let f = v + R_DIODE * 2.0 * DIODE_IS * sinh - vin;
        let df = 1.0 + R_DIODE * 2.0 * DIODE_IS / DIODE_NVT * cosh;
        let next = (v - f / df).clamp(-1.0, 1.0);
        if (next - v).abs() < 1e-12 {
            return next;
        }
        v = next;
    }
    v
}

/// The diode clipper as a lookup table over the rail-limited input range,
/// linear between grid points 1 mV apart. The clipper is a static curve,
/// so solving it once per grid point at start-up beats running Newton's
/// method on every sample. Built on first use, shared by all channels.
///
/// First-order antiderivative anti-aliasing on this knee was tried and
/// measured: 1 dB better at 8x. Not worth its weight.
struct DiodeTable {
    g: Vec<f64>,
}

const TABLE_N: usize = 8193;
const TABLE_STEP: f64 = 2.0 * RAIL_V as f64 / (TABLE_N - 1) as f64;

impl DiodeTable {
    fn build() -> Self {
        let g = (0..TABLE_N)
            .map(|i| diode_node_voltage(-RAIL_V as f64 + i as f64 * TABLE_STEP))
            .collect();
        Self { g }
    }

    fn get() -> &'static Self {
        static TABLE: std::sync::OnceLock<DiodeTable> = std::sync::OnceLock::new();
        TABLE.get_or_init(Self::build)
    }

    fn g(&self, x: f64) -> f64 {
        let pos = (x + RAIL_V as f64) / TABLE_STEP;
        let i = (pos.floor() as usize).min(TABLE_N - 2);
        let t = pos - i as f64;
        self.g[i] + t * (self.g[i + 1] - self.g[i])
    }
}

fn diode_clip(vin: f32) -> f32 {
    let x = (vin as f64).clamp(-RAIL_V as f64, RAIL_V as f64);
    DiodeTable::get().g(x) as f32
}

/// The op-amp as a rate-limited one-pole: `d(out)/dt = clamp(w (in - out),
/// -S, +S)`, output held within the rails.
///
/// Both limits come from the same part. The 30 pF compensation cap is the
/// dominant pole (closed-loop corner `w` = GBW / gain) and, because the
/// input stage can only push so much current into it, also the slew limit
/// `S`. Small errors see a one-pole low-pass; once the error exceeds S / w
/// (about 100 V of output error at full Distortion, which is 50 mV at the
/// input, where a bipolar input pair saturates) the output ramps at S.
///
/// An earlier version put the pole before a plain slew limiter. That
/// starved the limiter of drive at full Distortion, and the hard hand-over
/// between "ramping" and "tracking" snapped to the sample grid and aliased
/// at -58 dB. Here the hand-over is where `w (in - out)` reaches S, so the
/// output's slope is continuous through it.
#[derive(Clone)]
struct OpAmp {
    out: f32,
    /// One-pole coefficient for the closed-loop corner at the high rate.
    g: f32,
}

impl OpAmp {
    fn process(&mut self, input: f32, step: f32) -> f32 {
        let drive = (self.g * (input - self.out)).clamp(-step, step);
        self.out = (self.out + drive).clamp(-RAIL_V, RAIL_V);
        self.out
    }
}

pub struct Controls {
    pub rf: f32,
    pub volume: f32,
}

#[derive(Clone)]
pub struct Pedal {
    sample_rate: f32,
    input_hp: OnePole,
    gain: GainNetwork,
    rolloff: OnePole,
    os: Oversampler,
    opamp: OpAmp,
    slew_step: f32,
    filter: OnePole,
    output_hp: OnePole,
}

impl Pedal {
    pub fn new(sample_rate: f32) -> Self {
        let high = sample_rate * FACTOR as f32;
        Self {
            sample_rate,
            input_hp: OnePole::new(20.0, sample_rate),
            gain: GainNetwork::new(sample_rate),
            rolloff: OnePole::new(feedback_cap_hz(feedback_ohms(0.5), sample_rate), sample_rate),
            os: Oversampler::new(),
            opamp: OpAmp {
                out: 0.0,
                g: one_pole_g(closed_loop_hz(feedback_ohms(0.5)), high),
            },
            // Volts the op-amp output may move in one high-rate sample.
            slew_step: SLEW_V_PER_US * 1.0e6 / high,
            filter: OnePole::new(filter_hz(0.5, sample_rate), sample_rate),
            output_hp: OnePole::new(5.0, sample_rate),
        }
    }

    /// Knob-dependent corners, cheap enough to recompute once per block.
    pub fn set_knobs(&mut self, distortion: f32, filter: f32) {
        let rf = feedback_ohms(distortion);
        self.rolloff = OnePole::new(feedback_cap_hz(rf, self.sample_rate), self.sample_rate)
            .with_state_of(&self.rolloff);
        self.opamp.g = one_pole_g(closed_loop_hz(rf), self.sample_rate * FACTOR as f32);
        self.filter =
            OnePole::new(filter_hz(filter, self.sample_rate), self.sample_rate).with_state_of(&self.filter);
    }

    pub fn process(&mut self, x: f32, c: &Controls) -> f32 {
        let x = self.input_hp.highpass(x) * VOLTS_PER_FS;
        let v = self.rolloff.lowpass(self.gain.process(x, c.rf));
        let mut block = self.os.up(v);
        for s in block.iter_mut() {
            *s = diode_clip(self.opamp.process(*s, self.slew_step));
        }
        let y = self.filter.lowpass(self.os.down(block));
        self.output_hp.highpass(y) / VOLTS_PER_FS * c.volume
    }

    pub fn reset(&mut self) {
        self.input_hp.reset();
        self.gain.reset();
        self.rolloff.reset();
        self.os.reset();
        self.opamp.out = 0.0;
        self.filter.reset();
        self.output_hp.reset();
    }
}

pub struct Bromios {
    params: Arc<BromiosParams>,
    channels: Vec<Pedal>,
}

#[derive(Params)]
struct BromiosParams {
    #[id = "distortion"]
    distortion: FloatParam,
    #[id = "filter"]
    filter: FloatParam,
    #[id = "volume"]
    volume: FloatParam,
}

impl Default for Bromios {
    fn default() -> Self {
        Self {
            params: Arc::new(BromiosParams::default()),
            channels: Vec::new(),
        }
    }
}

impl Default for BromiosParams {
    fn default() -> Self {
        Self {
            distortion: FloatParam::new("Distortion", 5.0, FloatRange::Linear { min: 0.0, max: 10.0 })
                .with_smoother(SmoothingStyle::Linear(20.0))
                .with_value_to_string(formatters::v2s_f32_rounded(1)),
            filter: FloatParam::new("Filter", 5.0, FloatRange::Linear { min: 0.0, max: 10.0 })
                .with_value_to_string(formatters::v2s_f32_rounded(1)),
            // The diodes hold the output near 0.65 V, hot for an amp input.
            // -10 dB lands it around a loud pickup.
            volume: FloatParam::new(
                "Volume",
                util::db_to_gain(-10.0),
                FloatRange::Skewed {
                    min: util::db_to_gain(-40.0),
                    max: util::db_to_gain(12.0),
                    factor: FloatRange::gain_skew_factor(-40.0, 12.0),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(20.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(1))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),
        }
    }
}

impl Plugin for Bromios {
    const NAME: &'static str = "bromios";
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
        context: &mut impl InitContext<Self>,
    ) -> bool {
        let n = audio_io_layout
            .main_output_channels
            .map(|c| c.get())
            .unwrap_or(1) as usize;
        self.channels = vec![Pedal::new(buffer_config.sample_rate); n];
        context.set_latency_samples(LATENCY);
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
        // ponytail: filter corners update once per block, like the amp's
        // tone knobs. The Distortion gain itself is smoothed per sample.
        let distortion = self.params.distortion.value() / 10.0;
        let filter = self.params.filter.value() / 10.0;
        for ch in &mut self.channels {
            ch.set_knobs(distortion, filter);
        }

        for frame in buffer.iter_samples() {
            let c = Controls {
                rf: feedback_ohms(self.params.distortion.smoothed.next() / 10.0),
                volume: self.params.volume.smoothed.next(),
            };
            for (i, sample) in frame.into_iter().enumerate() {
                *sample = self.channels[i].process(*sample, &c);
            }
        }

        ProcessStatus::Normal
    }
}

impl ClapPlugin for Bromios {
    const CLAP_ID: &'static str = "com.dkoutavas.bromios";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("RAT-style distortion pedal");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Distortion,
        ClapFeature::Mono,
        ClapFeature::Stereo,
    ];
}

impl Vst3Plugin for Bromios {
    const VST3_CLASS_ID: [u8; 16] = *b"BromiosRatPedal1";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Distortion];
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{sine, spectrum_db};

    const FS: f32 = 48000.0;

    fn steady_peak(signal: &[f32]) -> f32 {
        signal[signal.len() / 2..].iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    #[test]
    fn gain_network_favours_mids_over_bass() {
        let rf = feedback_ohms(1.0);
        let peak_at = |freq: f64| {
            let mut net = GainNetwork::new(FS);
            let out: Vec<f32> = sine(freq, FS as f64, 1e-4, 48000)
                .iter()
                .map(|&x| net.process(x, rf))
                .collect();
            steady_peak(&out)
        };
        let ratio_db = 20.0 * (peak_at(2000.0) / peak_at(100.0)).log10();
        assert!(ratio_db > 12.0, "mids only {ratio_db:.1} dB above bass");
        // And the headline number: about 66 dB above 720 Hz.
        let hf_gain_db = 20.0 * (peak_at(5000.0) / 1e-4).log10();
        assert!((hf_gain_db - 66.0).abs() < 2.0, "HF gain {hf_gain_db:.1} dB");
    }

    #[test]
    fn opamp_slews_big_errors_and_filters_small_ones() {
        let pedal = Pedal::new(FS);
        let step = pedal.slew_step;
        // 0.3 V/us means 4 V takes 13.3 us: about 5 samples at 384 kHz.
        assert!((step - 0.78125).abs() < 1e-5, "step {step}");
        let high = FS * FACTOR as f32;
        let mut amp = OpAmp {
            out: 0.0,
            g: one_pole_g(closed_loop_hz(feedback_ohms(1.0)), high),
        };
        // Big error: every step is a full slew step until the rail.
        let mut prev = 0.0f32;
        let mut reached = false;
        for _ in 0..200 {
            let out = amp.process(200.0, step);
            assert!((out - prev).abs() <= step + 1e-6);
            if !reached && out < RAIL_V - 1e-6 {
                assert!((out - prev - step).abs() < 1e-5, "ramp step {}", out - prev);
            }
            prev = out;
            reached |= (out - RAIL_V).abs() < 1e-6;
        }
        assert!(reached, "never reached the rail");
        // Small error: a one-pole at about 470 Hz, settled well within 10 ms.
        let mut amp = OpAmp { out: 0.0, g: amp.g };
        let mut out = 0.0;
        for _ in 0..(0.01 * high) as usize {
            out = amp.process(1.0, step);
        }
        assert!((out - 1.0).abs() < 1e-3, "settled at {out}");
    }

    #[test]
    fn diode_clipper_clamps_big_and_passes_small() {
        let big = diode_clip(4.0);
        assert!((0.6..0.7).contains(&big), "4 V in gave {big} V");
        let small = diode_clip(0.05);
        assert!((small - 0.05).abs() < 0.0005, "50 mV in gave {small} V");
        // The table agrees with the solver it was built from, between grid
        // points too.
        for vin in [-3.3, -0.71, -0.2, 0.123, 0.456, 0.789, 2.5] {
            let table = diode_clip(vin) as f64;
            let exact = diode_node_voltage(vin as f64);
            assert!((table - exact).abs() < 1e-6, "{vin}: {table} vs {exact}");
        }
    }

    fn run(input: &[f32], distortion: f32) -> Vec<f32> {
        let mut pedal = Pedal::new(FS);
        pedal.set_knobs(distortion, 0.0);
        let c = Controls {
            rf: feedback_ohms(distortion),
            volume: 1.0,
        };
        input.iter().map(|&x| pedal.process(x, &c)).collect()
    }

    #[test]
    fn aliasing_stays_below_minus_60_db_at_full_distortion() {
        let input = sine(1010.0, FS as f64, 0.1, FS as usize);
        let output = run(&input, 1.0);
        let spec = spectrum_db(&output[output.len() - 4800..]);
        let fundamental = spec[101];
        let mut worst = (f64::NEG_INFINITY, 0usize);
        for (k, &level) in spec.iter().enumerate().skip(5) {
            let near_harmonic = (k % 101 <= 3) || (k % 101 >= 98);
            if !near_harmonic && k * 10 < 18000 && level > worst.0 {
                worst = (level, k);
            }
        }
        let rel = worst.0 - fundamental;
        eprintln!("bromios aliasing: worst audible line {rel:.1} dB at {} Hz", worst.1 * 10);
        assert!(rel < -60.0, "aliasing at {rel:.1} dB");
    }

    #[test]
    fn silence_in_silence_out_and_no_dc() {
        let output = run(&vec![0.0; 4800], 1.0);
        assert!(output.iter().all(|v| v.is_finite() && v.abs() < 1e-6));
        let input = sine(220.0, FS as f64, 0.2, FS as usize);
        let output = run(&input, 1.0);
        let tail = &output[output.len() - 4800..];
        let mean = tail.iter().sum::<f32>() / tail.len() as f32;
        assert!(mean.abs() < 1e-3, "DC offset {mean}");
    }
}

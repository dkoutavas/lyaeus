//! meilichios: a Boss Blues Driver style overdrive, "the gentle one".
//!
//! Where bromios is a razor, this is a light, open, amp-like overdrive for
//! in front of the amp: a clean boost into lyaeus at low Gain, or stacked
//! into bromios. It is a character model, not a schematic trace. What the
//! BD-2 does that a Tube Screamer does not: it keeps the bass, it does not
//! hump the mids, and its clip has a softer knee with a little asymmetry
//! from the FET stages, which reads as even harmonics and "amp-like".
//!
//! Signal path, base rate in square brackets, oversampled rate in braces:
//!
//! ```text
//! [in -> 30 Hz high-pass -> Gain] -> up
//!   {diode-feedback soft clip -> FET square-law bend}
//! -> down -> [Tone blend -> 10 Hz high-pass -> Level -> out]
//! ```

use nih_plug::prelude::*;
use std::sync::Arc;

use crate::filters::OnePole;
use crate::oversample::{Oversampler, LATENCY};

/// Two 1N4148 in series each way in the feedback loop: the knee sits near
/// 1.2 V, twice a single diode's, which is a big part of why the pedal
/// stays open and dynamic.
const DIODE_KNEE_V: f32 = 1.2;
/// FET output stage: a square-law bend that leaves the slope at least 0.7
/// across the clip range and adds even harmonics.
const FET_CURVE: f32 = 0.15;
/// Tone control pivot. Below it the Tone knob turns toward a low-pass,
/// above it toward a high-pass, Boss style.
const TONE_HZ: f32 = 1000.0;

pub struct Controls {
    pub gain: f32,
    pub tone: f32,
    pub level: f32,
}

#[derive(Clone)]
pub struct Drive {
    input_hp: OnePole,
    os: Oversampler,
    tone_lp: OnePole,
    output_hp: OnePole,
}

impl Drive {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            input_hp: OnePole::new(30.0, sample_rate),
            os: Oversampler::new(),
            tone_lp: OnePole::new(TONE_HZ, sample_rate),
            output_hp: OnePole::new(10.0, sample_rate),
        }
    }

    pub fn process(&mut self, x: f32, c: &Controls) -> f32 {
        let x = self.input_hp.highpass(x) * c.gain;
        let mut block = self.os.up(x);
        for s in block.iter_mut() {
            let clipped = DIODE_KNEE_V * (*s / DIODE_KNEE_V).tanh();
            *s = clipped - FET_CURVE * clipped * clipped / DIODE_KNEE_V;
        }
        let y = self.os.down(block);
        // Blend between a low-pass and a high-pass at the same corner. At
        // noon the two add back to the input, times two for unity.
        let lp = self.tone_lp.lowpass(y);
        let hp = y - lp;
        let y = 2.0 * ((1.0 - c.tone) * lp + c.tone * hp);
        self.output_hp.highpass(y) * c.level
    }

    pub fn reset(&mut self) {
        self.input_hp.reset();
        self.os.reset();
        self.tone_lp.reset();
        self.output_hp.reset();
    }
}

pub struct Meilichios {
    params: Arc<MeilichiosParams>,
    channels: Vec<Drive>,
}

#[derive(Params)]
struct MeilichiosParams {
    #[id = "gain"]
    gain: FloatParam,
    #[id = "tone"]
    tone: FloatParam,
    #[id = "level"]
    level: FloatParam,
}

impl Default for Meilichios {
    fn default() -> Self {
        Self {
            params: Arc::new(MeilichiosParams::default()),
            channels: Vec::new(),
        }
    }
}

fn gain_db(name: &str, default_db: f32, min_db: f32, max_db: f32) -> FloatParam {
    FloatParam::new(
        name,
        util::db_to_gain(default_db),
        FloatRange::Skewed {
            min: util::db_to_gain(min_db),
            max: util::db_to_gain(max_db),
            factor: FloatRange::gain_skew_factor(min_db, max_db),
        },
    )
    .with_smoother(SmoothingStyle::Logarithmic(20.0))
    .with_unit(" dB")
    .with_value_to_string(formatters::v2s_f32_gain_to_db(1))
    .with_string_to_value(formatters::s2v_f32_gain_to_db())
}

impl Default for MeilichiosParams {
    fn default() -> Self {
        Self {
            gain: gain_db("Gain", 18.0, 0.0, 36.0),
            tone: FloatParam::new("Tone", 5.0, FloatRange::Linear { min: 0.0, max: 10.0 })
                .with_smoother(SmoothingStyle::Linear(20.0))
                .with_value_to_string(formatters::v2s_f32_rounded(1)),
            level: gain_db("Level", -6.0, -40.0, 12.0),
        }
    }
}

impl Plugin for Meilichios {
    const NAME: &'static str = "meilichios";
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
        self.channels = vec![Drive::new(buffer_config.sample_rate); n];
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
        for frame in buffer.iter_samples() {
            let c = Controls {
                gain: self.params.gain.smoothed.next(),
                tone: self.params.tone.smoothed.next() / 10.0,
                level: self.params.level.smoothed.next(),
            };
            for (i, sample) in frame.into_iter().enumerate() {
                *sample = self.channels[i].process(*sample, &c);
            }
        }
        ProcessStatus::Normal
    }
}

impl ClapPlugin for Meilichios {
    const CLAP_ID: &'static str = "com.dkoutavas.meilichios";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Blues Driver style overdrive");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Distortion,
        ClapFeature::Mono,
        ClapFeature::Stereo,
    ];
}

impl Vst3Plugin for Meilichios {
    const VST3_CLASS_ID: [u8; 16] = *b"MeilichiosDrive1";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Distortion];
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{rms, sine, spectrum_db};

    const FS: f32 = 48000.0;

    fn run(input: &[f32], gain_db: f32, tone: f32) -> Vec<f32> {
        let mut d = Drive::new(FS);
        let c = Controls {
            gain: 10f32.powf(gain_db / 20.0),
            tone,
            level: 1.0,
        };
        input.iter().map(|&x| d.process(x, &c)).collect()
    }

    #[test]
    fn aliasing_stays_below_minus_60_db_at_full_gain() {
        let input = sine(1010.0, FS as f64, 0.3, FS as usize);
        let output = run(&input, 36.0, 0.5);
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
        eprintln!("meilichios aliasing: worst audible line {rel:.1} dB at {} Hz", worst.1 * 10);
        assert!(rel < -60.0, "aliasing at {rel:.1} dB");
    }

    #[test]
    fn silence_in_silence_out_and_no_dc() {
        let output = run(&vec![0.0; 4800], 36.0, 0.5);
        assert!(output.iter().all(|v| v.is_finite() && v.abs() < 1e-6));
        let input = sine(220.0, FS as f64, 0.3, FS as usize);
        let output = run(&input, 36.0, 0.5);
        let tail = &output[output.len() - 4800..];
        let mean = tail.iter().sum::<f32>() / tail.len() as f32;
        assert!(mean.abs() < 1e-3, "DC offset {mean}");
    }

    #[test]
    fn gain_knob_drives_the_clip() {
        let input = sine(220.0, FS as f64, 0.1, FS as usize);
        let clean = rms(&run(&input, 0.0, 0.5)[43200..]);
        let hot = rms(&run(&input, 36.0, 0.5)[43200..]);
        assert!(hot > clean * 4.0, "clean {clean}, hot {hot}");
        // Clipped output cannot pass the knee by much.
        let peak = run(&input, 36.0, 0.5)[43200..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak < DIODE_KNEE_V * 1.1, "peak {peak}");
    }

    #[test]
    fn tone_knob_tilts_the_top() {
        let input = sine(5000.0, FS as f64, 0.05, FS as usize);
        let dark = rms(&run(&input, 0.0, 0.0)[43200..]);
        let bright = rms(&run(&input, 0.0, 1.0)[43200..]);
        let db = 20.0 * (bright / dark).log10();
        assert!(db > 10.0, "tone range at 5 kHz only {db:.1} dB");
    }
}

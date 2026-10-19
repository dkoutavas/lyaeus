//! lyaeus: a guitar tube-amp overdrive, and bromios, the RAT-style pedal
//! that goes in front of it. Both ship in this one bundle. The amp's plugin
//! glue lives here and its sound in `amp.rs`; the pedal is all in `rat.rs`.

use nih_plug::prelude::*;
use std::sync::Arc;

mod amp;
mod filters;
mod oversample;
mod rat;
mod tonestack;
#[cfg(test)]
mod testutil;

use amp::{Channel, Controls};
use rat::Bromios;

pub struct Lyaeus {
    params: Arc<LyaeusParams>,
    channels: Vec<Channel>,
}

#[derive(Params)]
struct LyaeusParams {
    #[id = "gain"]
    gain: FloatParam,
    #[id = "bass"]
    bass: FloatParam,
    #[id = "mid"]
    mid: FloatParam,
    #[id = "treble"]
    treble: FloatParam,
    #[id = "bright"]
    bright: BoolParam,
    #[id = "sag"]
    sag: FloatParam,
    #[id = "master"]
    master: FloatParam,
}

impl Default for Lyaeus {
    fn default() -> Self {
        Self {
            params: Arc::new(LyaeusParams::default()),
            channels: Vec::new(),
        }
    }
}

fn tone_knob(name: &str) -> FloatParam {
    FloatParam::new(name, 5.0, FloatRange::Linear { min: 0.0, max: 10.0 })
        .with_value_to_string(formatters::v2s_f32_rounded(1))
}

impl Default for LyaeusParams {
    fn default() -> Self {
        Self {
            // Stored as linear gain so the smoother runs in the domain the
            // DSP uses; shown in dB.
            gain: FloatParam::new(
                "Gain",
                util::db_to_gain(25.0),
                FloatRange::Skewed {
                    min: util::db_to_gain(0.0),
                    max: util::db_to_gain(50.0),
                    factor: FloatRange::gain_skew_factor(0.0, 50.0),
                },
            )
            .with_smoother(SmoothingStyle::Logarithmic(20.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(1))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),
            bass: tone_knob("Bass"),
            mid: tone_knob("Mid"),
            treble: tone_knob("Treble"),
            bright: BoolParam::new("Bright", false),
            sag: FloatParam::new("Sag", 0.3, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_smoother(SmoothingStyle::Linear(20.0))
                .with_unit("%")
                .with_value_to_string(formatters::v2s_f32_percentage(0))
                .with_string_to_value(formatters::s2v_f32_percentage()),
            master: FloatParam::new(
                "Master",
                util::db_to_gain(-6.0),
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

impl Plugin for Lyaeus {
    const NAME: &'static str = "lyaeus";
    const VENDOR: &'static str = "dkoutavas";
    const URL: &'static str = "https://github.com/dkoutavas/lyaeus";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    // Stereo tracks run as two independent mono amps.
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
        self.channels = vec![Channel::new(buffer_config.sample_rate); n];
        context.set_latency_samples(oversample::LATENCY);
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
        // ponytail: tone knobs update once per block, not per sample. Only
        // audible if you automate them fast; per-sample if that ever matters.
        let treble = self.params.treble.value() / 10.0;
        let mid = self.params.mid.value() / 10.0;
        let bass = self.params.bass.value() / 10.0;
        for ch in &mut self.channels {
            ch.set_tone(treble, mid, bass);
        }

        for frame in buffer.iter_samples() {
            let c = Controls {
                gain: self.params.gain.smoothed.next(),
                bright: self.params.bright.value(),
                sag: self.params.sag.smoothed.next(),
                master: self.params.master.smoothed.next(),
            };
            for (i, sample) in frame.into_iter().enumerate() {
                *sample = self.channels[i].process(*sample, &c);
            }
        }

        ProcessStatus::Normal
    }
}

impl ClapPlugin for Lyaeus {
    const CLAP_ID: &'static str = "com.dkoutavas.lyaeus";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Guitar tube-amp overdrive");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Distortion,
        ClapFeature::Mono,
        ClapFeature::Stereo,
    ];
}

impl Vst3Plugin for Lyaeus {
    const VST3_CLASS_ID: [u8; 16] = *b"LyaeusTubeAmp001";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Distortion];
}

nih_export_clap!(Lyaeus, Bromios);
nih_export_vst3!(Lyaeus, Bromios);

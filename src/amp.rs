//! The amplifier: one channel of audio through two triode-like gain stages,
//! the tone stack, and a sagging power stage, with the nonlinear part run
//! oversampled.
//!
//! Signal path, base rate in square brackets, oversampled rate in braces:
//!
//! ```text
//! [in -> 40 Hz high-pass -> bright shelf -> gain] -> up
//!   {stage 1 -> coupling HP -> Miller LP -> stage 2 -> coupling HP
//!      -> tone stack -> makeup -> power stage with sag}
//! -> down -> [10 Hz high-pass -> master -> out]
//! ```

use crate::filters::OnePole;
use crate::oversample::{Oversampler, FACTOR};
use crate::tonestack::ToneStack;

/// Fixed gain of the second triode stage. The first stage carries the Gain
/// knob; this one is what makes "gain at zero" still a warm, slightly
/// compressed clean rather than a DI.
const STAGE2_GAIN: f32 = 4.0;

/// Gain after the tone stack. A passive stack loses 10 dB or more in the
/// mids; in the real amp the next triode makes that up.
const MAKEUP: f32 = 4.0;

/// How far the Sag knob can pull the power stage ceiling down. At 3.0 a
/// hard-driven stage with Sag at maximum compresses by about 6 dB, enough
/// to hear as a knob and still short of a studio compressor.
const SAG_DEPTH: f32 = 3.0;

/// Per-sample control values, already smoothed by the host-side parameters.
pub struct Controls {
    pub gain: f32,
    pub bright: bool,
    pub sag: f32,
    pub master: f32,
}

/// Asymmetric soft clipper standing in for one triode gain stage.
///
/// Positive input swings drive the grid positive, where grid current clamps
/// them early and fairly hard. Negative swings push the tube toward cutoff,
/// which rounds off more gently. Two different thresholds on one tanh give
/// exactly that asymmetry, and the asymmetry is what produces the even
/// harmonics people call "tube warmth". The stage inverts, like a triode;
/// that flips which side clips first in the next stage. Slope is 1 at zero
/// on both sides, so the curve has no kink.
fn triode(x: f32) -> f32 {
    const GRID: f32 = 0.6;
    const CUTOFF: f32 = 1.0;
    let th = if x >= 0.0 { GRID } else { CUTOFF };
    -th * (x / th).tanh()
}

/// Push-pull output stage whose supply droops under load.
///
/// The envelope follower tracks how hard the stage is driven. "Sag" lowers
/// the clip ceiling with that envelope: a loud chord compresses within a
/// few milliseconds, then swells back as the supply recovers. On a real amp
/// this comes from the rectifier and filter caps not keeping up.
#[derive(Clone)]
struct PowerStage {
    env: f32,
    attack: f32,
    release: f32,
}

impl PowerStage {
    fn new(sample_rate: f32) -> Self {
        let coeff = |ms: f32| 1.0 - (-1000.0 / (ms * sample_rate)).exp();
        Self {
            env: 0.0,
            attack: coeff(5.0),
            release: coeff(150.0),
        }
    }

    fn process(&mut self, x: f32, sag: f32) -> f32 {
        let level = x.abs();
        let coeff = if level > self.env { self.attack } else { self.release };
        self.env += coeff * (level - self.env);
        // ponytail: the droop follows the input level, not the output
        // current. Good enough for the feel; a rectifier model if it ever
        // matters.
        let headroom = 1.0 / (1.0 + SAG_DEPTH * sag * self.env);
        headroom * (x / headroom).tanh()
    }

    fn reset(&mut self) {
        self.env = 0.0;
    }
}

#[derive(Clone)]
pub struct Channel {
    input_hp: OnePole,
    bright_hp: OnePole,
    os: Oversampler,
    couple1: OnePole,
    miller: OnePole,
    couple2: OnePole,
    tone: ToneStack,
    power: PowerStage,
    output_hp: OnePole,
}

impl Channel {
    pub fn new(sample_rate: f32) -> Self {
        let high = sample_rate * FACTOR as f32;
        Self {
            // Tightens the low end before it hits the clippers, like the
            // small input coupling cap on a Marshall's bright channel.
            input_hp: OnePole::new(40.0, sample_rate),
            // Bright switch: +6 dB shelf above 2 kHz (x + highpass(x)).
            bright_hp: OnePole::new(2000.0, sample_rate),
            os: Oversampler::new(),
            // Interstage coupling caps. They also strip the DC that
            // asymmetric clipping creates, and because they take a few
            // milliseconds to do it, a note's attack sits at a different
            // bias than its sustain: the "duck then bloom" of a pushed amp.
            couple1: OnePole::new(20.0, high),
            // Miller capacitance rolls a triode stage off around 6 kHz.
            // Without this between stages the result is fizz, not an amp.
            miller: OnePole::new(6000.0, high),
            couple2: OnePole::new(20.0, high),
            tone: ToneStack::new(high as f64),
            power: PowerStage::new(high),
            output_hp: OnePole::new(10.0, sample_rate),
        }
    }

    /// Tone knobs, each 0..=1. Cheap enough to call once per block.
    pub fn set_tone(&mut self, treble: f32, mid: f32, bass: f32) {
        self.tone.set(treble as f64, mid as f64, bass as f64);
    }

    pub fn process(&mut self, x: f32, c: &Controls) -> f32 {
        let x = self.input_hp.highpass(x);
        let x = if c.bright { x + self.bright_hp.highpass(x) } else { x };
        let mut block = self.os.up(x * c.gain);
        for v in block.iter_mut() {
            let s1 = self.couple1.highpass(triode(*v));
            let s2 = self.couple2.highpass(triode(STAGE2_GAIN * self.miller.lowpass(s1)));
            let t = self.tone.process(s2) * MAKEUP;
            *v = self.power.process(t, c.sag);
        }
        let y = self.os.down(block);
        self.output_hp.highpass(y) * c.master
    }

    pub fn reset(&mut self) {
        self.input_hp.reset();
        self.bright_hp.reset();
        self.os.reset();
        self.couple1.reset();
        self.miller.reset();
        self.couple2.reset();
        self.tone.reset();
        self.power.reset();
        self.output_hp.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{rms, sine, spectrum_db};

    const FS: f32 = 48000.0;

    fn db_to_gain(db: f32) -> f32 {
        10f32.powf(db / 20.0)
    }

    fn run(input: &[f32], gain_db: f32, sag: f32) -> Vec<f32> {
        let mut ch = Channel::new(FS);
        ch.set_tone(0.5, 0.5, 0.5);
        let c = Controls {
            gain: db_to_gain(gain_db),
            bright: false,
            sag,
            master: 1.0,
        };
        input.iter().map(|&x| ch.process(x, &c)).collect()
    }

    /// The one measurement that decides whether the oversampling factor is
    /// enough: every spectral line that is not a harmonic of the input is
    /// aliasing. 1010 Hz at 48 kHz with a 4800-sample window puts the
    /// harmonics exactly on every 101st bin. The frequency is not a round
    /// number on purpose: the aliases of a 1 kHz tone at a 192 kHz rate all
    /// land on multiples of 1 kHz, exactly on top of the harmonics, and the
    /// test would see nothing. 1010 shares no useful factor with 192000.
    #[test]
    fn aliasing_stays_below_minus_60_db_at_full_gain() {
        let input = sine(1010.0, FS as f64, 0.1, FS as usize);
        let output = run(&input, 50.0, 0.0);
        let window = &output[output.len() - 4800..];
        let spec = spectrum_db(window);
        let fundamental = spec[101];
        // Two numbers: the worst line inside the band anyone hears, and the
        // worst line anywhere. Above 18 kHz the decimation filter's
        // transition band lets a little through by design; that region is
        // reported, not judged.
        let mut worst_audible = (f64::NEG_INFINITY, 0usize);
        let mut worst_any = (f64::NEG_INFINITY, 0usize);
        for (k, &level) in spec.iter().enumerate().skip(5) {
            // Hann leakage spreads each line over a few neighbouring bins.
            let near_harmonic = (k % 101 <= 3) || (k % 101 >= 98);
            if near_harmonic {
                continue;
            }
            if level > worst_any.0 {
                worst_any = (level, k);
            }
            if k * 10 < 18000 && level > worst_audible.0 {
                worst_audible = (level, k);
            }
        }
        let audible = worst_audible.0 - fundamental;
        let any = worst_any.0 - fundamental;
        eprintln!(
            "aliasing: worst audible line {audible:.1} dB at {} Hz, worst anywhere {any:.1} dB at {} Hz",
            worst_audible.1 * 10,
            worst_any.1 * 10
        );
        assert!(audible < -60.0, "aliasing at {audible:.1} dB");
    }

    #[test]
    fn silence_in_silence_out() {
        let output = run(&vec![0.0; 4800], 50.0, 1.0);
        assert!(output.iter().all(|v| v.is_finite() && v.abs() < 1e-6));
    }

    #[test]
    fn output_carries_no_dc() {
        let input = sine(220.0, FS as f64, 0.3, FS as usize);
        let output = run(&input, 40.0, 0.3);
        let tail = &output[output.len() - 4800..];
        let mean = tail.iter().sum::<f32>() / tail.len() as f32;
        assert!(mean.abs() < 1e-3, "DC offset {mean}");
    }

    #[test]
    fn sag_compresses_a_held_note() {
        let input = sine(220.0, FS as f64, 0.5, FS as usize);
        let stiff = rms(&run(&input, 30.0, 0.0)[43200..]);
        let saggy = rms(&run(&input, 30.0, 1.0)[43200..]);
        assert!(saggy < stiff * 0.8, "sag {saggy} vs stiff {stiff}");
    }
}

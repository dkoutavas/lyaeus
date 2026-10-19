//! Fender-style passive treble / mid / bass tone stack.
//!
//! Circuit analysis by Yeh and Smith, "Discretization of the '59 Bassman
//! Tone Stack" (DAFx 2006). The coefficient formulas below are theirs, in the
//! form Faust's `tonestacks.lib` carries them. They give the analog transfer
//! function
//!
//! ```text
//! H(s) = (b1 s + b2 s^2 + b3 s^3) / (1 + a1 s + a2 s^2 + a3 s^3)
//! ```
//!
//! which `bilinear3` turns into a digital filter whenever a knob moves.
//!
//! Component values are the Fender Twin Reverb's (AA270). The '59 Bassman
//! set, if you want to compare, is R1 250k, R2 1M, R3 25k, R4 56k,
//! C1 250p, C2 20n, C3 20n.

use crate::filters::{bilinear3, Iir3};

const R1: f64 = 250e3; // treble pot
const R2: f64 = 250e3; // bass pot
const R3: f64 = 10e3; // mid pot
const R4: f64 = 100e3; // slope resistor from the previous stage
const C1: f64 = 120e-12;
const C2: f64 = 100e-9;
const C3: f64 = 47e-9;

#[derive(Clone)]
pub struct ToneStack {
    iir: Iir3,
    sample_rate: f64,
}

impl ToneStack {
    pub fn new(sample_rate: f64) -> Self {
        let mut stack = Self {
            iir: Iir3::new(),
            sample_rate,
        };
        stack.set(0.5, 0.5, 0.5);
        stack
    }

    /// Knob positions, each 0..=1.
    pub fn set(&mut self, treble: f64, mid: f64, bass: f64) {
        let (num, den) = analog_coefficients(treble, mid, bass);
        let (b, a) = bilinear3(num, den, self.sample_rate);
        self.iir.set(b, a);
    }

    pub fn process(&mut self, x: f32) -> f32 {
        self.iir.process(x as f64) as f32
    }

    pub fn reset(&mut self) {
        self.iir.reset();
    }
}

/// Analog numerator and denominator in powers of s for the given knob
/// positions. `t` and `m` are linear pots. The bass pot is an audio-taper
/// pot in the real amp, so its position goes through exp(3.4 (L - 1)): at
/// half rotation the wiper sits at about 18 % of the track.
pub fn analog_coefficients(t: f64, m: f64, bass: f64) -> ([f64; 4], [f64; 4]) {
    let l = (3.4 * (bass - 1.0)).exp();

    let b1 = t * C1 * R1 + m * C3 * R3 + l * (C1 * R2 + C2 * R2) + (C1 * R3 + C2 * R3);

    let b2 = t * (C1 * C2 * R1 * R4 + C1 * C3 * R1 * R4) - m * m * (C1 * C3 * R3 * R3 + C2 * C3 * R3 * R3)
        + m * (C1 * C3 * R1 * R3 + C1 * C3 * R3 * R3 + C2 * C3 * R3 * R3)
        + l * (C1 * C2 * R1 * R2 + C1 * C2 * R2 * R4 + C1 * C3 * R2 * R4)
        + l * m * (C1 * C3 * R2 * R3 + C2 * C3 * R2 * R3)
        + (C1 * C2 * R1 * R3 + C1 * C2 * R3 * R4 + C1 * C3 * R3 * R4);

    let b3 = l * m * (C1 * C2 * C3 * R1 * R2 * R3 + C1 * C2 * C3 * R2 * R3 * R4)
        - m * m * (C1 * C2 * C3 * R1 * R3 * R3 + C1 * C2 * C3 * R3 * R3 * R4)
        + m * (C1 * C2 * C3 * R1 * R3 * R3 + C1 * C2 * C3 * R3 * R3 * R4)
        + t * C1 * C2 * C3 * R1 * R3 * R4
        - t * m * C1 * C2 * C3 * R1 * R3 * R4
        + t * l * C1 * C2 * C3 * R1 * R2 * R4;

    let a1 = (C1 * R1 + C1 * R3 + C2 * R3 + C2 * R4 + C3 * R4) + m * C3 * R3 + l * (C1 * R2 + C2 * R2);

    let a2 = m * (C1 * C3 * R1 * R3 - C2 * C3 * R3 * R4 + C1 * C3 * R3 * R3 + C2 * C3 * R3 * R3)
        + l * m * (C1 * C3 * R2 * R3 + C2 * C3 * R2 * R3)
        - m * m * (C1 * C3 * R3 * R3 + C2 * C3 * R3 * R3)
        + l * (C1 * C2 * R2 * R4 + C1 * C2 * R1 * R2 + C1 * C3 * R2 * R4 + C2 * C3 * R2 * R4)
        + (C1 * C2 * R1 * R4
            + C1 * C3 * R1 * R4
            + C1 * C2 * R3 * R4
            + C1 * C2 * R1 * R3
            + C1 * C3 * R3 * R4
            + C2 * C3 * R3 * R4);

    let a3 = l * m * (C1 * C2 * C3 * R1 * R2 * R3 + C1 * C2 * C3 * R2 * R3 * R4)
        - m * m * (C1 * C2 * C3 * R1 * R3 * R3 + C1 * C2 * C3 * R3 * R3 * R4)
        + m * (C1 * C2 * C3 * R3 * R3 * R4 + C1 * C2 * C3 * R1 * R3 * R3 - C1 * C2 * C3 * R1 * R3 * R4)
        + l * C1 * C2 * C3 * R1 * R2 * R4
        + C1 * C2 * C3 * R1 * R3 * R4;

    ([0.0, b1, b2, b3], [1.0, a1, a2, a3])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{analog_magnitude, digital_magnitude};

    const FS: f64 = 192000.0;

    fn db(x: f64) -> f64 {
        20.0 * x.log10()
    }

    fn analog_db(knobs: (f64, f64, f64), f: f64) -> f64 {
        let (num, den) = analog_coefficients(knobs.0, knobs.1, knobs.2);
        db(analog_magnitude(num, den, f))
    }

    #[test]
    fn digital_matches_analog_below_the_warping_region() {
        for knobs in [(0.5, 0.5, 0.5), (1.0, 0.0, 1.0), (0.0, 1.0, 0.0)] {
            let (num, den) = analog_coefficients(knobs.0, knobs.1, knobs.2);
            let (b, a) = bilinear3(num, den, FS);
            for f in [100.0, 500.0, 2000.0, 5000.0] {
                let diff = db(digital_magnitude(b, a, f, FS)) - db(analog_magnitude(num, den, f));
                assert!(diff.abs() < 0.1, "{knobs:?} at {f} Hz: {diff} dB off");
            }
        }
    }

    #[test]
    fn noon_setting_scoops_the_mids() {
        let noon = (0.5, 0.5, 0.5);
        let mids = analog_db(noon, 500.0);
        assert!(mids < analog_db(noon, 80.0) - 3.0, "no scoop vs bass: {mids}");
        assert!(mids < analog_db(noon, 5000.0) - 3.0, "no scoop vs treble: {mids}");
    }

    #[test]
    fn treble_knob_moves_the_top() {
        let up = analog_db((1.0, 0.5, 0.5), 5000.0);
        let down = analog_db((0.0, 0.5, 0.5), 5000.0);
        assert!(up - down > 6.0, "treble range only {} dB", up - down);
    }

    #[test]
    fn bass_knob_moves_the_bottom() {
        let up = analog_db((0.5, 0.5, 1.0), 80.0);
        let down = analog_db((0.5, 0.5, 0.0), 80.0);
        assert!(up - down > 6.0, "bass range only {} dB", up - down);
    }

    #[test]
    fn passive_network_never_gains() {
        for knobs in [(0.0, 0.0, 0.0), (1.0, 1.0, 1.0), (0.5, 0.5, 0.5), (1.0, 0.0, 1.0)] {
            let mut f = 20.0;
            while f < 20000.0 {
                let g = analog_db(knobs, f);
                assert!(g <= 0.01, "{knobs:?} gains {g} dB at {f} Hz");
                f *= 1.5;
            }
        }
    }
}

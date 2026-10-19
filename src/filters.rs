//! Small linear building blocks: a one-pole filter and a third-order IIR with
//! the bilinear transform that feeds it.

use std::f64::consts::PI;

/// One-pole low-pass, `y += g * (x - y)`. The high-pass is `x - lowpass(x)`,
/// which is what a coupling capacitor does to a signal: it passes the wiggle
/// and blocks the DC.
#[derive(Clone, Copy)]
pub struct OnePole {
    g: f32,
    state: f32,
}

impl OnePole {
    pub fn new(cutoff_hz: f32, sample_rate: f32) -> Self {
        // Exact pole placement for an RC section: the pole sits at
        // exp(-2 pi fc / fs), so g is the distance from that pole to 1.
        let g = 1.0 - (-2.0 * PI as f32 * cutoff_hz / sample_rate).exp();
        Self { g, state: 0.0 }
    }

    pub fn lowpass(&mut self, x: f32) -> f32 {
        self.state += self.g * (x - self.state);
        self.state
    }

    pub fn highpass(&mut self, x: f32) -> f32 {
        x - self.lowpass(x)
    }

    pub fn reset(&mut self) {
        self.state = 0.0;
    }
}

/// Third-order IIR in transposed direct form II. State and coefficients are
/// f64 on purpose: the tone stack runs at the oversampled rate, where its
/// poles crowd against z = 1 and f32 coefficient rounding would move them.
#[derive(Clone, Copy)]
pub struct Iir3 {
    b: [f64; 4],
    a: [f64; 4],
    s: [f64; 3],
}

impl Iir3 {
    pub fn new() -> Self {
        Self {
            b: [1.0, 0.0, 0.0, 0.0],
            a: [1.0, 0.0, 0.0, 0.0],
            s: [0.0; 3],
        }
    }

    /// Coefficients in powers of z^-1, `a[0]` must be 1.
    pub fn set(&mut self, b: [f64; 4], a: [f64; 4]) {
        debug_assert!((a[0] - 1.0).abs() < 1e-12);
        self.b = b;
        self.a = a;
    }

    pub fn process(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.s[0];
        self.s[0] = self.b[1] * x - self.a[1] * y + self.s[1];
        self.s[1] = self.b[2] * x - self.a[2] * y + self.s[2];
        self.s[2] = self.b[3] * x - self.a[3] * y;
        y
    }

    pub fn reset(&mut self) {
        self.s = [0.0; 3];
    }
}

/// Bilinear transform of a third-order analog transfer function.
///
/// Input polynomials are in powers of s (index 0 is the constant term),
/// output polynomials in powers of z^-1 with the denominator normalized so
/// that `a[0] == 1`. Substituting `s = c (1 - z^-1) / (1 + z^-1)` with
/// `c = 2 fs` and clearing the fraction with `(1 + z^-1)^3` gives, for a
/// polynomial `p0 + p1 s + p2 s^2 + p3 s^3`:
///
/// ```text
/// z^0:   p0 + p1 c + p2 c^2 +   p3 c^3
/// z^-1: 3p0 + p1 c - p2 c^2 - 3 p3 c^3
/// z^-2: 3p0 - p1 c - p2 c^2 + 3 p3 c^3
/// z^-3:  p0 - p1 c + p2 c^2 -   p3 c^3
/// ```
///
/// No frequency pre-warping: at the oversampled rate the warping error
/// below 20 kHz is under 4 % and shrinks fast below that.
pub fn bilinear3(num: [f64; 4], den: [f64; 4], sample_rate: f64) -> ([f64; 4], [f64; 4]) {
    let c = 2.0 * sample_rate;
    let expand = |p: [f64; 4]| -> [f64; 4] {
        let (p0, p1, p2, p3) = (p[0], p[1] * c, p[2] * c * c, p[3] * c * c * c);
        [
            p0 + p1 + p2 + p3,
            3.0 * p0 + p1 - p2 - 3.0 * p3,
            3.0 * p0 - p1 - p2 + 3.0 * p3,
            p0 - p1 + p2 - p3,
        ]
    };
    let b = expand(num);
    let a = expand(den);
    let norm = 1.0 / a[0];
    (b.map(|v| v * norm), a.map(|v| v * norm))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{analog_magnitude, digital_magnitude};

    #[test]
    fn one_pole_lowpass_reaches_minus_3db_near_cutoff() {
        // A 1 kHz low-pass fed with a 1 kHz sine should settle near -3 dB.
        let fs = 48000.0;
        let mut lp = OnePole::new(1000.0, fs);
        let n = 48000;
        let mut peak: f32 = 0.0;
        for i in 0..n {
            let x = (2.0 * PI as f32 * 1000.0 * i as f32 / fs).sin();
            let y = lp.lowpass(x);
            if i > n / 2 {
                peak = peak.max(y.abs());
            }
        }
        let db = 20.0 * peak.log10();
        assert!((db - (-3.0)).abs() < 0.3, "got {db} dB");
    }

    #[test]
    fn bilinear_matches_analog_response_at_low_frequency() {
        // A simple analog low-pass 1 / (1 + s / w0) with w0 at 1 kHz.
        let fs = 192000.0;
        let w0 = 2.0 * PI * 1000.0;
        let num = [1.0, 0.0, 0.0, 0.0];
        let den = [1.0, 1.0 / w0, 0.0, 0.0];
        let (b, a) = bilinear3(num, den, fs);
        for f in [100.0, 1000.0, 5000.0] {
            let analog = analog_magnitude(num, den, f);
            let digital = digital_magnitude(b, a, f, fs);
            let diff = 20.0 * (digital / analog).log10();
            assert!(diff.abs() < 0.05, "{f} Hz: {diff} dB off");
        }
    }
}

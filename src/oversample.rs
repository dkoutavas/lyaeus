//! Oversampling around the nonlinear stages.
//!
//! Clipping creates harmonics far above the audible band. At the plain sample
//! rate anything above half the rate folds back below it as inharmonic
//! "fizz" (aliasing). Running the clippers at FACTOR times the rate moves
//! that fold point FACTOR times higher, and the decimation filter removes
//! what lives up there before the signal comes back down.
//!
//! One windowed-sinc low-pass FIR serves both directions: polyphase for the
//! interpolation after zero-stuffing (the stuffed zeros are never multiplied)
//! and plain for the anti-alias pass before decimation. It is linear phase,
//! so each pass delays by exactly (TAPS - 1) / 2 high-rate samples.

use std::f64::consts::PI;

/// 8x. Measured with the aliasing test in `amp.rs` at full gain: 4x left
/// the worst audible alias at -66 dB, 8x at -96 dB, 16x at -104 dB. 8x
/// buys 30 dB for a doubling of a cheap loop; 16x buys 8 more for another.
pub const FACTOR: usize = 8;

/// Filter length. Must be FACTOR * m + 1 so the two passes add up to a whole
/// number of base-rate samples of latency; see `LATENCY`.
const TAPS: usize = 257;
const PHASE_LEN: usize = TAPS / FACTOR + 1;

/// Round-trip latency in base-rate samples: two passes of (TAPS - 1) / 2
/// high-rate samples, divided by FACTOR.
pub const LATENCY: u32 = ((TAPS - 1) / FACTOR) as u32;

#[derive(Clone)]
pub struct Oversampler {
    taps: [f32; TAPS],
    /// `phases[p][k] = FACTOR * taps[FACTOR * k + p]`, zero padded.
    phases: [[f32; PHASE_LEN]; FACTOR],
    /// Base-rate input history, newest first.
    up_hist: [f32; PHASE_LEN],
    /// High-rate history, newest first. FACTOR - 1 extra slots hold the
    /// samples newer than the decimation point (see `down`).
    down_hist: [f32; TAPS + FACTOR - 1],
}

impl Oversampler {
    pub fn new() -> Self {
        let taps = design_lowpass();
        let mut phases = [[0.0; PHASE_LEN]; FACTOR];
        for (i, &h) in taps.iter().enumerate() {
            // Zero-stuffing divides the average level by FACTOR; the gain
            // here puts it back.
            phases[i % FACTOR][i / FACTOR] = h * FACTOR as f32;
        }
        Self {
            taps,
            phases,
            up_hist: [0.0; PHASE_LEN],
            down_hist: [0.0; TAPS + FACTOR - 1],
        }
    }

    /// One base-rate sample in, FACTOR high-rate samples out.
    pub fn up(&mut self, x: f32) -> [f32; FACTOR] {
        self.up_hist.copy_within(0..PHASE_LEN - 1, 1);
        self.up_hist[0] = x;
        let mut out = [0.0; FACTOR];
        for (p, phase) in self.phases.iter().enumerate() {
            out[p] = dot(phase, &self.up_hist);
        }
        out
    }

    /// FACTOR high-rate samples in, one base-rate sample out.
    ///
    /// The output is the filter evaluated at the first of the FACTOR new
    /// samples, not the last. That keeps the round trip an integer number of
    /// base-rate samples; the other FACTOR - 1 samples only feed history.
    pub fn down(&mut self, block: [f32; FACTOR]) -> f32 {
        self.down_hist.copy_within(0..TAPS - 1, FACTOR);
        for (i, &v) in block.iter().enumerate() {
            self.down_hist[FACTOR - 1 - i] = v;
        }
        dot(&self.taps, &self.down_hist[FACTOR - 1..])
    }

    pub fn reset(&mut self) {
        self.up_hist = [0.0; PHASE_LEN];
        self.down_hist = [0.0; TAPS + FACTOR - 1];
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Blackman-windowed sinc low-pass. Blackman gives about -74 dB stopband;
/// with 257 taps at a 384 kHz rate the transition band is about 8 kHz wide.
///
/// The -6 dB point sits at 5/6 of the base-rate Nyquist (20 kHz at 48 kHz),
/// not at Nyquist itself, so the whole transition band lies below Nyquist
/// and everything that would fold back meets the full stopband. The cost is
/// a gentle roll-off from 17 kHz up, which the 6 kHz Miller filter in the
/// preamp has already made irrelevant.
fn design_lowpass() -> [f32; TAPS] {
    let cutoff = (5.0 / 6.0) * 0.5 / FACTOR as f64; // cycles per high-rate sample
    let mid = (TAPS - 1) as f64 / 2.0;
    let mut taps = [0.0f64; TAPS];
    for (i, t) in taps.iter_mut().enumerate() {
        let n = i as f64 - mid;
        let sinc = if n == 0.0 {
            2.0 * cutoff
        } else {
            (2.0 * PI * cutoff * n).sin() / (PI * n)
        };
        let w = 2.0 * PI * i as f64 / (TAPS - 1) as f64;
        let blackman = 0.42 - 0.5 * w.cos() + 0.08 * (2.0 * w).cos();
        *t = sinc * blackman;
    }
    let sum: f64 = taps.iter().sum();
    taps.map(|t| (t / sum) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::sine;

    #[test]
    fn round_trip_is_a_pure_delay() {
        let fs = 48000.0;
        let input = sine(1000.0, fs, 0.5, 2000);
        let mut os = Oversampler::new();
        let output: Vec<f32> = input
            .iter()
            .map(|&x| {
                let high = os.up(x);
                os.down(high)
            })
            .collect();
        let latency = LATENCY as usize;
        let mut worst: f32 = 0.0;
        for n in (latency + TAPS)..input.len() {
            worst = worst.max((output[n] - input[n - latency]).abs());
        }
        assert!(worst < 2e-3, "worst sample error {worst}");
    }

    #[test]
    fn phases_sum_to_unity_gain() {
        // DC through the polyphase interpolator must come out as DC at 1.0.
        let mut os = Oversampler::new();
        let mut last = [0.0; FACTOR];
        for _ in 0..PHASE_LEN {
            last = os.up(1.0);
        }
        for v in last {
            assert!((v - 1.0).abs() < 1e-4, "phase output {v}");
        }
    }
}

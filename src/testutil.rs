//! Helpers for the unit tests only. No FFT crate: a naive DFT over a few
//! thousand samples runs in milliseconds and keeps the dependency list at one.

use std::f64::consts::PI;

pub fn sine(freq: f64, sample_rate: f64, amplitude: f32, n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| amplitude * (2.0 * PI * freq * i as f64 / sample_rate).sin() as f32)
        .collect()
}

pub fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}

/// Hann-windowed magnitude spectrum in dBFS, one entry per bin from 0 to
/// Nyquist. Bin k is at frequency k * fs / len.
pub fn spectrum_db(x: &[f32]) -> Vec<f64> {
    let n = x.len();
    let windowed: Vec<f64> = x
        .iter()
        .enumerate()
        .map(|(i, &v)| v as f64 * 0.5 * (1.0 - (2.0 * PI * i as f64 / n as f64).cos()))
        .collect();
    (0..=n / 2)
        .map(|k| {
            let (mut re, mut im) = (0.0, 0.0);
            for (i, &v) in windowed.iter().enumerate() {
                let phase = 2.0 * PI * k as f64 * i as f64 / n as f64;
                re += v * phase.cos();
                im -= v * phase.sin();
            }
            // Hann window halves the coherent gain, so scale by 4 / n to make
            // a full-scale sine read 0 dBFS.
            20.0 * ((re * re + im * im).sqrt() * 4.0 / n as f64).max(1e-12).log10()
        })
        .collect()
}

/// |H(j w)| of an analog transfer function with polynomials in powers of s.
pub fn analog_magnitude(num: [f64; 4], den: [f64; 4], freq: f64) -> f64 {
    let w = 2.0 * PI * freq;
    let eval = |p: [f64; 4]| {
        // s = j w: s^2 = -w^2 (real), s^3 = -j w^3.
        let re = p[0] - p[2] * w * w;
        let im = p[1] * w - p[3] * w * w * w;
        (re * re + im * im).sqrt()
    };
    eval(num) / eval(den)
}

/// |H(e^{j w})| of a digital filter with polynomials in powers of z^-1.
pub fn digital_magnitude(b: [f64; 4], a: [f64; 4], freq: f64, sample_rate: f64) -> f64 {
    let w = 2.0 * PI * freq / sample_rate;
    let eval = |p: [f64; 4]| {
        let (mut re, mut im) = (0.0, 0.0);
        for (k, &c) in p.iter().enumerate() {
            re += c * (w * k as f64).cos();
            im -= c * (w * k as f64).sin();
        }
        (re * re + im * im).sqrt()
    };
    eval(b) / eval(a)
}

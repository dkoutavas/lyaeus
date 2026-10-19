//! Offline render of the plugin chain over a WAV file, for listening and
//! for measuring when no guitar is at hand.
//!
//! Usage: cargo run --release --example render -- <input.wav> <cab-ir.wav> <out-dir>
//!
//! Both WAVs must be mono 48 kHz 32-bit float (ffmpeg -ac 1 -ar 48000
//! -c:a pcm_f32le). Writes one 16-bit WAV per preset plus a log of what
//! euios detected.

use lyaeus::amp::{Channel, Controls as AmpControls};
use lyaeus::drive::{Controls as DriveControls, Drive};
use lyaeus::feedback::{Controls as FbControls, Feedbacker};
use lyaeus::rat::{feedback_ohms, Controls as RatControls, Pedal};
use std::fs;
use std::io::Write;

const FS: f32 = 48000.0;
/// Naive convolution: the cab IR is cut here. 85 ms holds the body of a
/// guitar cabinet response.
const IR_TAPS: usize = 4096;

fn db(x: f32) -> f32 {
    10f32.powf(x / 20.0)
}

fn read_f32_wav(path: &str) -> Vec<f32> {
    let bytes = fs::read(path).expect("read wav");
    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    let mut pos = 12;
    let mut format = 0u16;
    let mut channels = 0u16;
    let mut bits = 0u16;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = &bytes[pos + 8..(pos + 8 + size).min(bytes.len())];
        if id == b"fmt " {
            format = u16::from_le_bytes(body[0..2].try_into().unwrap());
            channels = u16::from_le_bytes(body[2..4].try_into().unwrap());
            bits = u16::from_le_bytes(body[14..16].try_into().unwrap());
        } else if id == b"data" {
            assert_eq!((format, channels, bits), (3, 1, 32), "need mono 32-bit float");
            return body
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
                .collect();
        }
        pos += 8 + size + (size & 1);
    }
    panic!("no data chunk in {path}");
}

fn write_i16_wav(path: &str, samples: &[f32]) {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(FS as u32).to_le_bytes());
    out.extend_from_slice(&(FS as u32 * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for &s in samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    fs::write(path, out).expect("write wav");
}

/// Peak, overall RMS, and the RMS of the loudest one-second window. The
/// loud window's crest factor (peak minus loud RMS) says how hard the
/// signal is clipped: a sine is 3 dB, a square 0 dB, clean guitar 10+.
fn stats(name: &str, x: &[f32]) {
    let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    let rms = (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
    let win = FS as usize;
    let loud = x
        .chunks(win)
        .map(|c| (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt())
        .fold(0.0f32, f32::max);
    let (peak_db, loud_db) = (20.0 * peak.max(1e-9).log10(), 20.0 * loud.max(1e-9).log10());
    println!(
        "{name:<34} peak {peak_db:6.1}  rms {:6.1}  loud-1s {loud_db:6.1}  crest {:4.1} dB",
        20.0 * rms.max(1e-9).log10(),
        peak_db - loud_db
    );
}

/// IR files come at arbitrary levels (this one carries +17 dB). Scale to
/// unit energy so the cab neither boosts nor cuts overall.
fn normalize(ir: &[f32]) -> Vec<f32> {
    let energy: f32 = ir.iter().map(|v| v * v).sum();
    let k = 1.0 / energy.sqrt();
    ir.iter().map(|v| v * k).collect()
}

fn convolve(x: &[f32], ir: &[f32]) -> Vec<f32> {
    let ir = normalize(&ir[..ir.len().min(IR_TAPS)]);
    let ir = &ir[..];
    let mut y = vec![0.0f32; x.len()];
    for (n, out) in y.iter_mut().enumerate() {
        let taps = ir.len().min(n + 1);
        let mut acc = 0.0;
        for k in 0..taps {
            acc += ir[k] * x[n - k];
        }
        *out = acc;
    }
    y
}

fn amp(x: &[f32], gain_db: f32) -> Vec<f32> {
    let mut ch = Channel::new(FS);
    ch.set_tone(0.5, 0.5, 0.5);
    let c = AmpControls {
        gain: db(gain_db),
        bright: false,
        sag: 0.3,
        master: db(-6.0),
    };
    x.iter().map(|&v| ch.process(v, &c)).collect()
}

fn rat(x: &[f32], distortion: f32) -> Vec<f32> {
    let mut p = Pedal::new(FS);
    p.set_knobs(distortion, 0.5);
    let c = RatControls {
        rf: feedback_ohms(distortion),
        volume: db(-10.0),
    };
    x.iter().map(|&v| p.process(v, &c)).collect()
}

fn blues(x: &[f32], gain_db: f32) -> Vec<f32> {
    let mut d = Drive::new(FS);
    let c = DriveControls {
        gain: db(gain_db),
        tone: 0.5,
        level: db(-6.0),
    };
    x.iter().map(|&v| d.process(v, &c)).collect()
}

fn feedbacker(x: &[f32], onset_s: f32, swell_s: f32, log_path: &str) -> Vec<f32> {
    let mut fb = Feedbacker::new(FS);
    let c = FbControls {
        harmonic: 2,
        onset_s,
        swell_s,
        gain: 0.5,
        wobble: 0.3,
        active: true,
    };
    let mut log = fs::File::create(log_path).expect("log");
    let mut last_stage = "";
    let mut out = Vec::with_capacity(x.len());
    for (n, &v) in x.iter().enumerate() {
        out.push(fb.process(v, &c));
        let (pitch, stage, env) = fb.probe();
        if n % 1024 == 0 || stage != last_stage {
            writeln!(
                log,
                "{:7.3}s  pitch {:>7}  {:8}  env {:6.1} dB",
                n as f32 / FS,
                pitch.map(|p| format!("{p:.1}")).unwrap_or_else(|| "-".into()),
                stage,
                env
            )
            .unwrap();
            last_stage = stage;
        }
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (input, ir_path, out_dir) = (&args[1], &args[2], &args[3]);
    fs::create_dir_all(out_dir).unwrap();
    let x = read_f32_wav(input);
    let ir = read_f32_wav(ir_path);
    println!("input {} s, ir {} taps (using {})", x.len() as f32 / FS, ir.len(), ir.len().min(IR_TAPS));
    stats("dry input", &x);

    let save = |name: &str, y: Vec<f32>| {
        stats(name, &y);
        let cab = convolve(&y, &ir);
        stats(&format!("{name} + cab"), &cab);
        write_i16_wav(&format!("{out_dir}/{name}.wav"), &cab);
    };

    save("01-lyaeus-gain25", amp(&x, 25.0));
    save("02-lyaeus-gain45", amp(&x, 45.0));
    save("03-bromios70-lyaeus10", amp(&rat(&x, 0.7), 10.0));
    save("04-meilichios18-lyaeus10", amp(&blues(&x, 18.0), 10.0));
    let fb = feedbacker(&x, 0.8, 0.6, &format!("{out_dir}/05-euios.log"));
    save("05-euios-bromios70-lyaeus10", amp(&rat(&fb, 0.7), 10.0));
    // Riffs rarely hold a note 0.8 s; a quick onset shows the effect on them.
    let fb = feedbacker(&x, 0.3, 0.3, &format!("{out_dir}/06-euios-fast.log"));
    save("06-euios-fast-bromios70-lyaeus10", amp(&rat(&fb, 0.7), 10.0));
    write_i16_wav(&format!("{out_dir}/00-dry.wav"), &x);
    println!("wrote {out_dir}");
}

# lyaeus

A guitar tube-amp overdrive plugin for Linux: CLAP and VST3, written in Rust
with nih-plug. Lyaeus (Λυαῖος, "the loosener") is an epithet of Dionysus.

The target sound is a cranked Fender-style amp: the kind of amp Sonic Youth
pushed into breakup and then hit with a pedal. This plugin is the amp. Put
your pedal plugins in front of it and a cabinet impulse response after it.

No model and no machine learning. Each stage is a small piece of circuit
math with a comment that says what it stands for and why it is there.

## Install

Requirements: a Rust toolchain and Bitwig Studio (or another CLAP/VST3
host) on native Linux.

```bash
./build-linux.sh            # test, build, install to ~/.clap and ~/.vst3
./build-linux.sh --check    # test and build only
```

Restart the host after each install. A loaded plugin stays in memory, so
the host keeps the old build until it restarts.

In Bitwig: add lyaeus to the guitar track, then add the Convolution device
after it with a guitar cabinet impulse response. Without a cabinet the
output is the raw power stage and sounds thin and fizzy, as a real amp does
through a flat speaker.

## Knobs

| Knob | What it does |
|------|--------------|
| Gain | Drive into the first triode stage, 0 to 50 dB. 0 dB is a warm clean, 50 dB is full roar. |
| Bass, Mid, Treble | A Fender Twin Reverb tone stack, 0 to 10. The knobs interact as on the real amp: all at 5 scoops the mids. |
| Bright | +6 dB shelf above 2 kHz into the first stage, like the Fender bright switch. |
| Sag | How much the power supply droops under load. More sag means more compression and a slower swell after a hard attack. |
| Master | Output level, -40 to +12 dB. |

## Signal path

```text
in -> 40 Hz high-pass -> Bright -> Gain -> 8x up
  -> stage 1 -> coupling HP -> 6 kHz Miller LP -> stage 2 -> coupling HP
  -> tone stack -> makeup gain -> power stage with sag
  -> 8x down -> 10 Hz high-pass -> Master -> out
```

Each triode stage is an asymmetric soft clipper: positive swings clamp
early and hard (grid conduction), negative swings round off gently
(cutoff). The asymmetry creates even harmonics. Each stage inverts, so the
next stage clips the other side first.

The coupling high-pass filters after each stage remove the DC that
asymmetric clipping creates. They take a few milliseconds to do it, so a
note's attack sits at a different bias than its sustain. That is the "duck
then bloom" of a pushed amp, and it falls out of the circuit for free.

The 6 kHz low-pass between the stages stands for the Miller capacitance of
a triode. Without it the second stage clips fizz instead of guitar.

The tone stack is the passive treble/mid/bass network of a Fender Twin
Reverb, computed from its resistor and capacitor values with the circuit
analysis of Yeh and Smith (DAFx 2006). The bass pot has an audio taper in
the amp, and the plugin maps the knob the same way.

The power stage is a symmetric soft clipper whose ceiling follows an
envelope of the signal. Sag sets how far the ceiling drops.

## Oversampling

Clipping creates harmonics far above the audible band. At the plain sample
rate they fold back below Nyquist as inharmonic fizz. The nonlinear path
runs at 8 times the host rate behind a 257-tap windowed-sinc filter, and
the plugin reports 32 samples of latency to the host.

The unit test `aliasing_stays_below_minus_60_db_at_full_gain` measures the
worst non-harmonic spectral line for a 1010 Hz tone at full gain:

| Oversampling | Worst audible alias |
|--------------|---------------------|
| 4x | -66 dB |
| 8x | -96 dB |
| 16x | -104 dB |

8x is the shipped setting. Change `FACTOR` and `TAPS` in `src/oversample.rs`
to try another.

## Tests

```bash
cargo test --release -- --nocapture
```

The tests use no FFT crate; a naive DFT over 4800 samples is fast enough.
They check that the tone stack's digital filter matches the analog
transfer function, that the stack scoops at noon and moves with each knob,
that the oversampler round trip is a pure 32-sample delay, that silence
stays silence, that the output carries no DC, that Sag compresses a held
note, and the aliasing level above.

## Roadmap

- A RAT-style distortion pedal as a second plugin, to go in front.
- A cabinet impulse response loader, if Bitwig's Convolution device ever
  gets in the way.
- A standalone JACK build for tweaking without restarting the host.

## License

GPL-3.0-or-later.

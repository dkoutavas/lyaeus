# lyaeus

Four guitar plugins for Linux in one bundle, CLAP and VST3, written in
Rust with nih-plug. The names are epithets of Dionysus.

- **lyaeus** (Λυαῖος, "the loosener"): a cranked Fender-style tube amp, the
  kind Sonic Youth pushed into breakup.
- **bromios** (Βρόμιος, "the roarer"): a ProCo RAT style distortion pedal.
- **meilichios** (Μειλίχιος, "the gentle one"): a Blues Driver style
  overdrive.
- **euios** (Εὔιος, from the bacchic cry): guitar feedback at bedroom
  volume.

Chain in the host: euios, then a pedal or two, then lyaeus, then a cabinet
impulse response.

No model and no machine learning. Each stage is a small piece of circuit
math with a comment that says what it stands for and why it is there.

## Install

Requirements: a Rust toolchain and Bitwig Studio (or another CLAP/VST3
host) on native Linux. One build installs both plugins.

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

## lyaeus, the amp

### Knobs

| Knob | What it does |
|------|--------------|
| Gain | Drive into the first triode stage, 0 to 50 dB. 0 dB is a warm clean, 50 dB is full roar. |
| Bass, Mid, Treble | A Fender Twin Reverb tone stack, 0 to 10. The knobs interact as on the real amp: all at 5 scoops the mids. |
| Bright | +6 dB shelf above 2 kHz into the first stage, like the Fender bright switch. |
| Sag | How much the power supply droops under load. More sag means more compression and a slower swell after a hard attack. |
| Master | Output level, -40 to +12 dB. |

### Signal path

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

## bromios, the pedal

### Knobs

| Knob | What it does |
|------|--------------|
| Distortion | The RAT's 100k feedback pot, audio taper: half way is about 9 kOhm, not 50. |
| Filter | One-pole low-pass from 32 kHz (0) down to 475 Hz (10). Clockwise is darker, as on the pedal. |
| Volume | Output level, -40 to +12 dB. The diodes hold the signal near 0.65 V, so -10 dB lands around a hot pickup. |

### Signal path

```text
in -> 20 Hz high-pass -> gain network -> feedback-cap low-pass -> 8x up
  -> op-amp (rate-limited one-pole within the rails) -> diode clipper
  -> 8x down -> Filter low-pass -> 5 Hz high-pass -> Volume -> out
```

The model works in volts, with one full-scale sample taken as one volt.

The gain network is the LM308 non-inverting stage with the Distortion pot
as feedback and two RC legs to ground, 47 Ohm + 4.7 uF and 560 Ohm +
2.2 uF. Gain is `1 + Rf/R` above each leg's corner (720 Hz and 130 Hz), so
it reaches 66 dB for the mids and highs but far less for bass. That is why
a RAT stays tight at full tilt.

The op-amp is a rate-limited one-pole. Its 30 pF compensation cap is both
the dominant pole (closed-loop corner = 1 MHz / gain, about 470 Hz at full
Distortion) and the reason the output cannot move faster than 0.3 V/us.
Small errors see a low-pass; large ones ramp at the slew rate. Square edges
become ramps, and that softening is most of what people mean by "the LM308
is the sound". The output stays within +-4 V, the rails on a 9 V supply.

The diode clipper is 1 kOhm into two antiparallel 1N914 diodes. The node
voltage solves `(vin - v) / R = 2 Is sinh(v / nVt)`, which has no closed
form, so the plugin solves it with Newton's method once per grid point at
start-up and interpolates a table in the audio loop.

### What the measurements taught

The first op-amp model was a low-pass followed by a plain slew limiter. It
aliased at -54 dB, and neither antiderivative anti-aliasing on the diode
knee nor sub-sample ramp starts moved that by more than 4 dB. Bypassing
blocks one at a time found the source: the hand-over between "ramping" and
"tracking" snapped to the sample grid. The rate-limited one-pole has no
such hand-over, its slope is continuous, and the same test now reads
-93 dB. Antiderivative anti-aliasing was then measured again: 1 dB better,
so it was removed.

| Op-amp model | Worst audible alias at 8x |
|--------------|---------------------------|
| low-pass, then slew limiter | -54 dB |
| same, plus ADAA on the diode knee | -58 dB |
| rate-limited one-pole | -93 dB |

## meilichios, the overdrive

### Knobs

| Knob | What it does |
|------|--------------|
| Gain | 0 to 36 dB into the clip. |
| Tone | Boss-style blend between a low-pass and a high-pass at 1 kHz. 5 is flat, 0 dark, 10 bright and thin. |
| Level | Output, -40 to +12 dB. |

A character model, not a schematic trace. The clip is a tanh with its knee
at 1.2 V, two diodes in series each way as in the BD-2's feedback loop,
followed by a square-law bend standing for the FET output stage: a little
asymmetry, even harmonics, "amp-like". It keeps the bass and does not hump
the mids. Use it as a clean boost into lyaeus at low Gain or stacked into
bromios.

## euios, the feedbacker

### Knobs

| Knob | What it does |
|------|--------------|
| Harmonic | Which harmonic of the held note feeds back: 1 to 5. 2 is the octave, the classic. |
| Onset | How long a note must be held before feedback starts, 0.1 to 3 s. |
| Swell | How long the feedback takes to reach full level, 0.1 to 2 s. |
| Gain | Feedback level, up to about a hot pickup's. |
| Wobble | A few cents of slow pitch wander, as real feedback has. |
| Active | The footswitch. Automate it or map it to a MIDI controller. |

Real feedback is the speaker shaking the string at one of its modes until
the loop gain passes one; a harmonic swells in and takes over. There is no
acoustic loop inside a plugin chain, so euios follows the DigiTech FreqOut
idea: it reads the pitch of the note you hold, waits the Onset time, then
swells in a sine at the chosen harmonic and lets the pedals and the amp
after it do what a loud room would. A fed-back string mode is close to a
sine anyway.

It only sounds while a note is held, so there is no idle hiss. Once
feedback is established it keeps going until you mute the strings; a
decaying note does not stop it, a palm mute does. A new note restarts the
Onset clock. Bends and vibrato are followed with a 50 ms lag.

Pitch detection is YIN (de Cheveigné and Kawahara, 2002) on a 1024-sample
window, refreshed every 1024 samples, 70 to 1000 Hz. The tests read sines
across the guitar range and a Karplus-Strong plucked string to within 1 %
and call white noise unvoiced. Put euios first in the chain, on the clean
guitar.

## Oversampling

Clipping creates harmonics far above the audible band. At the plain sample
rate they fold back below Nyquist as inharmonic fizz. The nonlinear path
runs at 8 times the host rate behind a 257-tap windowed-sinc filter, and
the plugin reports 32 samples of latency to the host.

The aliasing tests measure the worst non-harmonic spectral line for a
1010 Hz tone at full gain or full distortion:

| Oversampling | lyaeus | bromios | meilichios |
|--------------|--------|---------|------------|
| 4x | -66 dB | | |
| 8x | -96 dB | -93 dB | -124 dB |
| 16x | -104 dB | | |

8x is the shipped setting for all three. euios has no clipper and no
oversampling. Change `FACTOR` and `TAPS` in `src/oversample.rs`
to try another.

## Tests

```bash
cargo test --release -- --nocapture
```

The tests use no FFT crate; a naive DFT over 4800 samples is fast enough.
For the amp they check that the tone stack's digital filter matches the
analog transfer function, that the stack scoops at noon and moves with
each knob, that the oversampler round trip is a pure 32-sample delay, that
silence stays silence, that the output carries no DC, that Sag compresses
a held note, and the aliasing level above. For the pedal they check the
gain network's 66 dB and its bass roll-off, the op-amp's slew rate and
small-signal pole, the diode table against the solver, silence, DC, and
the aliasing level. For meilichios: aliasing, silence, DC, the clip
ceiling, the tone range. For euios: YIN on sines, on a plucked string and
on noise; silence; the octave swelling in after the onset and dying after
a mute; a new note restarting the clock.

## Offline render

To hear the chain on a recording without a host, or to measure it:

```bash
ffmpeg -i take.wav -ac 1 -ar 48000 -c:a pcm_f32le in.wav
ffmpeg -i cab.wav -ac 1 -ar 48000 -c:a pcm_f32le ir.wav
cargo run --release --example render -- in.wav ir.wav renders
```

It writes one 16-bit WAV per preset with the cab applied, prints peak,
RMS and crest factor per stage, and logs what euios detected.

## Roadmap

- A cabinet impulse response loader, if Bitwig's Convolution device ever
  gets in the way.
- A standalone JACK build for tweaking without restarting the host.

## License

GPL-3.0-or-later.

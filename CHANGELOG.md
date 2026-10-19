# Changelog

## 0.2.0 - 2026-10-19

Adds bromios, a ProCo RAT style distortion pedal, as a second plugin in the
same bundle.

- Gain stage with the RAT's two RC ground legs: up to 66 dB above 720 Hz, far less for bass.
- LM308 modeled as a rate-limited one-pole: 0.3 V/us slew and the closed-loop pole from one compensation cap, output held within the rails.
- 1 kOhm into two 1N914 diodes, solved once into a lookup table.
- Filter knob from 32 kHz down to 475 Hz, clockwise darker, like the pedal.
- Aliasing at full Distortion measured at -93 dB with 8x oversampling.

## 0.1.0 - 2026-10-19

First build.

- Two asymmetric triode-style gain stages with coupling and Miller filters.
- Fender Twin Reverb tone stack from the Yeh and Smith circuit analysis.
- Push-pull power stage with a Sag control.
- Bright switch, Gain and Master in dB.
- 8x oversampling around the nonlinear path, 32 samples of reported latency.
- Thirteen unit tests, including an aliasing measurement at full gain.

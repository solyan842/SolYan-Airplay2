# SolYan AirPlay2

Native Windows sender for streaming PC audio to Apple HomePod / HomePod mini using real AirPlay 2.

## v0.1.0 development target

- AirPlay/AirPlay 2 discovery with receiver feature parsing
- Native Windows WASAPI loopback capture
- 44.1 kHz / 16-bit / stereo PCM capture path aligned to 352-frame audio chunks
- HomePod capability reporting: buffered audio, PTP, transient pairing
- Real AirPlay 2 pairing/session/transport as the next engine milestone
- Portable Windows build through GitHub Actions

## Engine

The AirPlay 2 protocol layer is based on `lmcgartland/airplay2-rs`, pinned to:

`a2f980cf25bf13ae20158497cc2d2ea69cd88fa7`

This project is therefore licensed under GPL-3.0-or-later. See `LICENSE` and `THIRD_PARTY_NOTICES.md`.

## Test the current build

Run without arguments to discover AirPlay receivers:

```powershell
.\solyan-airplay.exe
```

The output reports the model, source version, raw AirPlay feature mask and whether the receiver advertises AirPlay 2 buffered audio, PTP and transient pairing.

To verify Windows system-audio capture, play audio on the PC and run:

```powershell
.\solyan-airplay.exe --capture-test
```

The test captures the default Windows render device for five seconds and reports the number of PCM chunks and peak sample value.

## Latency direction

The low-latency target is not "zero latency". The design prioritizes the HomePod receiver's real AirPlay 2 capabilities, with buffered audio + PTP where supported, then measures the minimum stable render lead time on hardware.

<!-- v0.2.2-ui-polish-build: clean-logo + collapsible-diagnostics + fixed-footer -->

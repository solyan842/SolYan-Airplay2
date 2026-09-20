# SolYan AirPlay2 Stability Contract (v0.2.17)

This document is the implementation contract for the stable 16-bit / 44.1 kHz path.
It exists to prevent later UI/features work from reintroducing startup and timeline bugs.

## Reference implementations

The design is intentionally split by protocol family:

- Music Assistant `airplay-cli` DESIGN.md and native AP2 implementation are the
  primary reference for native AirPlay 2 session lifecycle, feedback cadence,
  buffered-before-start behavior, starvation handling and warm timeline rules.
- `pyatv` RAOP code is used as a reference for RTP pacing, retransmit history,
  sync cadence and receiver latency behavior. Its legacy RAOP RECORD/FLUSH order
  is not copied into the native AP2 route.
- The vendored `airplay2-rs` commit remains the protocol/crypto/timing base;
  SolYan carries local stability patches where upstream behavior is unsafe for
  continuous Windows loopback audio.

## Native AP2 startup invariants

1. Establish pairing/verification and session SETUP.
2. Send a bare RECORD on the session URL.
3. Perform audio stream SETUP and require HTTP 200.
4. Start Windows loopback capture in native shared-mode format.
5. Prime approximately 450 ms of source-time PCM before opening RTP. Prime with
   real captured PCM whenever available; use source-cadence silence only while
   Windows genuinely has no block.
6. Start the RTP streamer only after the internal audio buffer has runway.
7. Never send FLUSH on a fresh session. FLUSH is a warm-boundary operation for
   an already-running timeline.

## Runtime invariants

- RTP sequence/timestamp continuity is sacred while the session is armed.
- A live-source starvation never skips an RTP packet. The streamer emits an
  encoded-silence packet on the same timeline and micro-fades the first real PCM
  after recovery.
- RTSP control work never blocks the PCM producer.
- Feedback runs independently about every 2 seconds.
- One feedback timeout is tolerated; three consecutive failures mark the
  control session unhealthy.
- Synthetic silence is paced by source audio time and cannot build an arbitrary
  backlog in front of the next track.
- WASAPI event loss is recovered by polling the pending capture buffer every
  20 ms, not 250 ms.
- The live handoff queue is deliberately bounded and shallow; the internal
  streamer buffer provides the main jitter runway.

## Stable wire format

- ALAC
- 16-bit
- 44.1 kHz
- stereo
- 352 frames per RTP packet

24-bit and 48 kHz experiments are explicitly out of scope for the stable path.

## Validation before merging to stable/*

A candidate must pass CI and real HomePod tests:

1. 10 cold Start -> audible -> Stop cycles on one HomePod.
2. Music already playing before Start: first transient must not stumble.
3. Start while Windows is silent, then begin playback.
4. Pause/silence 60 seconds, then resume without reconnect.
5. At least 20 consecutive track changes without stream death.
6. One-hour continuous playback with no RTP timeline reset or unbounded latency growth.
7. Temporary local network disturbance: short feedback misses must not stop audio;
   persistent control loss must surface as a clear session-health failure.
8. Stereo-pair testing only after single-speaker tests pass.

Do not merge a build to `stable/*` solely because CI passes; hardware playback
is part of the stability gate.

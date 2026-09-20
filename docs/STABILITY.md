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


## Runtime recovery (v0.2.18)

- A Windows WASAPI capture failure must not tear down a healthy AirPlay RTP
  session immediately. The live RTP clock continues with encoded silence while
  SolYan releases the failed capture client, re-selects the default render
  endpoint and opens a fresh loopback capture client.
- The recovered capture format must remain stereo i16-normalized at the same
  native source sample rate. A mid-session mix-rate change is surfaced clearly
  instead of silently feeding the resampler with the wrong clock rate.
- Feedback timeouts are control-health signals, not proof that RTP is dead.
  Three consecutive timeouts mark control as degraded but do not stop audio.
  A non-timeout/hard RTSP failure remains terminal.
- The GUI executable name is stable: `SolYan-AirPlay2.exe`. Version numbers
  belong in file metadata and release artifact names, reducing repeated Windows
  Firewall prompts caused only by versioned executable paths/names.


## PTP timeline ownership (v0.2.19)

Single-speaker native AirPlay 2 uses a persistent sender-owned PTP timeline.

- UDP 319/320 are bound before Session SETUP. Failure to bind is fatal.
- One 64-bit ClockID is derived per RTSP session and used both in
  timingPeerInfo/timingPeerList and every PTP packet identity.
- timingPeerInfo includes DeviceType=0, ClockID, Addresses and
  SupportsClockPortMatchingOverride=false.
- SolYan sends SETPEERS with receiver + sender addresses after stream SETUP.
- The old single-speaker BMCA-yield path is disabled.
- Sender priority1=246 holds grandmaster against the observed HomePod priority.
- Sync + Follow_Up are sent unicast every 125ms; Announce refreshes about
  every 2s; Delay_Req is answered with Delay_Resp.
- Source silence, track changes and WASAPI recovery do not change the PTP
  grandmaster or reset RTP sequence/timestamp continuity.

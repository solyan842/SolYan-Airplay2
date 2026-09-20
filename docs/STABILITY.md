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


## Native AirPlay 2 gPTP contract (v0.2.21)

v0.2.21 replaces the experimental PTP assumptions from earlier development
builds with the packet contract measured by current Music Assistant airplay-cli
and corroborated by OwnTone/libairptp.

### Stable sender identity

- SolYan has one persistent sender Device-ID/DACP-ID stored under
  %LOCALAPPDATA%/SolYan/AirPlay2.
- Transient HomePod sessions reuse that ID instead of generating a new sender
  on every Start.
- timingPeerInfo.ID and PTP ClockID are deterministically derived from the same
  sender identity.
- Pair-verify reuses the persisted paired sender identity.

### gPTP wire invariants

- PTP is IEEE 802.1AS/gPTP: majorSdoId=1 in the high nibble of byte 0.
- SourcePortIdentity uses the sender ClockID and iOS-style portNumber 0x8005.
- Sender-grandmaster Announce dataset:
  priority1=128, priority2=128, clockClass=6, clockAccuracy=0x21,
  offsetScaledLogVariance=0x436A, timeSource=0x20.
- Sync/Follow_Up is two-step and sent every 125ms.
- Announce and Apple Signaling are refreshed every 1s.
- Announce contains PATH_TRACE.
- Follow_Up contains both IEEE 802.1AS Follow_Up Information and Apple ClockID
  organization-extension TLVs.
- REQUEST_UNICAST_TRANSMISSION is answered with GRANT_UNICAST_TRANSMISSION.
- Delay_Req is answered on UDP 320.
- Pdelay_Req is answered with Pdelay_Resp on UDP 319 and
  Pdelay_Resp_Follow_Up on UDP 320.
- UDP 319/320 must bind successfully. A native AP2 session does not silently
  fall back to ephemeral PTP ports.

### Clock ownership routing

Normal AirPlay 2 receivers, older HomePods, stereo-pair members and
Apple-TV-routed speakers use the sender-owned gPTP grandmaster.

A standalone HomePod is routed to receiver-clock follow mode only when all of
these discovered properties hold:

- model begins with AudioAccessory
- osvers major >= 27
- igl = 1
- no pgid
- no tsid

In follow mode SolYan binds gPTP before Session SETUP, sends SETPEERS, then
waits for the receiver's Announce + Sync/Follow_Up. Streaming is not considered
timing-ready until both the receiver grandmaster ClockID and a real clock offset
are locked. Failure to lock is a clear setup error, not a silent Playing state.

### Removed direction

The v0.2.20 experimental source-silence warm re-anchor is not part of this
branch. A live realtime session keeps one continuous RTP/timing line; timing
ownership is repaired at the gPTP layer instead of repeatedly reseating the
render anchor after ordinary source silence.


## Native AP2 idle keepalive contract (v0.2.22)

The native AirPlay 2 feedback keepalive is a global RTSP control endpoint:

- Send exactly `POST /feedback`.
- Do not derive the path from the session URI.
- Do not attach a request body or Content-Type when no feedback body exists.
- Feedback runs on an independent control task and never blocks PCM/RTP.
- Do not wrap an encrypted RTSP exchange in a shorter outer cancellation
  timeout. Cancelling mid-HAP-frame can leave the next read starting inside a
  previous encrypted frame.
- HTTP status misses are treated as degraded keepalive health while RTP
  continues; transport/framing failures remain hard control-channel failures.

This fixes the pre-v0.2.22 behavior that sent
`rtsp://receiver/SESSION_UUID/feedback` instead of the native `/feedback`
keepalive endpoint.


## Service supervisor and RTP heartbeat (v0.2.23)

The Windows GUI now treats live streaming as a long-lived service rather than
a one-shot task.

- If run_live_stream exits unexpectedly while the user has not pressed Stop,
  the worker does not return the UI to Idle.
- The service automatically reconstructs the AirPlay session with bounded
  reconnect backoff: 250ms, 500ms, 1s, then 2s maximum.
- User Stop remains authoritative and terminates the supervisor cleanly.
- The UI logs AUTO-RECOVERY with attempt number, reason and retry delay.

The live core also verifies the wire heartbeat:

- packets_sent must continue advancing even during source silence because
  encoded-silence RTP packets keep the session clock alive.
- If an observed packets_sent counter does not advance for 3 seconds while the
  live service is active, the run is declared stalled.
- The supervisor then rebuilds the session automatically instead of leaving the
  application in a permanently silent state.
- SERVICE HEALTH telemetry is emitted once per second with captured chunks,
  synthetic silence chunks, live queue depth, packets_sent, feedback streak,
  control health and capture restart count.

This is intentionally receiver-agnostic and covers HomePod, AirPort and other
receivers that exhibit the same long-idle failure.

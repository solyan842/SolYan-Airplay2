# SolYan AirPlay Engine V1 — clean rewrite

This branch is a clean-room project rewrite inside the existing repository.

## Non-negotiable rules

1. Track/source lifetime is not transport lifetime.
2. Pause is not disconnect.
3. Starvation is not EOF.
4. After native AirPlay 2 starts, RTP sequence and RTP timestamp never stop because PCM input is temporarily absent.
5. A native realtime session owns one immutable RTP-to-wall-clock anchor for its lifetime.
6. Warm boundaries (pause/resume/seek/next/source change/starvation) do not FLUSH, teardown, re-pair, reset sequence numbers, reset crypto, or create a new anchor.
7. Silence and music travel through the same packetizer and the same RTP clock.
8. Route is resolved from receiver capabilities before the session starts:
   - RAOP
   - AirPlay 2 RAOP-compatible
   - AirPlay 2 native
9. AirPort/legacy RAOP and HomePod/native AP2 are different transports and must not be forced through one state machine.
10. GUI and WASAPI are clients of the engine. They must never own protocol/session state.

## Reference behavior

The architecture is modeled on the current music-assistant/airplay-cli design:

- one persistent PCM input for process/session lifetime;
- route selection before connect;
- native AP2 splice timeline;
- encoded silence to prevent an armed realtime lane from running dry;
- connection and input remain open across next-track/seek;
- RAOP and AP2-native have separate protocol paths.

The implementation is rewritten for native Windows/Rust and does not reuse the previous SolYan vendor engine.

## Layering

Windows capture
-> PCM normalizer
-> persistent PCM ring
-> packet clock
-> route-specific transport

Route-specific transports:

- RAOP transport
- AP2 RAOP-compatible transport
- AP2 native transport

Shared services:

- discovery/capability parser
- ALAC encoder
- HAP credentials store
- absolute-deadline scheduler
- PTP/NTP clock service
- metrics

## Required acceptance tests before GUI integration

- cold start with audio already playing;
- cold start while source is silent, then audio begins;
- silence 15 s / 30 s / 60 s then resume;
- WAV -> FLAC -> WAV;
- offline player -> YouTube -> offline;
- repeated next-track transitions;
- source endpoint sample-rate change;
- 30+ minute continuous playback;
- AirPort route;
- HomePod route.

No GUI integration until these engine-level tests are structurally supported.

# SolYan AirPlay Engine V1

Clean rewrite of the SolYan Windows AirPlay sender.

This branch intentionally starts from the repository bootstrap commit and does **not** reuse the previous AirPlay client, audio streamer, RTSP session orchestration, timing engine, or vendor tree.

Current phase: engine invariants first.

- Route separation: RAOP / AirPlay 2 compat / AirPlay 2 native
- Persistent PCM ring
- Immutable native RTP timeline
- Silence is data, never EOF
- Source/track transitions do not own the AirPlay session

See `docs/ARCHITECTURE-V1.md`.

Target audio profile for the first stable engine:

- ALAC
- 16-bit
- 44.1 kHz
- stereo
- 352 frames per packet

GUI/WASAPI integration will be added only after the new transport core passes the session-lifecycle acceptance tests.

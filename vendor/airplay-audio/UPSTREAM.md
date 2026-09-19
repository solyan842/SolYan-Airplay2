# Upstream provenance

This directory vendors `crates/airplay-audio` from:

- Repository: https://github.com/lmcgartland/airplay2-rs
- Revision: `a2f980cf25bf13ae20158497cc2d2ea69cd88fa7`
- License: GPL-3.0-or-later

## SolYan patch

Only the RTP socket QoS helper is changed for portability:

- Unix keeps the upstream DSCP/send-buffer behavior.
- Non-Unix platforms, including Windows, skip the optional QoS hint.
- RTP packet formats, encryption, retransmission, codec handling, and timing logic are unchanged.

This patch exists because the pinned upstream implementation uses Unix-only
`AsRawFd` / `libc::setsockopt` APIs in `rtp.rs`.

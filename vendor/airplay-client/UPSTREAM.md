# Vendored airplay-client

Source: https://github.com/lmcgartland/airplay2-rs
Pinned upstream commit: a2f980cf25bf13ae20158497cc2d2ea69cd88fa7

SolYan patch:
- single live startup is FLUSH -> RECORD ACK -> start sender
- FLUSH and RECORD require HTTP 200
- HomePod group startup requires every member to acknowledge FLUSH/RECORD

Kept local so the stable SolYan sender does not depend on an upstream startup ordering bug.

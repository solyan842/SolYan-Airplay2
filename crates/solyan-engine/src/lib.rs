//! SolYan AirPlay engine rewrite.
//!
//! This crate intentionally does not depend on the previous SolYan AirPlay
//! protocol/audio engine. It starts from transport invariants first.

pub mod pcm_ring;
pub mod route;
pub mod timeline;

pub use pcm_ring::{PcmRing, PushResult};
pub use route::{ReceiverCapabilities, Route, RouteResolver};
pub use timeline::{Boundary, Timeline, TimelineError, TimelineState};

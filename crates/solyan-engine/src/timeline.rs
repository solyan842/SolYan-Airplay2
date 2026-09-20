#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineState {
    Cold,
    Armed,
    Running,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Boundary {
    FirstStart,
    Pause,
    Resume,
    NextTrack,
    Seek,
    SourceChange,
    Starvation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineError {
    AlreadyAnchored,
    NotAnchored,
    Stopped,
}

#[derive(Debug, Clone)]
pub struct Timeline {
    state: TimelineState,
    anchor_rtp: Option<u64>,
    anchor_wall_ns: Option<u64>,
    head_rtp: u64,
}

impl Default for Timeline {
    fn default() -> Self {
        Self {
            state: TimelineState::Cold,
            anchor_rtp: None,
            anchor_wall_ns: None,
            head_rtp: 0,
        }
    }
}

impl Timeline {
    pub fn state(&self) -> TimelineState {
        self.state
    }

    pub fn anchor(&self) -> Option<(u64, u64)> {
        self.anchor_rtp.zip(self.anchor_wall_ns)
    }

    pub fn head_rtp(&self) -> u64 {
        self.head_rtp
    }

    pub fn anchor_once(&mut self, rtp: u64, wall_ns: u64) -> Result<(), TimelineError> {
        if self.state == TimelineState::Stopped {
            return Err(TimelineError::Stopped);
        }
        if self.anchor_rtp.is_some() {
            return Err(TimelineError::AlreadyAnchored);
        }
        self.anchor_rtp = Some(rtp);
        self.anchor_wall_ns = Some(wall_ns);
        self.head_rtp = rtp;
        self.state = TimelineState::Armed;
        Ok(())
    }

    pub fn start(&mut self) -> Result<(), TimelineError> {
        if self.anchor_rtp.is_none() {
            return Err(TimelineError::NotAnchored);
        }
        if self.state == TimelineState::Stopped {
            return Err(TimelineError::Stopped);
        }
        self.state = TimelineState::Running;
        Ok(())
    }

    pub fn advance(&mut self, frames: u32) -> Result<u64, TimelineError> {
        if self.state != TimelineState::Running {
            return Err(TimelineError::NotAnchored);
        }
        let at = self.head_rtp;
        self.head_rtp = self.head_rtp.wrapping_add(frames as u64);
        Ok(at)
    }

    pub fn warm_boundary(&mut self, _boundary: Boundary) -> Result<(), TimelineError> {
        if self.state == TimelineState::Stopped {
            return Err(TimelineError::Stopped);
        }
        if self.anchor_rtp.is_none() {
            return Err(TimelineError::NotAnchored);
        }
        Ok(())
    }

    pub fn stop(&mut self) {
        self.state = TimelineState::Stopped;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_is_immutable_for_session_lifetime() {
        let mut t = Timeline::default();
        t.anchor_once(10_000, 123_000_000).unwrap();
        assert_eq!(
            t.anchor_once(20_000, 456_000_000),
            Err(TimelineError::AlreadyAnchored)
        );
        assert_eq!(t.anchor(), Some((10_000, 123_000_000)));
    }

    #[test]
    fn pause_next_seek_source_change_and_starvation_do_not_reanchor() {
        let mut t = Timeline::default();
        t.anchor_once(1_000, 9_000).unwrap();
        t.start().unwrap();
        let anchor = t.anchor();

        for boundary in [
            Boundary::Pause,
            Boundary::Resume,
            Boundary::NextTrack,
            Boundary::Seek,
            Boundary::SourceChange,
            Boundary::Starvation,
        ] {
            t.warm_boundary(boundary).unwrap();
            assert_eq!(t.anchor(), anchor);
            assert_eq!(t.state(), TimelineState::Running);
        }
    }

    #[test]
    fn silence_and_music_share_one_rtp_head() {
        let mut t = Timeline::default();
        t.anchor_once(0, 1).unwrap();
        t.start().unwrap();

        assert_eq!(t.advance(352).unwrap(), 0);
        assert_eq!(t.advance(352).unwrap(), 352);
        assert_eq!(t.advance(352).unwrap(), 704);
        assert_eq!(t.head_rtp(), 1056);
    }
}

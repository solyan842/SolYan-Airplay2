use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushResult {
    Stored,
    DroppedOldest,
}

pub struct PcmRing {
    samples: VecDeque<i16>,
    capacity_samples: usize,
}

impl PcmRing {
    pub fn new(capacity_samples: usize) -> Self {
        assert!(capacity_samples > 0);
        Self {
            samples: VecDeque::with_capacity(capacity_samples),
            capacity_samples,
        }
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn push(&mut self, input: &[i16]) -> PushResult {
        let mut result = PushResult::Stored;
        for &sample in input {
            if self.samples.len() == self.capacity_samples {
                self.samples.pop_front();
                result = PushResult::DroppedOldest;
            }
            self.samples.push_back(sample);
        }
        result
    }

    pub fn pop_or_silence(&mut self, samples_needed: usize) -> Vec<i16> {
        let mut out = Vec::with_capacity(samples_needed);
        for _ in 0..samples_needed {
            out.push(self.samples.pop_front().unwrap_or(0));
        }
        out
    }

    pub fn source_changed(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starvation_is_silence_not_eof() {
        let mut ring = PcmRing::new(4096);
        let packet = ring.pop_or_silence(704);
        assert_eq!(packet.len(), 704);
        assert!(packet.iter().all(|&s| s == 0));
    }

    #[test]
    fn source_change_does_not_destroy_buffer() {
        let mut ring = PcmRing::new(16);
        ring.push(&[1, 2, 3, 4]);
        ring.source_changed();
        assert_eq!(ring.pop_or_silence(4), vec![1, 2, 3, 4]);
    }

    #[test]
    fn bounded_ring_drops_oldest_not_transport() {
        let mut ring = PcmRing::new(4);
        assert_eq!(ring.push(&[1, 2, 3, 4]), PushResult::Stored);
        assert_eq!(ring.push(&[5]), PushResult::DroppedOldest);
        assert_eq!(ring.pop_or_silence(4), vec![2, 3, 4, 5]);
    }
}

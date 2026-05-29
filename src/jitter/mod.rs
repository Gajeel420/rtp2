use std::collections::BTreeMap;

use crate::rtp::RtpPacket;

/// Sequence-number jitter buffer with configurable depth.
pub struct JitterBuffer {
    buffer: BTreeMap<u16, RtpPacket>,
    next_seq: u16,
    initialized: bool,
    max_depth: usize,
}

impl JitterBuffer {
    pub fn new(max_depth: usize) -> Self {
        Self {
            buffer: BTreeMap::new(),
            next_seq: 0,
            initialized: false,
            max_depth,
        }
    }

    /// Insert a packet and return any packets that are now ready for output
    /// (in sequence order).
    pub fn push(&mut self, pkt: RtpPacket) -> Vec<RtpPacket> {
        if !self.initialized {
            self.next_seq = pkt.sequence_number;
            self.initialized = true;
        }

        self.buffer.insert(pkt.sequence_number, pkt);

        let mut output = Vec::new();

        while let Some(p) = self.buffer.remove(&self.next_seq) {
            self.next_seq = self.next_seq.wrapping_add(1);
            output.push(p);
        }

        while self.buffer.len() >= self.max_depth {
            if let Some((&oldest_seq, _)) = self.buffer.iter().next() {
                if let Some(p) = self.buffer.remove(&oldest_seq) {
                    log::warn!(
                        "Jitter buffer full: forcing seq {} (expected {})",
                        oldest_seq,
                        self.next_seq
                    );
                    self.next_seq = oldest_seq.wrapping_add(1);
                    output.push(p);
                }
            } else {
                break;
            }
        }

        output
    }

    /// Flush all buffered packets regardless of order (e.g. on stream end).
    pub fn flush(&mut self) -> Vec<RtpPacket> {
        let mut output: Vec<RtpPacket> = self.buffer.values().cloned().collect();
        self.buffer.clear();
        output.sort_by_key(|p| p.sequence_number);
        output
    }
}

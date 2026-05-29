use std::collections::HashMap;

use bytes::Bytes;

/// Reassembled Annex-B NAL units ready for output.
pub type NaluVec = Vec<Vec<u8>>;

/// H.264 RTP depacketizer (RFC 6184).
pub struct H264Depacketizer {
    /// Fragments accumulated for FU-A; keyed by (ssrc, start_seq).
    fu_buffers: HashMap<(u32, u16), Vec<u8>>,
    /// Tracks the start_seq for ongoing FU-A per SSRC.
    fu_start_seq: HashMap<u32, u16>,
}

impl H264Depacketizer {
    pub fn new() -> Self {
        Self {
            fu_buffers: HashMap::new(),
            fu_start_seq: HashMap::new(),
        }
    }

    /// Process one RTP payload. Returns zero or more complete NAL units (raw bytes,
    /// without Annex-B start code — the caller prepends `\x00\x00\x00\x01`).
    pub fn push(&mut self, ssrc: u32, seq: u16, payload: Bytes) -> NaluVec {
        if payload.is_empty() {
            return vec![];
        }

        let nal_hdr = payload[0];
        let nal_type = nal_hdr & 0x1F;

        match nal_type {
            1..=23 => {
                vec![payload.to_vec()]
            }
            24 => {
                self.parse_stap_a(&payload[1..])
            }
            28 => {
                self.parse_fu_a(ssrc, seq, nal_hdr, &payload[1..])
            }
            _ => {
                log::warn!("Unsupported NAL type {}, dropping packet", nal_type);
                vec![]
            }
        }
    }

    fn parse_stap_a(&self, data: &[u8]) -> NaluVec {
        let mut nalus = Vec::new();
        let mut i = 0;
        while i + 2 <= data.len() {
            let size = u16::from_be_bytes([data[i], data[i + 1]]) as usize;
            i += 2;
            if i + size > data.len() {
                log::warn!("STAP-A NALU size {} exceeds remaining bytes", size);
                break;
            }
            nalus.push(data[i..i + size].to_vec());
            i += size;
        }
        nalus
    }

    fn parse_fu_a(&mut self, ssrc: u32, seq: u16, nal_hdr: u8, fu_payload: &[u8]) -> NaluVec {
        if fu_payload.is_empty() {
            return vec![];
        }

        let fu_hdr = fu_payload[0];
        let start_bit = (fu_hdr & 0x80) != 0;
        let end_bit = (fu_hdr & 0x40) != 0;
        let nal_type = fu_hdr & 0x1F;

        let fragment = &fu_payload[1..];

        if start_bit {
            let reconstructed_hdr = (nal_hdr & 0xE0) | nal_type;
            let mut buf = Vec::with_capacity(fragment.len() + 1);
            buf.push(reconstructed_hdr);
            buf.extend_from_slice(fragment);
            self.fu_start_seq.insert(ssrc, seq);
            self.fu_buffers.insert((ssrc, seq), buf);

            if end_bit {
                let start = self.fu_start_seq[&ssrc];
                if let Some(complete) = self.fu_buffers.remove(&(ssrc, start)) {
                    self.fu_start_seq.remove(&ssrc);
                    return vec![complete];
                }
            }
        } else if let Some(&start_seq) = self.fu_start_seq.get(&ssrc) {
            if let Some(buf) = self.fu_buffers.get_mut(&(ssrc, start_seq)) {
                buf.extend_from_slice(fragment);

                if end_bit {
                    if let Some(complete) = self.fu_buffers.remove(&(ssrc, start_seq)) {
                        self.fu_start_seq.remove(&ssrc);
                        return vec![complete];
                    }
                }
            }
        } else {
            log::warn!("FU-A middle/end fragment with no start (ssrc={}, seq={})", ssrc, seq);
        }

        vec![]
    }
}

impl Default for H264Depacketizer {
    fn default() -> Self {
        Self::new()
    }
}

pub mod depacketizer;

use anyhow::{bail, Result};
use bytes::Bytes;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct RtpPacket {
    pub version: u8,
    pub payload_type: u8,
    pub sequence_number: u16,
    pub timestamp: u32,
    pub ssrc: u32,
    pub payload: Bytes,
}

/// Parse an RTP packet per RFC 3550.
pub fn parse_rtp(data: Bytes) -> Result<RtpPacket> {
    if data.len() < 12 {
        bail!("RTP packet too short: {} bytes", data.len());
    }

    let version = (data[0] >> 6) & 0x03;
    if version != 2 {
        bail!("Unsupported RTP version: {}", version);
    }

    let has_padding = (data[0] & 0x20) != 0;
    let has_extension = (data[0] & 0x10) != 0;
    let csrc_count = (data[0] & 0x0F) as usize;
    let payload_type = data[1] & 0x7F;
    let sequence_number = u16::from_be_bytes([data[2], data[3]]);
    let timestamp = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let ssrc = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);

    let mut offset = 12 + csrc_count * 4;
    if offset > data.len() {
        bail!("RTP header overflows packet length");
    }

    if has_extension {
        if offset + 4 > data.len() {
            bail!("RTP extension header truncated");
        }
        let ext_len = u16::from_be_bytes([data[offset + 2], data[offset + 3]]) as usize;
        offset += 4 + ext_len * 4;
    }

    if offset > data.len() {
        bail!("RTP extension overflows packet length");
    }

    let mut payload_end = data.len();
    if has_padding && payload_end > offset {
        let pad_len = data[payload_end - 1] as usize;
        if pad_len == 0 || pad_len > payload_end - offset {
            bail!("Invalid RTP padding length");
        }
        payload_end -= pad_len;
    }

    let payload = data.slice(offset..payload_end);

    Ok(RtpPacket {
        version,
        payload_type,
        sequence_number,
        timestamp,
        ssrc,
        payload,
    })
}

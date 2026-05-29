use std::sync::{
    atomic::{AtomicU32, Ordering},
    Arc,
};

use crate::sdp::TrackInfo;

/// RTSP session state shared between the client and demux task.
#[derive(Debug)]
pub struct RtspSession {
    /// Monotonically increasing command sequence number.
    pub cseq: Arc<AtomicU32>,
    /// Session identifier returned by the server in the `Session:` header.
    pub session_id: Option<String>,
    /// Tracks parsed from the SDP (populated after DESCRIBE).
    pub tracks: Vec<TrackInfo>,
}

impl RtspSession {
    pub fn new() -> Self {
        Self {
            cseq: Arc::new(AtomicU32::new(1)),
            session_id: None,
            tracks: Vec::new(),
        }
    }

    /// Fetch-and-increment CSeq.
    pub fn next_cseq(&self) -> u32 {
        self.cseq.fetch_add(1, Ordering::SeqCst)
    }
}

impl Default for RtspSession {
    fn default() -> Self {
        Self::new()
    }
}

/// A parsed RTSP response.
#[derive(Debug)]
pub struct RtspResponse {
    pub status: u16,
    pub reason: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl RtspResponse {
    /// Retrieve the value of the first matching header (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        let name_lower = name.to_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| k.to_lowercase() == name_lower)
            .map(|(_, v)| v.as_str())
    }
}

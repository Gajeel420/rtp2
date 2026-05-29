use anyhow::{bail, Result};

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TrackInfo {
    pub control_url: String,
    pub media_type: String,
    pub payload_type: u8,
}

/// Parse an SDP body and extract media track information.
///
/// `base_url` is the RTSP URL used for the DESCRIBE request (or the Content-Base
/// header value if present in the response) and is used to resolve relative
/// `a=control:` attributes.
pub fn parse_sdp(sdp: &str, base_url: &str) -> Result<Vec<TrackInfo>> {
    let mut tracks = Vec::new();
    let mut current_media: Option<(String, u8)> = None;
    let mut current_control: Option<String> = None;

    let base = base_url.trim_end_matches('/');

    for line in sdp.lines() {
        let line = line.trim_end_matches('\r');

        if let Some(rest) = line.strip_prefix("m=") {
            if let Some((media_type, pt)) = current_media.take() {
                let control = current_control.take().unwrap_or_default();
                let resolved = resolve_control(base, &control);
                tracks.push(TrackInfo {
                    control_url: resolved,
                    media_type,
                    payload_type: pt,
                });
            }
            current_control = None;

            let parts: Vec<&str> = rest.splitn(4, ' ').collect();
            if parts.len() >= 4 {
                let media_type = parts[0].to_string();
                let pt: u8 = parts[3]
                    .split_whitespace()
                    .next()
                    .unwrap_or("0")
                    .parse()
                    .unwrap_or(0);
                current_media = Some((media_type, pt));
            }
        } else if let Some(rest) = line.strip_prefix("a=control:") {
            current_control = Some(rest.to_string());
        }
    }

    if let Some((media_type, pt)) = current_media.take() {
        let control = current_control.take().unwrap_or_default();
        let resolved = resolve_control(base, &control);
        tracks.push(TrackInfo {
            control_url: resolved,
            media_type,
            payload_type: pt,
        });
    }

    if tracks.is_empty() {
        bail!("No media tracks found in SDP");
    }

    Ok(tracks)
}

fn resolve_control(base: &str, control: &str) -> String {
    if control.starts_with("rtsp://") || control.starts_with("rtsps://") {
        control.to_string()
    } else if control == "*" || control.is_empty() {
        base.to_string()
    } else {
        format!("{}/{}", base, control)
    }
}

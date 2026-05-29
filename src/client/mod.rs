pub mod demux;
pub mod session;

use anyhow::{bail, Context, Result};
use bytes::Bytes;
use tokio::io::AsyncWriteExt;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch};
use tokio::time::{self, Duration};
use url::Url;

use crate::auth::DigestAuthenticator;
use crate::jitter::JitterBuffer;
use crate::output::FrameSink;
use crate::rtp::{depacketizer::H264Depacketizer, parse_rtp};
use crate::sdp::{parse_sdp, TrackInfo};
use crate::transport::{bind_rtp_pair, spawn_udp_recv_task};

use self::demux::demux_task;
use self::session::{RtspResponse, RtspSession};

const USER_AGENT: &str = "rtp2/0.1";
const KEEPALIVE_SECS: u64 = 30;
const RESP_TIMEOUT_SECS: u64 = 15;
const RTP_CHANNEL_DEPTH: usize = 512;
const RESP_CHANNEL_DEPTH: usize = 16;
const JITTER_MAX_DEPTH: usize = 50;

pub struct RtspClient {
    url: String,
    auth: DigestAuthenticator,
    session: RtspSession,
    writer: OwnedWriteHalf,
    rtp_rx: mpsc::Receiver<Bytes>,
    resp_rx: mpsc::Receiver<String>,
    error_rx: watch::Receiver<Option<String>>,
    use_udp: bool,
}

impl RtspClient {
    pub async fn connect_and_ingest(rtsp_url: &str, use_udp: bool) -> Result<()> {
        let parsed = Url::parse(rtsp_url)
            .with_context(|| format!("Invalid RTSP URL: {}", rtsp_url))?;

        let host = parsed
            .host_str()
            .with_context(|| "URL has no host")?
            .to_string();
        let port = parsed.port().unwrap_or(554);
        let username = if parsed.username().is_empty() {
            String::new()
        } else {
            parsed.username().to_string()
        };
        let password = parsed.password().unwrap_or("").to_string();

        let mut clean_url = parsed.clone();
        let _ = clean_url.set_username("");
        let _ = clean_url.set_password(None);
        let request_url = clean_url.to_string();

        log::info!("Connecting to {}:{}", host, port);
        let stream = TcpStream::connect(format!("{}:{}", host, port))
            .await
            .with_context(|| format!("TCP connect failed to {}:{}", host, port))?;

        let (reader, writer) = stream.into_split();

        let (rtp_tx, rtp_rx) = mpsc::channel::<Bytes>(RTP_CHANNEL_DEPTH);
        let (resp_tx, resp_rx) = mpsc::channel::<String>(RESP_CHANNEL_DEPTH);
        let (error_tx, error_rx) = watch::channel::<Option<String>>(None);

        tokio::spawn(demux_task(reader, rtp_tx, resp_tx, error_tx));

        let auth = DigestAuthenticator::new(username, password);
        let session = RtspSession::new();

        let mut client = RtspClient {
            url: request_url,
            auth,
            session,
            writer,
            rtp_rx,
            resp_rx,
            error_rx,
            use_udp,
        };

        client.handshake().await?;
        client.run_pipeline().await
    }

    async fn handshake(&mut self) -> Result<()> {
        self.send_and_receive("OPTIONS", &self.url.clone(), &[], None)
            .await
            .context("OPTIONS failed")?;

        let describe_resp = self
            .send_with_auth("DESCRIBE", &self.url.clone(), &["Accept: application/sdp"])
            .await
            .context("DESCRIBE failed")?;

        let base_url = describe_resp
            .header("Content-Base")
            .unwrap_or(&self.url)
            .trim_end_matches('/')
            .to_string();

        let sdp_body = String::from_utf8_lossy(&describe_resp.body).into_owned();
        if sdp_body.trim().is_empty() {
            bail!("DESCRIBE response has empty SDP body");
        }

        let tracks = parse_sdp(&sdp_body, &base_url).context("SDP parse failed")?;
        log::info!("Found {} track(s) in SDP", tracks.len());
        self.session.tracks = tracks;

        for i in 0..self.session.tracks.len() {
            self.setup_track(i).await?;
        }

        let session_id = self.session.session_id.clone().unwrap_or_default();
        let play_hdrs = [
            format!("Session: {}", session_id),
            "Range: npt=0.000-".to_string(),
        ];
        let hdrs_refs: Vec<&str> = play_hdrs.iter().map(|s| s.as_str()).collect();
        self.send_with_auth("PLAY", &self.url.clone(), &hdrs_refs)
            .await
            .context("PLAY failed")?;

        log::info!("PLAY sent — stream starting");
        Ok(())
    }

    async fn setup_track(&mut self, track_index: usize) -> Result<()> {
        let track: TrackInfo = self.session.tracks[track_index].clone();

        let transport_header;
        if self.use_udp {
            let (_rtp_sock, _rtcp_sock, rtp_port, rtcp_port) = bind_rtp_pair().await?;
            transport_header = format!(
                "Transport: RTP/AVP;unicast;client_port={}-{}",
                rtp_port, rtcp_port
            );
        } else {
            let channel_rtp = (track_index * 2) as u8;
            let channel_rtcp = channel_rtp + 1;
            transport_header = format!(
                "Transport: RTP/AVP/TCP;unicast;interleaved={}-{}",
                channel_rtp, channel_rtcp
            );
        }

        let session_extra;
        let mut extra_hdrs: Vec<&str> = vec![transport_header.as_str()];
        if let Some(sid) = &self.session.session_id {
            session_extra = format!("Session: {}", sid);
            extra_hdrs.push(session_extra.as_str());
        }

        let resp = self
            .send_with_auth("SETUP", &track.control_url, &extra_hdrs)
            .await
            .with_context(|| format!("SETUP failed for track {}", track.control_url))?;

        if self.session.session_id.is_none() {
            if let Some(sid) = resp.header("Session") {
                let id = sid.split(';').next().unwrap_or(sid).trim().to_string();
                self.session.session_id = Some(id);
            }
        }

        if self.use_udp {
            let (_rtp_sock2, _rtcp_sock2, rtp_port2, _rtcp_port2) = bind_rtp_pair().await?;
            log::info!("UDP RTP listening on port {}", rtp_port2);
            let _ = rtp_port2;
        }

        Ok(())
    }

    async fn run_pipeline(&mut self) -> Result<()> {
        let mut sink = FrameSink::stdout();
        let mut depacketizer = H264Depacketizer::new();
        let mut jitter = JitterBuffer::new(JITTER_MAX_DEPTH);
        let mut keepalive_interval = time::interval(Duration::from_secs(KEEPALIVE_SECS));
        keepalive_interval.tick().await;

        loop {
            tokio::select! {
                _ = self.error_rx.changed() => {
                    if let Some(err) = self.error_rx.borrow().as_deref() {
                        bail!("Demux error: {}", err);
                    }
                }

                maybe_data = self.rtp_rx.recv() => {
                    match maybe_data {
                        Some(data) => {
                            if let Err(e) = self.process_rtp_data(data, &mut depacketizer, &mut jitter, &mut sink).await {
                                log::warn!("RTP processing error: {}", e);
                            }
                        }
                        None => {
                            log::info!("RTP channel closed — stream ended");
                            for pkt in jitter.flush() {
                                let nalus = depacketizer.push(pkt.ssrc, pkt.sequence_number, pkt.payload);
                                for nalu in nalus {
                                    let _ = sink.write_nalu(&nalu).await;
                                }
                            }
                            return Ok(());
                        }
                    }
                }

                _ = keepalive_interval.tick() => {
                    if let Err(e) = self.send_keepalive().await {
                        log::warn!("Keepalive failed: {}", e);
                    }
                }
            }
        }
    }

    async fn process_rtp_data(
        &mut self,
        data: Bytes,
        depacketizer: &mut H264Depacketizer,
        jitter: &mut JitterBuffer,
        sink: &mut FrameSink,
    ) -> Result<()> {
        let pkt = parse_rtp(data)?;
        let packets = jitter.push(pkt);
        for p in packets {
            let nalus = depacketizer.push(p.ssrc, p.sequence_number, p.payload);
            for nalu in nalus {
                sink.write_nalu(&nalu).await?;
            }
        }
        Ok(())
    }

    async fn send_keepalive(&mut self) -> Result<()> {
        let session_id = self.session.session_id.clone().unwrap_or_default();
        let session_hdr = format!("Session: {}", session_id);
        let url = self.url.clone();

        match self
            .send_and_receive("GET_PARAMETER", &url, &[session_hdr.as_str()], None)
            .await
        {
            Ok(_) => Ok(()),
            Err(_) => {
                self.send_and_receive("OPTIONS", &url, &[], None).await?;
                Ok(())
            }
        }
    }

    async fn send_with_auth(
        &mut self,
        method: &str,
        url: &str,
        extra_headers: &[&str],
    ) -> Result<RtspResponse> {
        let resp = self
            .send_and_receive(method, url, extra_headers, None)
            .await?;

        if resp.status == 401 {
            if let Some(www_auth) = resp.header("WWW-Authenticate").map(|s| s.to_string()) {
                self.auth
                    .parse_challenge(&www_auth)
                    .context("Failed to parse Digest challenge")?;

                let auth_value = self
                    .auth
                    .apply(method, url)
                    .context("Auth state missing after parsing challenge")?;

                return self
                    .send_and_receive(method, url, extra_headers, Some(&auth_value))
                    .await;
            }
            bail!("401 Unauthorized but no WWW-Authenticate header");
        }

        if resp.status < 200 || resp.status >= 300 {
            bail!("{} {} → {} {}", method, url, resp.status, resp.reason);
        }

        Ok(resp)
    }

    async fn send_and_receive(
        &mut self,
        method: &str,
        url: &str,
        extra_headers: &[&str],
        auth_header: Option<&str>,
    ) -> Result<RtspResponse> {
        let cseq = self.session.next_cseq();

        let mut request = format!("{} {} RTSP/1.0\r\nCSeq: {}\r\nUser-Agent: {}\r\n", method, url, cseq, USER_AGENT);

        if let Some(auth) = auth_header {
            request.push_str(&format!("Authorization: {}\r\n", auth));
        } else if self.auth.is_challenged() {
            if let Some(auth_val) = self.auth.apply(method, url) {
                request.push_str(&format!("Authorization: {}\r\n", auth_val));
            }
        }

        for h in extra_headers {
            request.push_str(h);
            request.push_str("\r\n");
        }
        request.push_str("\r\n");

        log::debug!(">>> {}", request.lines().next().unwrap_or(""));
        AsyncWriteExt::write_all(&mut self.writer, request.as_bytes())
            .await
            .context("Failed to write RTSP request")?;

        self.recv_response().await
    }

    async fn recv_response(&mut self) -> Result<RtspResponse> {
        let raw = tokio::time::timeout(
            Duration::from_secs(RESP_TIMEOUT_SECS),
            self.resp_rx.recv(),
        )
        .await
        .context("Timed out waiting for RTSP response")?
        .context("Response channel closed")?;

        parse_response(&raw)
    }
}

fn parse_response(raw: &str) -> Result<RtspResponse> {
    let (header_part, body_part) = if let Some(pos) = raw.find("\r\n\r\n") {
        (&raw[..pos], &raw[pos + 4..])
    } else {
        (raw, "")
    };

    let mut lines = header_part.lines();

    let status_line = lines.next().unwrap_or("").trim();
    let mut parts = status_line.splitn(3, ' ');
    let _version = parts.next().unwrap_or("");
    let status: u16 = parts.next().unwrap_or("0").parse().unwrap_or(0);
    let reason = parts.next().unwrap_or("").to_string();

    log::debug!("<<< {} {}", status, reason);

    let mut headers: Vec<(String, String)> = Vec::new();
    for line in lines {
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some(last) = headers.last_mut() {
                last.1.push(' ');
                last.1.push_str(line.trim());
            }
        } else if let Some(colon) = line.find(':') {
            let key = line[..colon].trim().to_string();
            let value = line[colon + 1..].trim().to_string();
            headers.push((key, value));
        }
    }

    let body = body_part.as_bytes().to_vec();

    Ok(RtspResponse {
        status,
        reason,
        headers,
        body,
    })
}

#[allow(dead_code)]
pub async fn start_udp_ingestion(
    rtp_tx: mpsc::Sender<Bytes>,
    tracks: &[TrackInfo],
) -> Result<()> {
    for track in tracks {
        log::info!("Starting UDP ingestion for track: {}", track.control_url);
        let (rtp_sock, _rtcp_sock, rtp_port, _rtcp_port) = bind_rtp_pair().await?;
        log::info!("Bound UDP RTP on port {}", rtp_port);
        spawn_udp_recv_task(rtp_sock, rtp_tx.clone());
    }
    Ok(())
}

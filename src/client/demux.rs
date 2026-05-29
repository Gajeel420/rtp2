use anyhow::Result;
use bytes::Bytes;
use tokio::io::AsyncReadExt;
use tokio::net::tcp::OwnedReadHalf;
use tokio::sync::{mpsc, watch};

/// Runs as a dedicated task. Reads from a TCP socket and routes:
/// - Interleaved RTP/RTCP frames (starting with `$`) → `rtp_tx`
/// - RTSP text responses (everything else up to `\r\n\r\n`) → `resp_tx`
///
/// On any IO error the error is placed into `error_tx` and the task exits.
pub async fn demux_task(
    mut reader: OwnedReadHalf,
    rtp_tx: mpsc::Sender<Bytes>,
    resp_tx: mpsc::Sender<String>,
    error_tx: watch::Sender<Option<String>>,
) {
    if let Err(e) = demux_loop(&mut reader, &rtp_tx, &resp_tx).await {
        let msg = e.to_string();
        log::error!("Demux task error: {}", msg);
        let _ = error_tx.send(Some(msg));
    }
}

async fn demux_loop(
    reader: &mut OwnedReadHalf,
    rtp_tx: &mpsc::Sender<Bytes>,
    resp_tx: &mpsc::Sender<String>,
) -> Result<()> {
    loop {
        let first = reader.read_u8().await?;

        if first == b'$' {
            // Interleaved binary frame: $ | channel (1 byte) | length (2 bytes BE)
            let _channel = reader.read_u8().await?;
            let length = reader.read_u16().await? as usize;

            let mut payload = vec![0u8; length];
            reader.read_exact(&mut payload).await?;

            if rtp_tx.send(Bytes::from(payload)).await.is_err() {
                return Ok(());
            }
        } else {
            // RTSP text response. Accumulate until \r\n\r\n.
            let mut accum: Vec<u8> = vec![first];

            loop {
                let b = reader.read_u8().await?;
                accum.push(b);

                if accum.ends_with(b"\r\n\r\n") {
                    break;
                }
            }

            let header_text = String::from_utf8_lossy(&accum).into_owned();
            let content_length = parse_content_length(&header_text);

            if content_length > 0 {
                let mut body = vec![0u8; content_length];
                reader.read_exact(&mut body).await?;

                let body_text = String::from_utf8_lossy(&body).into_owned();
                let full = format!("{}{}", header_text, body_text);

                if resp_tx.send(full).await.is_err() {
                    return Ok(());
                }
            } else if resp_tx.send(header_text).await.is_err() {
                return Ok(());
            }
        }
    }
}

fn parse_content_length(response_text: &str) -> usize {
    for line in response_text.lines() {
        if let Some(rest) = line.strip_prefix("Content-Length:") {
            if let Ok(n) = rest.trim().parse::<usize>() {
                return n;
            }
        } else if let Some(rest) = line.strip_prefix("content-length:") {
            if let Ok(n) = rest.trim().parse::<usize>() {
                return n;
            }
        }
    }
    0
}

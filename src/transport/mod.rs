use anyhow::Result;
use bytes::Bytes;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// Bind a UDP socket pair for RTP (even port) and RTCP (odd port = rtp_port + 1).
///
/// Returns (rtp_socket, rtcp_socket, rtp_port, rtcp_port).
pub async fn bind_rtp_pair() -> Result<(UdpSocket, UdpSocket, u16, u16)> {
    let rtp_sock = UdpSocket::bind("0.0.0.0:0").await?;
    let rtp_port = rtp_sock.local_addr()?.port();

    let rtcp_port = rtp_port.wrapping_add(1);
    let rtcp_sock = match UdpSocket::bind(format!("0.0.0.0:{}", rtcp_port)).await {
        Ok(s) => s,
        Err(_) => {
            log::warn!(
                "Could not bind RTCP on port {}, binding on any port",
                rtcp_port
            );
            UdpSocket::bind("0.0.0.0:0").await?
        }
    };
    let actual_rtcp_port = rtcp_sock.local_addr()?.port();

    Ok((rtp_sock, rtcp_sock, rtp_port, actual_rtcp_port))
}

/// Spawn a tokio task that reads datagrams from `sock` and forwards raw bytes
/// into `tx`. The task exits when the channel is closed.
#[allow(dead_code)]
pub fn spawn_udp_recv_task(sock: UdpSocket, tx: mpsc::Sender<Bytes>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65535];
        loop {
            match sock.recv_from(&mut buf).await {
                Ok((n, _addr)) => {
                    let data = Bytes::copy_from_slice(&buf[..n]);
                    if tx.send(data).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    log::error!("UDP recv error: {}", e);
                    break;
                }
            }
        }
    })
}

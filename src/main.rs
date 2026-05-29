mod auth;
mod client;
mod jitter;
mod output;
mod rtp;
mod sdp;
mod transport;

use std::process;

use client::RtspClient;

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args: Vec<String> = std::env::args().collect();

    let (url, use_udp) = match parse_args(&args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: {}", e);
            eprintln!("Usage: rtp2 [--udp] <rtsp://[user:pass@]host[:port]/path>");
            process::exit(1);
        }
    };

    log::info!("Starting RTSP ingest from: {}", url);

    if let Err(e) = RtspClient::connect_and_ingest(&url, use_udp).await {
        eprintln!("Fatal: {}", e);
        process::exit(1);
    }
}

fn parse_args(args: &[String]) -> Result<(String, bool), String> {
    let mut use_udp = false;
    let mut url: Option<String> = None;

    for arg in args.iter().skip(1) {
        match arg.as_str() {
            "--udp" => use_udp = true,
            _ if arg.starts_with("rtsp://") || arg.starts_with("rtsps://") => {
                url = Some(arg.clone());
            }
            _ => {
                return Err(format!("Unrecognised argument: {}", arg));
            }
        }
    }

    match url {
        Some(u) => Ok((u, use_udp)),
        None => Err("No RTSP URL provided".to_string()),
    }
}

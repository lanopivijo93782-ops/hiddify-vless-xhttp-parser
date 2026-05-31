//! Reachability / latency testing and an optional real-throughput test.
//!
//! * Latency test (always available): measures the TCP handshake time to the
//!   node's `host:port`. It needs no proxy core and is used to rank nodes.
//! * Throughput test (opt-in): routes a download of a test URL through the node
//!   using a sing-box core binary and measures the achieved speed.

use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use futures::StreamExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

use crate::singbox;
use crate::vless::VlessNode;

/// Resolve the first usable IP for `host`. Accepts IP literals directly.
pub async fn resolve_first(host: &str, port: u16) -> Option<IpAddr> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Some(ip);
    }
    match tokio::net::lookup_host((host, port)).await {
        Ok(mut addrs) => addrs.next().map(|s| s.ip()),
        Err(_) => None,
    }
}

/// Measure the TCP connect latency to `addr`. Returns `None` on failure/timeout.
pub async fn tcp_latency(addr: SocketAddr, timeout: Duration) -> Option<Duration> {
    let start = Instant::now();
    match tokio::time::timeout(timeout, TcpStream::connect(addr)).await {
        Ok(Ok(_stream)) => Some(start.elapsed()),
        _ => None,
    }
}

/// Outcome of testing a single node.
#[derive(Debug, Clone)]
pub struct TestedNode {
    pub node: VlessNode,
    pub ip: IpAddr,
    pub provider: String,
    pub latency: Option<Duration>,
    /// Download speed in kilobits/sec when a real throughput test was run.
    pub throughput_kbps: Option<f64>,
}

impl TestedNode {
    /// Whether the node passed its reachability check.
    pub fn is_reachable(&self) -> bool {
        self.latency.is_some()
    }

    /// Sort score: prefer measured throughput, then lower latency.
    /// Higher is better.
    pub fn score(&self) -> f64 {
        if let Some(kbps) = self.throughput_kbps {
            // Throughput dominates; scale so it always outranks latency-only.
            1_000_000.0 + kbps
        } else if let Some(lat) = self.latency {
            // Map latency to a positive score (lower latency -> higher score).
            10_000.0 - (lat.as_millis() as f64).min(10_000.0)
        } else {
            -1.0
        }
    }
}

/// Run a real download test through `node` using a sing-box `core_bin`.
///
/// Spawns a local SOCKS proxy via sing-box, downloads `test_url` through it for
/// up to `duration`, and returns the measured speed in kbps.
pub async fn throughput_via_core(
    node: &VlessNode,
    core_bin: &Path,
    test_url: &str,
    socks_port: u16,
    duration: Duration,
    work_dir: &Path,
) -> Result<f64> {
    let config = singbox::build_test_config(node, socks_port);
    let cfg_path = work_dir.join(format!("singbox-{socks_port}.json"));
    tokio::fs::write(&cfg_path, serde_json::to_vec_pretty(&config)?)
        .await
        .with_context(|| format!("writing {}", cfg_path.display()))?;

    let mut child = tokio::process::Command::new(core_bin)
        .arg("run")
        .arg("-c")
        .arg(&cfg_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("spawning core {}", core_bin.display()))?;

    // Ensure the child is cleaned up no matter how we exit.
    let result = run_download(socks_port, test_url, duration).await;

    let _ = child.start_kill();
    let _ = tokio::fs::remove_file(&cfg_path).await;
    result
}

async fn run_download(socks_port: u16, test_url: &str, duration: Duration) -> Result<f64> {
    // Wait for the SOCKS port to come up.
    wait_for_port(socks_port, Duration::from_secs(5)).await?;

    let proxy = reqwest::Proxy::all(format!("socks5h://127.0.0.1:{socks_port}"))?;
    let client = reqwest::Client::builder()
        .proxy(proxy)
        .timeout(duration + Duration::from_secs(5))
        .build()?;

    let start = Instant::now();
    let resp = client.get(test_url).send().await?.error_for_status()?;
    let mut stream = resp.bytes_stream();

    let mut total: u64 = 0;
    let deadline = start + duration;
    while let Ok(Some(chunk)) = tokio::time::timeout_at(deadline.into(), stream.next()).await {
        match chunk {
            Ok(bytes) => total += bytes.len() as u64,
            Err(_) => break,
        }
    }
    let elapsed = start.elapsed().as_secs_f64().max(0.001);
    let kbps = (total as f64 * 8.0 / 1000.0) / elapsed;
    Ok(kbps)
}

async fn wait_for_port(port: u16, timeout: Duration) -> Result<()> {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let deadline = Instant::now() + timeout;
    loop {
        match TcpStream::connect(addr).await {
            Ok(mut s) => {
                let _ = s.shutdown().await;
                return Ok(());
            }
            Err(_) if Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
            Err(e) => return Err(e).context("proxy port never came up"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_prefers_throughput() {
        let node = VlessNode::parse("vless://u@h:443?type=xhttp").unwrap();
        let fast = TestedNode {
            node: node.clone(),
            ip: "1.1.1.1".parse().unwrap(),
            provider: "cloudflare".into(),
            latency: Some(Duration::from_millis(500)),
            throughput_kbps: Some(50_000.0),
        };
        let low_latency = TestedNode {
            node,
            ip: "1.1.1.1".parse().unwrap(),
            provider: "cloudflare".into(),
            latency: Some(Duration::from_millis(10)),
            throughput_kbps: None,
        };
        assert!(fast.score() > low_latency.score());
    }

    #[test]
    fn score_orders_latency() {
        let node = VlessNode::parse("vless://u@h:443?type=xhttp").unwrap();
        let mk = |ms| TestedNode {
            node: node.clone(),
            ip: "1.1.1.1".parse().unwrap(),
            provider: "x".into(),
            latency: Some(Duration::from_millis(ms)),
            throughput_kbps: None,
        };
        assert!(mk(20).score() > mk(200).score());
    }

    #[tokio::test]
    async fn resolves_ip_literal() {
        let ip = resolve_first("1.2.3.4", 443).await;
        assert_eq!(ip, Some("1.2.3.4".parse().unwrap()));
    }
}

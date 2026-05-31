mod cidr;
mod output;
mod singbox;
mod sources;
mod speedtest;
mod vless;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use futures::stream::{self, StreamExt};

use crate::cidr::ProviderDb;
use crate::speedtest::TestedNode;
use crate::vless::VlessNode;

/// VLESS + XHTTP aggregator for Hiddify: collect, filter to known networks
/// (VK / Yandex / Cloudflare / Google / Beeline), speed-test and rank.
#[derive(Parser, Debug)]
#[command(name = "hiddify-parser", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[command(flatten)]
    run: RunArgs,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run the full pipeline (default).
    Run(RunArgs),
    /// Refresh data/cidr/*.txt from RIPEstat for all known providers.
    UpdateCidr(UpdateCidrArgs),
}

#[derive(Parser, Debug, Clone)]
struct RunArgs {
    /// Source list file (one URL per line). Uses the embedded list if omitted.
    #[arg(long)]
    sources: Option<PathBuf>,

    /// Directory of provider CIDR files. Uses embedded ranges if omitted.
    #[arg(long)]
    cidr_dir: Option<PathBuf>,

    /// Output directory for the subscription artifacts.
    #[arg(long, default_value = "sub")]
    out: PathBuf,

    /// Max concurrent network operations.
    #[arg(long, default_value_t = 128)]
    concurrency: usize,

    /// TCP connect timeout for the latency test (milliseconds).
    #[arg(long, default_value_t = 2500)]
    connect_timeout_ms: u64,

    /// Keep at most this many nodes in the final subscription (0 = unlimited).
    #[arg(long, default_value_t = 200)]
    max_nodes: usize,

    /// Disable the CIDR/provider filter (debugging only).
    #[arg(long, default_value_t = false)]
    no_cidr_filter: bool,

    /// Path to a sing-box binary to enable the real download-speed test.
    #[arg(long)]
    core_bin: Option<PathBuf>,

    /// URL used for the real download-speed test.
    #[arg(
        long,
        default_value = "https://speed.cloudflare.com/__down?bytes=10000000"
    )]
    test_url: String,

    /// Duration of each throughput test (seconds).
    #[arg(long, default_value_t = 8)]
    throughput_secs: u64,

    /// Run the throughput test only on the top N latency-ranked nodes.
    #[arg(long, default_value_t = 30)]
    throughput_top: usize,
}

#[derive(Parser, Debug)]
struct UpdateCidrArgs {
    /// Directory to write provider CIDR files into.
    #[arg(long, default_value = "data/cidr")]
    out: PathBuf,
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent("hiddify-vless-xhttp-parser/0.1 (+https://github.com/lanopivijo93782-ops)")
        .connect_timeout(Duration::from_secs(15))
        .build()
        .context("building HTTP client")
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();
    match cli.command {
        Some(Command::UpdateCidr(args)) => update_cidr(args).await,
        Some(Command::Run(args)) => run(args).await,
        None => run(cli.run).await,
    }
}

async fn run(args: RunArgs) -> Result<()> {
    let client = http_client()?;

    // 1. Load sources.
    let urls = match &args.sources {
        Some(path) => {
            let body = std::fs::read_to_string(path)
                .with_context(|| format!("reading sources file {}", path.display()))?;
            sources::parse_source_list(&body)
        }
        None => sources::default_sources(),
    };
    tracing::info!(count = urls.len(), "loaded sources");

    // 2. Fetch all sources concurrently.
    let fetched = sources::fetch_all(&client, &urls, args.concurrency.min(32)).await;
    let ok = fetched.iter().filter(|f| f.body.is_some()).count();
    tracing::info!(ok, total = urls.len(), "fetched sources");

    // 3. Extract + parse + keep only VLESS/XHTTP, then de-duplicate.
    let mut by_key: BTreeMap<String, VlessNode> = BTreeMap::new();
    let mut raw_links = 0usize;
    for f in &fetched {
        let Some(body) = &f.body else { continue };
        let extracted = sources::extract_links(body);
        tracing::debug!(url = %f.url, found = extracted.len(), "scanned source");
        for link in extracted {
            raw_links += 1;
            if let Ok(node) = VlessNode::parse(&link) {
                if node.is_xhttp() {
                    by_key.insert(node.dedup_key(), node);
                }
            }
        }
    }
    let nodes: Vec<VlessNode> = by_key.into_values().collect();
    tracing::info!(
        raw_links,
        xhttp_unique = nodes.len(),
        "extracted VLESS+XHTTP nodes"
    );

    // 4. Load CIDR DB.
    let db = match &args.cidr_dir {
        Some(dir) => ProviderDb::from_dir(dir)?,
        None => ProviderDb::embedded(),
    };
    tracing::info!(ranges = db.len(), providers = ?db.summary(), "loaded CIDR database");

    // 5. Resolve + CIDR filter + latency test (concurrent).
    let timeout = Duration::from_millis(args.connect_timeout_ms);
    let no_filter = args.no_cidr_filter;
    let db_ref = &db;
    let mut tested: Vec<TestedNode> = stream::iter(nodes)
        .map(|node| async move {
            let ip = speedtest::resolve_first(&node.host, node.port).await?;
            let provider = match db_ref.match_ip(ip) {
                Some(p) => p.to_string(),
                None if no_filter => "other".to_string(),
                None => return None,
            };
            let latency =
                speedtest::tcp_latency((ip, node.port).into(), timeout).await;
            tracing::debug!(host = %node.host, %ip, provider, reachable = latency.is_some(), "tested node");
            Some(TestedNode {
                node,
                ip,
                provider,
                latency,
                throughput_kbps: None,
            })
        })
        .buffer_unordered(args.concurrency)
        .filter_map(|x| async move { x })
        .collect()
        .await;

    // Keep only reachable nodes.
    tested.retain(|t| t.is_reachable());
    tested.sort_by(|a, b| b.score().partial_cmp(&a.score()).unwrap());
    tracing::info!(reachable = tested.len(), "passed CIDR + reachability");

    // 6. Optional real throughput test on the top latency-ranked nodes.
    if let Some(core_bin) = &args.core_bin {
        run_throughput_tests(core_bin, &args, &mut tested).await;
        tested.sort_by(|a, b| b.score().partial_cmp(&a.score()).unwrap());
    }

    // 7. Cap and render.
    if args.max_nodes > 0 && tested.len() > args.max_nodes {
        tested.truncate(args.max_nodes);
    }

    let mut by_provider: BTreeMap<String, usize> = BTreeMap::new();
    for t in &tested {
        *by_provider.entry(t.provider.clone()).or_insert(0) += 1;
    }
    let links = output::render_links(&tested);
    let report = output::Report {
        generated_at: now_iso8601(),
        total: links.len(),
        by_provider: by_provider.clone(),
    };
    output::write_outputs(&args.out, &links, &report)?;

    tracing::info!(
        final = links.len(),
        by_provider = ?by_provider,
        out = %args.out.display(),
        "wrote subscription"
    );
    println!(
        "Done: {} VLESS+XHTTP nodes written to {} ({:?})",
        links.len(),
        args.out.display(),
        by_provider
    );
    Ok(())
}

async fn run_throughput_tests(core_bin: &Path, args: &RunArgs, tested: &mut [TestedNode]) {
    let work_dir = std::env::temp_dir().join("hiddify-parser-cores");
    if let Err(e) = std::fs::create_dir_all(&work_dir) {
        tracing::warn!(error = %e, "cannot create core work dir; skipping throughput");
        return;
    }
    let top = args.throughput_top.min(tested.len());
    tracing::info!(top, "running real throughput tests");
    let dur = Duration::from_secs(args.throughput_secs);
    for (i, t) in tested.iter_mut().take(top).enumerate() {
        let socks_port = 21080 + (i as u16 % 4000);
        match speedtest::throughput_via_core(
            &t.node,
            core_bin,
            &args.test_url,
            socks_port,
            dur,
            &work_dir,
        )
        .await
        {
            Ok(kbps) => {
                tracing::debug!(host = %t.node.host, ip = %t.ip, kbps, "throughput ok");
                t.throughput_kbps = Some(kbps);
            }
            Err(e) => {
                tracing::debug!(host = %t.node.host, ip = %t.ip, error = %e, "throughput failed")
            }
        }
    }
}

async fn update_cidr(args: UpdateCidrArgs) -> Result<()> {
    let client = http_client()?;
    std::fs::create_dir_all(&args.out)
        .with_context(|| format!("creating {}", args.out.display()))?;
    for (provider, asns) in cidr::PROVIDER_ASNS {
        tracing::info!(provider, ?asns, "fetching prefixes");
        let prefixes = cidr::fetch_provider_prefixes(&client, asns).await?;
        let body = cidr::render_provider_file(provider, asns, &prefixes);
        let path = args.out.join(format!("{provider}.txt"));
        std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
        println!(
            "{provider}: {} prefixes -> {}",
            prefixes.len(),
            path.display()
        );
    }
    Ok(())
}

/// Minimal RFC3339-ish UTC timestamp without pulling in a date crate.
fn now_iso8601() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Whole-second UTC; good enough for a "generated_at" marker.
    let days = now / 86_400;
    let secs = now % 86_400;
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    let (y, mo, d) = civil_from_days(days as i64);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

/// Convert days since the Unix epoch into a (year, month, day) civil date.
/// Algorithm from Howard Hinnant's date library.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_has_expected_shape() {
        let ts = now_iso8601();
        assert_eq!(ts.len(), 20);
        assert!(ts.ends_with('Z'));
        assert_eq!(&ts[4..5], "-");
    }

    #[test]
    fn civil_epoch_is_1970_01_01() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }
}

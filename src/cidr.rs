//! Provider CIDR database used to keep only nodes hosted on "known" networks
//! (VK, Yandex, Cloudflare, Google, Beeline, ...).
//!
//! Default ranges are embedded at compile time so the binary works fully
//! offline. They can be overridden from a directory at runtime, and refreshed
//! from authoritative sources via [`fetch_provider_prefixes`].

use std::net::IpAddr;
use std::path::Path;

use anyhow::{Context, Result};
use ipnetwork::IpNetwork;

/// Embedded default ranges: (provider, file contents).
const EMBEDDED: &[(&str, &str)] = &[
    ("vk", include_str!("../data/cidr/vk.txt")),
    ("yandex", include_str!("../data/cidr/yandex.txt")),
    ("cloudflare", include_str!("../data/cidr/cloudflare.txt")),
    ("google", include_str!("../data/cidr/google.txt")),
    ("beeline", include_str!("../data/cidr/beeline.txt")),
];

/// ASNs per provider, used by `update-cidr` to refresh ranges from RIPEstat.
pub const PROVIDER_ASNS: &[(&str, &[u32])] = &[
    ("vk", &[47764, 47541, 47542, 28709]),
    ("yandex", &[13238, 200350]),
    ("cloudflare", &[13335]),
    ("google", &[15169]),
    ("beeline", &[8402, 3216, 16345]),
];

#[derive(Debug, Clone)]
struct ProviderEntry {
    name: String,
    networks: Vec<IpNetwork>,
}

/// A set of providers and their networks, queryable by IP.
#[derive(Debug, Clone, Default)]
pub struct ProviderDb {
    entries: Vec<ProviderEntry>,
}

impl ProviderDb {
    /// Build the database from the ranges embedded in the binary.
    pub fn embedded() -> Self {
        let mut entries = Vec::new();
        for (name, body) in EMBEDDED {
            let networks = parse_networks(body);
            entries.push(ProviderEntry {
                name: (*name).to_string(),
                networks,
            });
        }
        ProviderDb { entries }
    }

    /// Load `*.txt` files from `dir`, where each filename (without extension)
    /// is the provider name. Falls back to embedded data when `dir` is missing.
    pub fn from_dir(dir: &Path) -> Result<Self> {
        if !dir.is_dir() {
            return Ok(Self::embedded());
        }
        let mut entries = Vec::new();
        let mut files: Vec<_> = std::fs::read_dir(dir)
            .with_context(|| format!("reading cidr dir {}", dir.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().map(|e| e == "txt").unwrap_or(false))
            .collect();
        files.sort();
        for path in files {
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();
            let body = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            entries.push(ProviderEntry {
                name,
                networks: parse_networks(&body),
            });
        }
        if entries.is_empty() {
            return Ok(Self::embedded());
        }
        Ok(ProviderDb { entries })
    }

    /// Return the name of the first provider whose ranges contain `ip`.
    pub fn match_ip(&self, ip: IpAddr) -> Option<&str> {
        for entry in &self.entries {
            if entry.networks.iter().any(|n| n.contains(ip)) {
                return Some(&entry.name);
            }
        }
        None
    }

    /// Total number of networks across all providers.
    pub fn len(&self) -> usize {
        self.entries.iter().map(|e| e.networks.len()).sum()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Providers and how many ranges each has (for logging).
    pub fn summary(&self) -> Vec<(String, usize)> {
        self.entries
            .iter()
            .map(|e| (e.name.clone(), e.networks.len()))
            .collect()
    }
}

/// Parse CIDR lines, ignoring blanks and `#` comments.
fn parse_networks(body: &str) -> Vec<IpNetwork> {
    body.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.parse::<IpNetwork>().ok())
        .collect()
}

/// Fetch announced prefixes for the given ASNs from RIPEstat. Used by the
/// `update-cidr` command to regenerate the data files.
pub async fn fetch_provider_prefixes(
    client: &reqwest::Client,
    asns: &[u32],
) -> Result<Vec<String>> {
    let mut prefixes: Vec<String> = Vec::new();
    for asn in asns {
        let url =
            format!("https://stat.ripe.net/data/announced-prefixes/data.json?resource=AS{asn}");
        let resp: RipeStat = client
            .get(&url)
            .send()
            .await
            .with_context(|| format!("requesting prefixes for AS{asn}"))?
            .error_for_status()?
            .json()
            .await
            .with_context(|| format!("decoding prefixes for AS{asn}"))?;
        for p in resp.data.prefixes {
            if p.prefix.parse::<IpNetwork>().is_ok() {
                prefixes.push(p.prefix);
            }
        }
    }
    prefixes.sort();
    prefixes.dedup();
    Ok(prefixes)
}

#[derive(serde::Deserialize)]
struct RipeStat {
    data: RipeData,
}

#[derive(serde::Deserialize)]
struct RipeData {
    prefixes: Vec<RipePrefix>,
}

#[derive(serde::Deserialize)]
struct RipePrefix {
    prefix: String,
}

/// Convenience: serialize a provider->prefixes map back into the on-disk format.
pub fn render_provider_file(provider: &str, asns: &[u32], prefixes: &[String]) -> String {
    let asn_list = asns
        .iter()
        .map(|a| format!("AS{a}"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut out = format!(
        "# {provider} ranges. ASN: {asn_list}.\n# Refresh with: hiddify-parser update-cidr\n"
    );
    for p in prefixes {
        out.push_str(p);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_db_is_populated() {
        let db = ProviderDb::embedded();
        assert!(db.len() > 30, "expected many ranges, got {}", db.len());
        assert_eq!(db.summary().len(), 5);
    }

    #[test]
    fn matches_cloudflare_ip() {
        let db = ProviderDb::embedded();
        let ip: IpAddr = "104.16.0.1".parse().unwrap();
        assert_eq!(db.match_ip(ip), Some("cloudflare"));
    }

    #[test]
    fn matches_yandex_ip() {
        let db = ProviderDb::embedded();
        let ip: IpAddr = "77.88.55.88".parse().unwrap();
        assert_eq!(db.match_ip(ip), Some("yandex"));
    }

    #[test]
    fn unknown_ip_returns_none() {
        let db = ProviderDb::embedded();
        let ip: IpAddr = "1.2.3.4".parse().unwrap();
        assert_eq!(db.match_ip(ip), None);
    }
}

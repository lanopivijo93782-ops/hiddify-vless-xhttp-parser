//! База CIDR провайдеров: оставляем только узлы в известных российских сетях
//! (VK, Yandex, MTS, Beeline, MegaFon, Rostelecom, Tele2, ER-Telecom, TTK).
//! Только сети, чьи IP в РФ не блокируют.
//!
//! Диапазоны по умолчанию встроены в бинарь на этапе компиляции, поэтому он
//! работает полностью офлайн. Их можно переопределить каталогом во время
//! выполнения и обновить из RIPEstat через [`fetch_provider_prefixes`].

use std::net::IpAddr;
use std::path::Path;

use anyhow::{Context, Result};
use ipnetwork::IpNetwork;

/// Встроенные диапазоны по умолчанию: (провайдер, содержимое файла).
/// Только российские сети, чьи IP в РФ не блокируют.
const EMBEDDED: &[(&str, &str)] = &[
    ("vk", include_str!("../data/cidr/vk.txt")),
    ("yandex", include_str!("../data/cidr/yandex.txt")),
    ("mts", include_str!("../data/cidr/mts.txt")),
    ("beeline", include_str!("../data/cidr/beeline.txt")),
    ("megafon", include_str!("../data/cidr/megafon.txt")),
    ("rostelecom", include_str!("../data/cidr/rostelecom.txt")),
    ("tele2", include_str!("../data/cidr/tele2.txt")),
    ("ertelecom", include_str!("../data/cidr/ertelecom.txt")),
    ("ttk", include_str!("../data/cidr/ttk.txt")),
];

/// ASN каждого провайдера — используются `update-cidr` для загрузки префиксов
/// из RIPEstat. Все провайдеры — крупные российские операторы/сервисы.
pub const PROVIDER_ASNS: &[(&str, &[u32])] = &[
    ("vk", &[47764, 47541, 47542, 28709]),
    ("yandex", &[13238, 200350]),
    ("mts", &[8359, 25513]),
    ("beeline", &[3216, 8402, 16345]),
    ("megafon", &[31133, 25159, 31163]),
    ("rostelecom", &[12389, 42610, 8997]),
    ("tele2", &[48092, 41330]),
    ("ertelecom", &[9049, 50543, 39435]),
    ("ttk", &[20485]),
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
        "# Диапазоны {provider}. ASN: {asn_list}.\n# Обновление: hiddify-parser update-cidr\n"
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
        assert_eq!(db.summary().len(), PROVIDER_ASNS.len());
    }

    #[test]
    fn matches_yandex_ip() {
        let db = ProviderDb::embedded();
        let ip: IpAddr = "77.88.55.88".parse().unwrap();
        assert_eq!(db.match_ip(ip), Some("yandex"));
    }

    #[test]
    fn matches_vk_ip() {
        let db = ProviderDb::embedded();
        let ip: IpAddr = "87.240.190.78".parse().unwrap();
        assert_eq!(db.match_ip(ip), Some("vk"));
    }

    #[test]
    fn unknown_ip_returns_none() {
        let db = ProviderDb::embedded();
        let ip: IpAddr = "1.2.3.4".parse().unwrap();
        assert_eq!(db.match_ip(ip), None);
    }
}

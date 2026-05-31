//! Rendering the filtered/tested nodes into Hiddify-ready artifacts.

use std::path::Path;

use anyhow::{Context, Result};
use base64::Engine;
use serde::Serialize;

use crate::speedtest::TestedNode;

/// Короткое имя провайдера для отображения в имени ключа.
fn provider_label(provider: &str) -> &str {
    match provider {
        "vk" => "VK",
        "yandex" => "YA",
        "mts" => "MTS",
        "beeline" => "Beeline",
        "megafon" => "MegaFon",
        "rostelecom" => "Rostelecom",
        "tele2" => "Tele2",
        "ertelecom" => "ER-Telecom",
        "ttk" => "TTK",
        other => other,
    }
}

/// Флаг страны провайдера (эмодзи, без текста). Белый список — только РФ.
fn provider_flag(_provider: &str) -> &str {
    "\u{1F1F7}\u{1F1FA}" // 🇷🇺
}

/// Переписывает имя каждого ключа в формат `[флаг]:[провайдер]` и возвращает
/// список `vless://` ссылок в порядке ранжирования (лучшие первыми).
pub fn render_links(tested: &[TestedNode]) -> Vec<String> {
    let mut links = Vec::with_capacity(tested.len());
    for (i, t) in tested.iter().enumerate() {
        let mut node = t.node.clone();
        let label = provider_label(&t.provider);
        let flag = provider_flag(&t.provider);
        let speed = match t.throughput_kbps {
            Some(kbps) => format!("{:.1}Mbps", kbps / 1000.0),
            None => match t.latency {
                Some(d) => format!("{}ms", d.as_millis()),
                None => "n/a".to_string(),
            },
        };
        // Формат имени ключа: "🇷🇺:YA #01 · 35ms"
        node.set_remark(format!(
            "{}:{} #{:02} \u{00B7} {}",
            flag,
            label,
            i + 1,
            speed
        ));
        links.push(node.to_string());
    }
    links
}

/// A machine-readable summary of a run.
#[derive(Serialize)]
pub struct Report {
    pub generated_at: String,
    pub total: usize,
    pub by_provider: std::collections::BTreeMap<String, usize>,
}

/// Write the subscription artifacts into `out_dir`:
/// * `vless.txt`         – plain newline-separated links
/// * `subscription.txt`  – base64 of `vless.txt` (Hiddify import format)
/// * `report.json`       – run statistics
pub fn write_outputs(out_dir: &Path, links: &[String], report: &Report) -> Result<()> {
    std::fs::create_dir_all(out_dir).with_context(|| format!("creating {}", out_dir.display()))?;

    let plain = links.join("\n");
    let plain_path = out_dir.join("vless.txt");
    std::fs::write(&plain_path, format!("{plain}\n"))
        .with_context(|| format!("writing {}", plain_path.display()))?;

    let b64 = base64::engine::general_purpose::STANDARD.encode(plain.as_bytes());
    let sub_path = out_dir.join("subscription.txt");
    std::fs::write(&sub_path, b64).with_context(|| format!("writing {}", sub_path.display()))?;

    let report_path = out_dir.join("report.json");
    std::fs::write(&report_path, serde_json::to_string_pretty(report)?)
        .with_context(|| format!("writing {}", report_path.display()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vless::VlessNode;
    use std::net::IpAddr;
    use std::time::Duration;

    fn tested(provider: &str, latency_ms: u64) -> TestedNode {
        TestedNode {
            node: VlessNode::parse("vless://u@cdn.example.com:443?type=xhttp&security=reality")
                .unwrap(),
            ip: "104.16.0.1".parse::<IpAddr>().unwrap(),
            provider: provider.to_string(),
            latency: Some(Duration::from_millis(latency_ms)),
            throughput_kbps: None,
        }
    }

    #[test]
    fn render_sets_remark_and_keeps_link_valid() {
        let links = render_links(&[tested("yandex", 42)]);
        assert_eq!(links.len(), 1);
        let parsed = VlessNode::parse(&links[0]).unwrap();
        assert!(parsed.remark.contains("YA"));
        assert!(parsed.remark.contains('\u{1F1F7}')); // 🇷🇺
        assert!(parsed.remark.contains("#01"));
        assert!(parsed.is_xhttp());
    }

    #[test]
    fn write_outputs_creates_files() {
        let dir = std::env::temp_dir().join(format!("hpx-test-{}", std::process::id()));
        let links = render_links(&[tested("yandex", 10)]);
        let report = Report {
            generated_at: "now".into(),
            total: 1,
            by_provider: Default::default(),
        };
        write_outputs(&dir, &links, &report).unwrap();
        assert!(dir.join("vless.txt").exists());
        assert!(dir.join("subscription.txt").exists());
        assert!(dir.join("report.json").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

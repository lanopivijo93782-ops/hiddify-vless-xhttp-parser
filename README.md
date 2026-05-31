# hiddify-vless-xhttp-parser

Aggregates **VLESS + XHTTP** configs from popular public GitHub collectors, keeps
**only** nodes hosted on *known* networks (VK, Yandex, Cloudflare/CDN, Google,
Beeline), speed-tests the survivors and emits a ready-to-import **Hiddify**
subscription.

Built in Rust. Designed to run unattended via GitHub Actions.

## Why these constraints

| Requirement | How it is implemented |
|---|---|
| **VLESS + XHTTP only** | Every parsed share-link is checked: `protocol == vless` and `type ∈ {xhttp, splithttp}` (`splithttp` is XHTTP's former name). Everything else is dropped. |
| **Only known servers (VK / Yandex / CDN / Google / Beeline)** | Each node's host is resolved to an IP and matched against curated provider CIDR sets in [`data/cidr/`](data/cidr). Nodes outside those ranges are discarded. The ranges are refreshable from RIPEstat via `update-cidr`. |
| **Max internet speed (test with URL)** | Two-stage ranking: a TCP-handshake **latency** test for every node (no proxy core needed), plus an optional **real download throughput** test that routes a test URL through each node using a `sing-box` core. Nodes are ranked best-first. |
| **Exclusively for Hiddify** | Output is a standard base64 subscription (`sub/subscription.txt`) plus the raw `sub/vless.txt`, both of which Hiddify imports directly. |
| **GitHub Actions** | [`update.yml`](.github/workflows/update.yml) rebuilds the subscription on a schedule and commits it; [`ci.yml`](.github/workflows/ci.yml) runs fmt/clippy/tests. |

## Pipeline

```
sources.txt ──fetch──► extract vless:// (+base64 decode)
            ──filter─► VLESS && XHTTP, de-duplicate
            ──resolve► host → IP
            ──CIDR───► keep VK / Yandex / Cloudflare / Google / Beeline
            ──test───► TCP latency  (+ optional sing-box throughput)
            ──rank───► best first, cap to --max-nodes
            ──emit───► sub/vless.txt + sub/subscription.txt + sub/report.json
```

## Usage

```bash
# Full pipeline with the embedded sources & CIDR ranges:
cargo run --release -- run

# Useful flags:
cargo run --release -- run \
  --max-nodes 200 \           # cap final list
  --connect-timeout-ms 2500 \ # latency-test timeout
  --out sub                   # output directory

# Real download-speed test through each node (needs a sing-box binary):
cargo run --release -- run --core-bin /usr/local/bin/sing-box \
  --test-url https://speed.cloudflare.com/__down?bytes=10000000 \
  --throughput-secs 8 --throughput-top 30

# Refresh provider CIDR ranges from RIPEstat:
cargo run --release -- update-cidr --out data/cidr
```

### Importing into Hiddify

Add the raw URL of `sub/subscription.txt` (or `sub/vless.txt`) as a profile in
Hiddify:

```
https://raw.githubusercontent.com/lanopivijo93782-ops/hiddify-vless-xhttp-parser/main/sub/subscription.txt
```

Hiddify auto-refreshes the profile; the GitHub Action keeps the file up to date.

## Outputs

* `sub/vless.txt` — newline-separated `vless://` links, remarks rewritten to
  `NN | Provider | xhttp | <latency or Mbps>`, ranked best-first.
* `sub/subscription.txt` — base64 of `vless.txt` (the format Hiddify imports).
* `sub/report.json` — run stats (`total`, `by_provider`, `generated_at`).

## Configuration files

* [`data/sources.txt`](data/sources.txt) — subscription sources, one URL per
  line (`#` comments allowed). Dead links are skipped with a warning.
* [`data/cidr/*.txt`](data/cidr) — one file per provider; filename = provider
  name; CIDR per line.

Both are embedded into the binary as defaults and can be overridden with
`--sources` / `--cidr-dir`.

## Notes on provider coverage

`Cloudflare` dominates real-world public VLESS+XHTTP configs because most are
fronted behind Cloudflare's CDN — exactly the "known CDN" case this tool targets.
VK / Yandex / Beeline / Google ranges are included so any nodes hosted there are
kept too. Refresh the ranges with `update-cidr` to track ASN changes.

## Development

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test --all
```

## License

MIT — see [LICENSE](LICENSE).

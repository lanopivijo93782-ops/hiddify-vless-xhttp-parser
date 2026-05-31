//! Minimal sing-box config generation for the optional real-throughput test.
//!
//! We build a config with a local SOCKS inbound and a single VLESS outbound
//! mapped from a [`VlessNode`], so traffic can be routed through the node and
//! a download speed measured against a test URL.

use serde_json::{json, Map, Value};

use crate::vless::VlessNode;

/// Build a sing-box JSON config that exposes a SOCKS proxy on
/// `127.0.0.1:<socks_port>` routed through `node`.
pub fn build_test_config(node: &VlessNode, socks_port: u16) -> Value {
    json!({
        "log": { "level": "error" },
        "inbounds": [{
            "type": "socks",
            "tag": "socks-in",
            "listen": "127.0.0.1",
            "listen_port": socks_port
        }],
        "outbounds": [ outbound_from_node(node) ]
    })
}

/// Map a VLESS node into a sing-box `vless` outbound object.
pub fn outbound_from_node(node: &VlessNode) -> Value {
    let mut out = Map::new();
    out.insert("type".into(), json!("vless"));
    out.insert("tag".into(), json!("proxy"));
    out.insert("server".into(), json!(node.host));
    out.insert("server_port".into(), json!(node.port));
    out.insert("uuid".into(), json!(node.uuid));

    if let Some(flow) = node.params.get("flow") {
        if !flow.is_empty() {
            out.insert("flow".into(), json!(flow));
        }
    }

    if let Some(tls) = tls_block(node) {
        out.insert("tls".into(), tls);
    }
    if let Some(transport) = transport_block(node) {
        out.insert("transport".into(), transport);
    }

    Value::Object(out)
}

fn tls_block(node: &VlessNode) -> Option<Value> {
    let security = node.security();
    if security != "tls" && security != "reality" {
        return None;
    }
    let mut tls = Map::new();
    tls.insert("enabled".into(), json!(true));

    let sni = node
        .params
        .get("sni")
        .or_else(|| node.params.get("host"))
        .cloned()
        .unwrap_or_else(|| node.host.clone());
    tls.insert("server_name".into(), json!(sni));

    if let Some(alpn) = node.params.get("alpn") {
        let parts: Vec<&str> = alpn.split(',').filter(|s| !s.is_empty()).collect();
        if !parts.is_empty() {
            tls.insert("alpn".into(), json!(parts));
        }
    }

    // uTLS fingerprint.
    if let Some(fp) = node.params.get("fp") {
        if !fp.is_empty() {
            tls.insert("utls".into(), json!({ "enabled": true, "fingerprint": fp }));
        }
    }

    if security == "reality" {
        let mut reality = Map::new();
        reality.insert("enabled".into(), json!(true));
        if let Some(pbk) = node.params.get("pbk") {
            reality.insert("public_key".into(), json!(pbk));
        }
        if let Some(sid) = node.params.get("sid") {
            reality.insert("short_id".into(), json!(sid));
        }
        tls.insert("reality".into(), Value::Object(reality));
    }

    Some(Value::Object(tls))
}

fn transport_block(node: &VlessNode) -> Option<Value> {
    match node.network() {
        "xhttp" | "splithttp" => {
            let mut t = Map::new();
            t.insert("type".into(), json!("xhttp"));
            if let Some(path) = node.params.get("path") {
                t.insert("path".into(), json!(path));
            }
            if let Some(host) = node.params.get("host") {
                if !host.is_empty() {
                    t.insert("host".into(), json!(host));
                }
            }
            if let Some(mode) = node.params.get("mode") {
                if !mode.is_empty() {
                    t.insert("mode".into(), json!(mode));
                }
            }
            Some(Value::Object(t))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_xhttp_reality_outbound() {
        let node = VlessNode::parse(
            "vless://uuid@cdn.example.com:443?type=xhttp&security=reality&sni=vk.com&pbk=KEY&sid=ab&fp=chrome&path=%2Fv#n",
        )
        .unwrap();
        let ob = outbound_from_node(&node);
        assert_eq!(ob["type"], "vless");
        assert_eq!(ob["server"], "cdn.example.com");
        assert_eq!(ob["tls"]["reality"]["public_key"], "KEY");
        assert_eq!(ob["tls"]["server_name"], "vk.com");
        assert_eq!(ob["transport"]["type"], "xhttp");
        assert_eq!(ob["transport"]["path"], "/v");
    }

    #[test]
    fn config_has_socks_inbound() {
        let node = VlessNode::parse("vless://uuid@h:443?type=xhttp&security=tls").unwrap();
        let cfg = build_test_config(&node, 10800);
        assert_eq!(cfg["inbounds"][0]["listen_port"], 10800);
        assert_eq!(cfg["inbounds"][0]["type"], "socks");
    }
}

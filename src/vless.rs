//! Parsing and (re)serialization of `vless://` share links.
//!
//! A VLESS link looks like:
//! `vless://<uuid>@<host>:<port>?type=xhttp&security=reality&sni=...#<remark>`
//!
//! We keep every query parameter so a parsed node can be turned back into an
//! equivalent link without losing information, while also exposing the few
//! fields the rest of the pipeline cares about (transport, host, port).

use std::collections::BTreeMap;
use std::fmt;

use percent_encoding::{percent_decode_str, utf8_percent_encode, NON_ALPHANUMERIC};

/// A single parsed VLESS node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VlessNode {
    pub uuid: String,
    pub host: String,
    pub port: u16,
    /// All query parameters in their original (decoded) form.
    pub params: BTreeMap<String, String>,
    /// The remark / fragment (text after `#`), decoded.
    pub remark: String,
}

impl VlessNode {
    /// Transport type (`type` query param). Defaults to `tcp` when absent,
    /// matching Xray/sing-box behaviour.
    pub fn network(&self) -> &str {
        self.params.get("type").map(String::as_str).unwrap_or("tcp")
    }

    /// Security layer (`security` query param), e.g. `tls`, `reality`, `none`.
    pub fn security(&self) -> &str {
        self.params
            .get("security")
            .map(String::as_str)
            .unwrap_or("none")
    }

    /// Returns true when the node uses the XHTTP transport. `splithttp` is the
    /// former name of the same transport and is treated as an alias.
    pub fn is_xhttp(&self) -> bool {
        matches!(self.network(), "xhttp" | "splithttp")
    }

    /// A stable identity used for de-duplication.
    pub fn dedup_key(&self) -> String {
        format!(
            "{}@{}:{}|{}|{}",
            self.uuid,
            self.host.to_ascii_lowercase(),
            self.port,
            self.network(),
            self.security()
        )
    }

    /// Replace the human-readable remark.
    pub fn set_remark(&mut self, remark: impl Into<String>) {
        self.remark = remark.into();
    }

    /// Parse a single `vless://` link.
    pub fn parse(link: &str) -> Result<Self, ParseError> {
        let link = link.trim();
        let rest = link.strip_prefix("vless://").ok_or(ParseError::NotVless)?;

        // Split off the fragment (#remark) first.
        let (main, remark) = match rest.split_once('#') {
            Some((m, r)) => (m, decode(r)),
            None => (rest, String::new()),
        };

        // Split userinfo (uuid) from the authority + query.
        let (uuid, after_at) = main.split_once('@').ok_or(ParseError::MissingUuid)?;
        if uuid.is_empty() {
            return Err(ParseError::MissingUuid);
        }

        // Split authority from query string.
        let (authority, query) = match after_at.split_once('?') {
            Some((a, q)) => (a, q),
            None => (after_at, ""),
        };

        let (host, port) = split_host_port(authority)?;

        let mut params = BTreeMap::new();
        for pair in query.split('&').filter(|s| !s.is_empty()) {
            let (k, v) = match pair.split_once('=') {
                Some((k, v)) => (decode(k), decode(v)),
                None => (decode(pair), String::new()),
            };
            params.insert(k, v);
        }

        Ok(VlessNode {
            uuid: decode(uuid),
            host,
            port,
            params,
            remark,
        })
    }
}

impl fmt::Display for VlessNode {
    /// Serialize back into a `vless://` link.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "vless://{}@{}:{}", self.uuid, self.host, self.port)?;
        if !self.params.is_empty() {
            let query: Vec<String> = self
                .params
                .iter()
                .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
                .collect();
            write!(f, "?{}", query.join("&"))?;
        }
        if !self.remark.is_empty() {
            write!(f, "#{}", encode(&self.remark))?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    NotVless,
    MissingUuid,
    MissingPort,
    BadPort,
    EmptyHost,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let msg = match self {
            ParseError::NotVless => "not a vless:// link",
            ParseError::MissingUuid => "missing uuid before '@'",
            ParseError::MissingPort => "missing ':port'",
            ParseError::BadPort => "port is not a valid number",
            ParseError::EmptyHost => "empty host",
        };
        f.write_str(msg)
    }
}

impl std::error::Error for ParseError {}

/// Split `host:port`, supporting bracketed IPv6 literals like `[::1]:443`.
fn split_host_port(authority: &str) -> Result<(String, u16), ParseError> {
    if let Some(end) = authority.strip_prefix('[') {
        // IPv6 literal: [addr]:port
        let (addr, after) = end.split_once(']').ok_or(ParseError::EmptyHost)?;
        let port = after
            .strip_prefix(':')
            .ok_or(ParseError::MissingPort)?
            .parse::<u16>()
            .map_err(|_| ParseError::BadPort)?;
        if addr.is_empty() {
            return Err(ParseError::EmptyHost);
        }
        return Ok((addr.to_string(), port));
    }

    let (host, port) = authority.rsplit_once(':').ok_or(ParseError::MissingPort)?;
    if host.is_empty() {
        return Err(ParseError::EmptyHost);
    }
    let port = port.parse::<u16>().map_err(|_| ParseError::BadPort)?;
    Ok((host.to_string(), port))
}

fn decode(s: &str) -> String {
    percent_decode_str(s).decode_utf8_lossy().into_owned()
}

fn encode(s: &str) -> String {
    utf8_percent_encode(s, NON_ALPHANUMERIC).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xhttp_reality_link() {
        let link = "vless://11111111-2222-3333-4444-555555555555@cdn.example.com:443?type=xhttp&security=reality&sni=vk.com&pbk=abc&fp=chrome&path=%2Fvideo#My%20Node";
        let n = VlessNode::parse(link).unwrap();
        assert_eq!(n.uuid, "11111111-2222-3333-4444-555555555555");
        assert_eq!(n.host, "cdn.example.com");
        assert_eq!(n.port, 443);
        assert_eq!(n.network(), "xhttp");
        assert_eq!(n.security(), "reality");
        assert_eq!(n.params.get("sni").unwrap(), "vk.com");
        assert_eq!(n.params.get("path").unwrap(), "/video");
        assert_eq!(n.remark, "My Node");
        assert!(n.is_xhttp());
    }

    #[test]
    fn splithttp_is_treated_as_xhttp() {
        let link = "vless://uuid@host:8443?type=splithttp&security=tls";
        let n = VlessNode::parse(link).unwrap();
        assert!(n.is_xhttp());
    }

    #[test]
    fn tcp_default_network_is_not_xhttp() {
        let link = "vless://uuid@host:443?security=tls";
        let n = VlessNode::parse(link).unwrap();
        assert_eq!(n.network(), "tcp");
        assert!(!n.is_xhttp());
    }

    #[test]
    fn ipv6_authority() {
        let link = "vless://uuid@[2606:4700:4700::1111]:443?type=xhttp";
        let n = VlessNode::parse(link).unwrap();
        assert_eq!(n.host, "2606:4700:4700::1111");
        assert_eq!(n.port, 443);
    }

    #[test]
    fn round_trips() {
        let link = "vless://uuid@host:443?security=reality&type=xhttp#name";
        let n = VlessNode::parse(link).unwrap();
        let s = n.to_string();
        let n2 = VlessNode::parse(&s).unwrap();
        assert_eq!(n, n2);
    }

    #[test]
    fn rejects_non_vless() {
        assert!(matches!(
            VlessNode::parse("vmess://whatever"),
            Err(ParseError::NotVless)
        ));
    }
}

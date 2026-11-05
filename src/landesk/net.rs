//! 来源网络白名单：默认只允许私有地址段连入。
//!
//! 产品定位是"私有网络里的直连"，所以默认策略是**拒绝公网来源**，而不是"开门 + 靠密钥"。
//! 想放开就显式配置 `--landesk-allow-network`（或 `landesk.json` 的 `allowed_networks`），
//! 那时配的这份列表会**替换**默认值。

use std::net::IpAddr;

use hbb_common::{anyhow::anyhow, log, ResultType};
use ipnet::IpNet;

/// 未配置时的默认放行集合：RFC1918 + 运营商级 NAT + 链路本地 + IPv6 私有与环回。
const DEFAULT_NETWORKS: &[&str] = &[
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "100.64.0.0/10",
    "127.0.0.0/8",
    "169.254.0.0/16",
    "fc00::/7",
    "fe80::/10",
    "::1/128",
];

/// 把配置里的 CIDR 列表解析成可匹配的形式；未配置时返回默认私有网段。
pub fn parse(configured: &[String]) -> ResultType<Vec<IpNet>> {
    let list: Vec<&str> = configured
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .collect();
    let list = if list.is_empty() {
        DEFAULT_NETWORKS.to_vec()
    } else {
        list
    };
    let mut nets = Vec::with_capacity(list.len());
    for value in list {
        let net: IpNet = value
            .parse()
            .map_err(|_| anyhow!("不是合法的 CIDR 网段：{value}"))?;
        nets.push(net);
    }
    Ok(nets)
}

/// 这个来源地址是否被允许。IPv4-mapped IPv6（`::ffff:192.168.1.2`）按 IPv4 判。
pub fn allowed(ip: IpAddr, nets: &[IpNet]) -> bool {
    let ip = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(v6)),
        IpAddr::V4(_) => ip,
    };
    nets.iter().any(|net| net.contains(&ip))
}

/// 配置里的原始字符串（`landesk.json` / 命令行给的那份）。
pub fn configured() -> Vec<String> {
    crate::landesk::config()
        .map(|config| config.allowed_networks.clone())
        .unwrap_or_default()
}

/// 解析后的白名单；配置有问题时记一条 warn 并退回默认值，不因此拒绝所有连接。
pub fn load() -> Vec<IpNet> {
    let configured = configured();
    match parse(&configured) {
        Ok(nets) => {
            if configured.is_empty() {
                log::info!(
                    "landesk: only private networks may connect ({} default ranges)",
                    nets.len()
                );
            } else {
                log::info!("landesk: allowed source networks: {}", configured.join(","));
            }
            nets
        }
        Err(err) => {
            log::warn!("landesk: {err}; falling back to private ranges only");
            parse(&[]).unwrap_or_default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed_with(ip: &str, configured: &[&str]) -> bool {
        let configured: Vec<String> = configured.iter().map(|v| v.to_string()).collect();
        let nets = parse(&configured).expect("parse");
        allowed(ip.parse().expect("ip"), &nets)
    }

    #[test]
    fn defaults_cover_private_networks() {
        assert!(allowed_with("192.168.1.20", &[]));
        assert!(allowed_with("10.1.2.3", &[]));
        assert!(allowed_with("172.16.0.9", &[]));
        assert!(allowed_with("100.64.10.2", &[]));
        assert!(allowed_with("127.0.0.1", &[]));
        assert!(allowed_with("fd00::20", &[]));
        assert!(allowed_with("::1", &[]));
    }

    #[test]
    fn defaults_reject_public_and_v4_mapped_public() {
        assert!(!allowed_with("8.8.8.8", &[]));
        assert!(!allowed_with("::ffff:8.8.8.8", &[]));
        assert!(!allowed_with("2001:4860:4860::8888", &[]));
    }

    #[test]
    fn v4_mapped_private_is_allowed() {
        assert!(allowed_with("::ffff:192.168.1.20", &[]));
        assert!(allowed_with("::ffff:127.0.0.1", &[]));
    }

    #[test]
    fn configured_list_replaces_the_default() {
        assert!(allowed_with("203.0.113.7", &["203.0.113.0/24"]));
        assert!(!allowed_with("192.168.1.20", &["203.0.113.0/24"]));
    }

    #[test]
    fn invalid_entry_is_an_error() {
        assert!(parse(&["192.168.1.1".to_owned()]).is_err());
        assert!(parse(&["not-a-network".to_owned()]).is_err());
    }
}

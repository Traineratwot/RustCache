//! `/api/netinfo` — local LAN addresses for PAC/discovery hints.

use axum::Json;

/// Local (LAN) IPv4 addresses of this machine, excluding loopback.
pub async fn netinfo() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "local_ips": lan_ipv4s() }))
}

/// Sorted LAN IPv4 list: real LAN (192.168/10.x) before docker/bridge (172.16-31.x).
pub fn lan_ipv4s() -> Vec<String> {
    let mut ips: Vec<String> = Vec::new();
    if let Ok(ifas) = local_ip_address::list_afinet_netifas() {
        for (_name, ip) in ifas {
            if let std::net::IpAddr::V4(v4) = ip {
                if !v4.is_loopback() && !v4.is_link_local() {
                    let s = v4.to_string();
                    if !ips.contains(&s) {
                        ips.push(s);
                    }
                }
            }
        }
    }
    ips.sort_by_key(|ip| {
        if ip.starts_with("192.168.") || ip.starts_with("10.") {
            0u8
        } else if ip.starts_with("172.") {
            1u8
        } else {
            2u8
        }
    });
    ips
}

/// Best LAN IPv4 for PAC `proxy_host` fallback (first of the sorted list).
pub fn best_lan_ipv4() -> Option<String> {
    lan_ipv4s().into_iter().next()
}

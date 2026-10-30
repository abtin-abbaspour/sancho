//! `net_iface`: physical network interfaces from `/sys/class/net`.

use sancho_core::System;

use crate::{safe_token, Value};

pub fn enumerate(sys: &dyn System) -> Vec<Value> {
    sys.read_dir("/sys/class/net")
        .into_iter()
        .filter(|name| valid_iface(name))
        // Only physical devices have a `device` link; skips lo, bridges, veths.
        .filter(|name| sys.exists(&format!("/sys/class/net/{name}/device")))
        .map(|name| {
            let base = format!("/sys/class/net/{name}");
            let wireless =
                sys.exists(&format!("{base}/wireless")) || sys.exists(&format!("{base}/phy80211"));
            // ARPHRD_ETHER = 1
            let ether = sys
                .read(&format!("{base}/type"))
                .is_some_and(|t| t.trim() == "1");
            let kind = if wireless { "wireless" } else { "wired" };
            Value::new(&name, format!("{name} ({kind})"))
                .tag("wireless", wireless)
                .tag("wired", ether && !wireless)
        })
        .collect()
}

/// Kernel interface names: at most 15 bytes, no '/', whitespace or ':'.
fn valid_iface(name: &str) -> bool {
    safe_token(name, "_.-", 15)
}

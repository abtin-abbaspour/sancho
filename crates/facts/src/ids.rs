//! Lookups in hwdata's `pci.ids` / `usb.ids`: a vendor line ("14c3  MEDIATEK
//! Corp.") followed by its devices, each indented by one tab ("0616  MT7921
//! ..."); subsystem lines are indented by two tabs.

/// "Vendor Model" for hex IDs (any case), falling back to the raw IDs.
pub fn describe(text: &str, vendor: &str, device: &str) -> String {
    let (vendor, device) = (vendor.to_ascii_lowercase(), device.to_ascii_lowercase());
    let mut vendor_name = None;
    let mut device_name = None;
    let mut in_vendor = false;
    for line in text.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('\t') {
            if in_vendor && !rest.starts_with('\t') {
                if let Some(name) = rest
                    .strip_prefix(device.as_str())
                    .and_then(|r| r.strip_prefix("  "))
                {
                    device_name = Some(name.trim().to_string());
                    break;
                }
            }
            continue;
        }
        if in_vendor {
            break; // next vendor: the device isn't listed
        }
        if let Some(name) = line
            .strip_prefix(vendor.as_str())
            .and_then(|r| r.strip_prefix("  "))
        {
            vendor_name = Some(name.trim().to_string());
            in_vendor = true;
        }
    }
    match (vendor_name, device_name) {
        (Some(v), Some(d)) => format!("{v} {d}"),
        (Some(v), None) => format!("{v} device {device}"),
        _ => format!("device {vendor}:{device}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDS: &str = "# comment\n8086  Intel Corporation\n\t2725  Wi-Fi 6E(802.11ax) AX210/AX1675* 2x2 [Typhoon Peak]\n\t\t8086 0024  Wi-Fi 6E AX210 160MHz\n14c3  MEDIATEK Corp.\n\t0616  MT7921 802.11ax PCI Express Wireless Network Adapter\n";

    #[test]
    fn known_unknown_and_partial() {
        assert_eq!(
            describe(IDS, "14C3", "0616"),
            "MEDIATEK Corp. MT7921 802.11ax PCI Express Wireless Network Adapter"
        );
        assert!(describe(IDS, "8086", "2725").starts_with("Intel Corporation Wi-Fi 6E"));
        assert_eq!(
            describe(IDS, "8086", "9999"),
            "Intel Corporation device 9999"
        );
        assert_eq!(describe(IDS, "abcd", "0001"), "device abcd:0001");
        // Subsystem lines (two tabs) are never mistaken for devices.
        assert_eq!(
            describe(IDS, "8086", "0024"),
            "Intel Corporation device 0024"
        );
    }
}

//! Optional native USB / FTDI probe enumeration (`usb-native` feature → rusb).
//!
//! Enumerates FTDI Vendor ID `0x0403` devices for [`crate::detect_boards`].
//! This is **detect-only**: listing a probe must never be treated as program DONE.
//! Actual programming remains openFPGALoader (or sim TAP). When the feature is
//! off or rusb/libusb is unavailable at build time, callers keep the OFL path.

/// FTDI USB vendor ID (Future Technology Devices International).
pub const FTDI_VID: u16 = 0x0403;

/// Common FT2232H dual-channel PID used on many JTAG bring-up cables.
pub const FTDI_PID_FT2232H: u16 = 0x6010;

/// One FTDI device seen via rusb (or empty when feature disabled).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FtdiDeviceInfo {
    pub bus: u8,
    pub address: u8,
    pub vid: u16,
    pub pid: u16,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial: Option<String>,
}

impl FtdiDeviceInfo {
    /// Short stable name for detect listings (`ftdi-busN-addrM`).
    pub fn name(&self) -> String {
        format!("ftdi-{}-{}", self.bus, self.address)
    }

    /// Human detail line (never implies program success).
    pub fn detail(&self) -> String {
        let mut parts = vec![
            format!("native-rusb FTDI vid={:#06x} pid={:#06x}", self.vid, self.pid),
            format!("bus={} addr={}", self.bus, self.address),
        ];
        if let Some(ref m) = self.manufacturer {
            parts.push(format!("mfr={m}"));
        }
        if let Some(ref p) = self.product {
            parts.push(format!("product={p}"));
        }
        if let Some(ref s) = self.serial {
            parts.push(format!("serial={s}"));
        }
        parts.push("detect-only (no program DONE claimed)".into());
        parts.join(" ")
    }
}

/// Result of optional rusb FTDI enumeration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeUsbScan {
    pub probes: Vec<FtdiDeviceInfo>,
    pub note: String,
    /// Whether this build was compiled with `usb-native`.
    pub feature_enabled: bool,
}

/// `true` when compiled with `--features usb-native`.
pub fn feature_enabled() -> bool {
    cfg!(feature = "usb-native")
}

/// Enumerate USB devices with FTDI VID `0x0403`.
///
/// When `usb-native` is disabled, returns an empty probe list and a note that
/// the OFL path remains active. Never fabricates devices. Never claims DONE.
pub fn enumerate_ftdi() -> NativeUsbScan {
    #[cfg(feature = "usb-native")]
    {
        enumerate_ftdi_rusb()
    }
    #[cfg(not(feature = "usb-native"))]
    {
        NativeUsbScan {
            probes: Vec::new(),
            note: "usb-native feature disabled — FTDI rusb enumeration off; use openFPGALoader --scan-usb / --cable ofl|usb"
                .into(),
            feature_enabled: false,
        }
    }
}

#[cfg(feature = "usb-native")]
fn enumerate_ftdi_rusb() -> NativeUsbScan {
    let devices = match rusb::devices() {
        Ok(d) => d,
        Err(e) => {
            return NativeUsbScan {
                probes: Vec::new(),
                note: format!("rusb devices() failed: {e} (OFL path remains)"),
                feature_enabled: true,
            };
        }
    };
    let mut probes = Vec::new();
    for dev in devices.iter() {
        let desc = match dev.device_descriptor() {
            Ok(d) => d,
            Err(_) => continue,
        };
        if desc.vendor_id() != FTDI_VID {
            continue;
        }
        let (manufacturer, product, serial) = read_string_descriptors(&dev, &desc);
        probes.push(FtdiDeviceInfo {
            bus: dev.bus_number(),
            address: dev.address(),
            vid: desc.vendor_id(),
            pid: desc.product_id(),
            manufacturer,
            product,
            serial,
        });
    }
    let note = if probes.is_empty() {
        "rusb: no FTDI (VID 0x0403) devices enumerated — detect-only; OFL path remains".into()
    } else {
        format!(
            "rusb: {} FTDI (VID 0x0403) device(s) — detect-only (no program DONE claimed)",
            probes.len()
        )
    };
    NativeUsbScan {
        probes,
        note,
        feature_enabled: true,
    }
}

#[cfg(feature = "usb-native")]
fn read_string_descriptors(
    dev: &rusb::Device<rusb::GlobalContext>,
    desc: &rusb::DeviceDescriptor,
) -> (Option<String>, Option<String>, Option<String>) {
    // Opening may fail without udev permissions; still list VID/PID from descriptor.
    let handle = match dev.open() {
        Ok(h) => h,
        Err(_) => return (None, None, None),
    };
    let timeout = std::time::Duration::from_millis(200);
    let lang = match handle.read_languages(timeout) {
        Ok(langs) if !langs.is_empty() => langs[0],
        _ => return (None, None, None),
    };
    let manufacturer = desc
        .manufacturer_string_index()
        .and_then(|i| handle.read_string_descriptor(lang, i, timeout).ok());
    let product = desc
        .product_string_index()
        .and_then(|i| handle.read_string_descriptor(lang, i, timeout).ok());
    let serial = desc
        .serial_number_string_index()
        .and_then(|i| handle.read_string_descriptor(lang, i, timeout).ok());
    (manufacturer, product, serial)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumerate_ftdi_never_panics_and_respects_feature() {
        let scan = enumerate_ftdi();
        assert_eq!(scan.feature_enabled, feature_enabled());
        // No fabricated probes: empty is fine on CI without hardware.
        for p in &scan.probes {
            assert_eq!(p.vid, FTDI_VID);
            assert!(p.detail().contains("detect-only"));
            assert!(!p.detail().to_ascii_lowercase().contains("done=1"));
        }
        assert!(!scan.note.is_empty());
    }
}

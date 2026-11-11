//! Real FTDI MPSSE channel over USB ([`MpssePort`] via rusb, feature
//! `usb-native`).
//!
//! Control requests are the FTDI vendor requests libftdi uses
//! (`SIO_RESET` 0x00, `SIO_SET_LATENCY_TIMER` 0x09, `SIO_SET_BITMODE` 0x0B,
//! `wIndex` = channel 1..=4). Channel A uses bulk OUT 0x02 / IN 0x81,
//! channel B 0x04 / 0x83. Every bulk-IN packet starts with two modem-status
//! bytes that are not data; [`strip_modem_status`] removes them.
//!
//! Without `usb-native` [`open_usb_ftdi`] returns an error that says so; the
//! engine above it is identical for the real cable and [`crate::MockFtdi`].
//! **Not yet verified on a physical board** — the byte stream is verified
//! against the mock device and the AN_108 encodings only.

#[cfg(feature = "usb-native")]
use crate::jtag_engine::MpssePort;

/// FTDI vendor request codes.
pub const SIO_RESET: u8 = 0x00;
pub const SIO_SET_LATENCY_TIMER: u8 = 0x09;
pub const SIO_SET_BITMODE: u8 = 0x0B;
/// `SIO_RESET` wValue: reset / purge RX / purge TX.
pub const SIO_RESET_SIO: u16 = 0;
pub const SIO_RESET_PURGE_RX: u16 = 1;
pub const SIO_RESET_PURGE_TX: u16 = 2;

/// FTDI PIDs with an MPSSE engine (FT2232H, FT4232H, FT232H, FT2232C/D).
pub const MPSSE_PIDS: [(u16, &str); 4] = [
    (0x6010, "FT2232H/D"),
    (0x6011, "FT4232H"),
    (0x6014, "FT232H"),
    (0x6048, "FT4233H"),
];

/// Remove the two modem-status bytes that lead every `max_packet`-sized
/// bulk-IN packet.
pub fn strip_modem_status(raw: &[u8], max_packet: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    for pkt in raw.chunks(max_packet.max(3)) {
        if pkt.len() > 2 {
            out.extend_from_slice(&pkt[2..]);
        }
    }
    out
}

/// Endpoints for MPSSE channel `ch` (`'A'` or `'B'`).
pub fn channel_endpoints(ch: char) -> Option<(u8, u8, u16)> {
    match ch.to_ascii_uppercase() {
        'A' => Some((0x02, 0x81, 1)),
        'B' => Some((0x04, 0x83, 2)),
        _ => None,
    }
}

/// Real FTDI MPSSE channel.
pub struct UsbFtdiPort {
    #[cfg(feature = "usb-native")]
    handle: rusb::DeviceHandle<rusb::GlobalContext>,
    #[cfg(feature = "usb-native")]
    ep_out: u8,
    #[cfg(feature = "usb-native")]
    ep_in: u8,
    #[cfg(feature = "usb-native")]
    index: u16,
    #[cfg(feature = "usb-native")]
    max_packet: usize,
    pub detail: String,
}

/// Open the first MPSSE-capable FTDI (or the one at `bus`/`addr`), channel
/// `ch`. Errors honestly when no cable is attached or the build lacks USB.
pub fn open_usb_ftdi(at: Option<(u8, u8)>, ch: char) -> Result<UsbFtdiPort, String> {
    let (ep_out, ep_in, index) =
        channel_endpoints(ch).ok_or_else(|| format!("FTDI channel {ch} has no MPSSE (A or B)"))?;
    #[cfg(not(feature = "usb-native"))]
    {
        let _ = (at, ep_out, ep_in, index);
        Err("native FTDI: this build has no USB access (helion-hw feature usb-native off)".into())
    }
    #[cfg(feature = "usb-native")]
    {
        let scan = crate::native_usb::enumerate_ftdi();
        let pick = scan
            .probes
            .iter()
            .filter(|p| at.is_none_or(|(b, a)| p.bus == b && p.address == a))
            .find(|p| MPSSE_PIDS.iter().any(|(pid, _)| *pid == p.pid))
            .ok_or_else(|| {
                format!(
                    "{}: no MPSSE-capable FTDI cable found ({} FTDI device(s) enumerated)",
                    crate::ERR_CODE_USB_0,
                    scan.probes.len()
                )
            })?
            .clone();
        let devices = rusb::devices().map_err(|e| format!("rusb devices(): {e}"))?;
        let dev = devices
            .iter()
            .find(|d| d.bus_number() == pick.bus && d.address() == pick.address)
            .ok_or("FTDI cable disappeared during open")?;
        let max_packet = dev
            .active_config_descriptor()
            .ok()
            .and_then(|c| {
                c.interfaces()
                    .flat_map(|i| i.descriptors().collect::<Vec<_>>())
                    .flat_map(|d| d.endpoint_descriptors().collect::<Vec<_>>())
                    .find(|e| e.address() == ep_in)
                    .map(|e| e.max_packet_size() as usize)
            })
            .unwrap_or(512);
        let handle = dev
            .open()
            .map_err(|e| format!("open {}: {e} (permissions / driver?)", pick.detail()))?;
        let iface = (index - 1) as u8;
        let _ = handle.set_auto_detach_kernel_driver(true);
        handle
            .claim_interface(iface)
            .map_err(|e| format!("claim interface {iface}: {e} (a VCP driver may own it)"))?;
        Ok(UsbFtdiPort {
            handle,
            ep_out,
            ep_in,
            index,
            max_packet,
            detail: format!("{} channel {}", pick.detail(), ch.to_ascii_uppercase()),
        })
    }
}

#[cfg(feature = "usb-native")]
impl UsbFtdiPort {
    fn control(&mut self, req: u8, value: u16) -> Result<(), String> {
        self.handle
            .write_control(
                rusb::request_type(
                    rusb::Direction::Out,
                    rusb::RequestType::Vendor,
                    rusb::Recipient::Device,
                ),
                req,
                value,
                self.index,
                &[],
                std::time::Duration::from_millis(500),
            )
            .map(|_| ())
            .map_err(|e| format!("FTDI control {req:#04x}/{value:#06x}: {e}"))
    }
}

#[cfg(feature = "usb-native")]
impl MpssePort for UsbFtdiPort {
    fn describe(&self) -> String {
        self.detail.clone()
    }

    fn usb_reset(&mut self) -> Result<(), String> {
        self.control(SIO_RESET, SIO_RESET_SIO)
    }

    fn set_latency_timer(&mut self, ms: u8) -> Result<(), String> {
        self.control(SIO_SET_LATENCY_TIMER, u16::from(ms))
    }

    fn set_bitmode(&mut self, mask: u8, mode: u8) -> Result<(), String> {
        self.control(SIO_SET_BITMODE, u16::from(mask) | (u16::from(mode) << 8))
    }

    fn purge(&mut self) -> Result<(), String> {
        self.control(SIO_RESET, SIO_RESET_PURGE_RX)?;
        self.control(SIO_RESET, SIO_RESET_PURGE_TX)
    }

    fn write(&mut self, data: &[u8]) -> Result<(), String> {
        let t = std::time::Duration::from_millis(2000);
        let mut at = 0;
        while at < data.len() {
            let n = self
                .handle
                .write_bulk(self.ep_out, &data[at..], t)
                .map_err(|e| format!("FTDI bulk OUT: {e}"))?;
            if n == 0 {
                return Err("FTDI bulk OUT wrote 0 bytes".into());
            }
            at += n;
        }
        Ok(())
    }

    fn read(&mut self, n: usize) -> Result<Vec<u8>, String> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(2000);
        let mut out = Vec::with_capacity(n);
        let mut buf = vec![0u8; self.max_packet * 8];
        while out.len() < n {
            if std::time::Instant::now() > deadline {
                return Err(format!(
                    "FTDI bulk IN timeout: {} of {n} byte(s) — no TDO invented",
                    out.len()
                ));
            }
            let got = match self.handle.read_bulk(
                self.ep_in,
                &mut buf,
                std::time::Duration::from_millis(200),
            ) {
                Ok(k) => k,
                Err(rusb::Error::Timeout) => 0,
                Err(e) => return Err(format!("FTDI bulk IN: {e}")),
            };
            out.extend(strip_modem_status(&buf[..got], self.max_packet));
        }
        if out.len() > n {
            return Err(format!(
                "FTDI bulk IN: {} byte(s) for a {n}-byte read — stream out of sync",
                out.len()
            ));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_two_status_bytes_per_packet() {
        // Two 4-byte packets: [st st d0 d1][st st d2 d3]
        let raw = [0x31, 0x60, 1, 2, 0x31, 0x60, 3, 4];
        assert_eq!(strip_modem_status(&raw, 4), vec![1, 2, 3, 4]);
        // Status-only packet (no data yet) yields nothing.
        assert!(strip_modem_status(&[0x31, 0x60], 512).is_empty());
        // Short final packet.
        assert_eq!(strip_modem_status(&[0x31, 0x60, 9], 512), vec![9]);
    }

    #[test]
    fn channel_endpoints_match_ft2232h() {
        assert_eq!(channel_endpoints('A'), Some((0x02, 0x81, 1)));
        assert_eq!(channel_endpoints('b'), Some((0x04, 0x83, 2)));
        assert_eq!(channel_endpoints('C'), None);
    }

    #[test]
    fn open_without_cable_is_an_honest_error() {
        // A host with a cable attached opens it; otherwise the error says why.
        if let Err(e) = open_usb_ftdi(None, 'A') {
            assert!(e.contains("usb-native") || e.contains("FTDI"), "{e}");
        }
    }
}

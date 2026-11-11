//! Package-level I/O reports over an implemented design: UG899
//! `report_ssn` and `write_ibis`, plus UG835 `report_control_sets`.
//!
//! The design's top-level ports are mapped to the IOB sites the placer
//! gave them, then to the balls of the part's modeled BGA package
//! ([`Device::default_package_kind`]). Electrical properties come from the
//! port attributes (`IOSTANDARD`, `DRIVE`, `SLEW`, ...) the user set; a
//! port without `IOSTANDARD` uses the HL10T default `LVCMOS33`, and the
//! row says so.

use helion_device::{
    Device, IbisPin, IoDirection, IoPinAssignment, IoPlan, PackageKind, SsnReport, analyze_ssn,
    write_ibis,
};
use helion_ir::{Design, PortDir, resolve};
use helion_pack::ControlSetReport;
use helion_place::Placed;
use std::collections::HashMap;

/// IOSTANDARD the HAD assumes when a port has none.
pub const DEFAULT_IOSTANDARD: &str = "LVCMOS33";

/// One placed top-level port with its ball and electrical properties.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedIoPort {
    pub port: String,
    pub dir: IoDirection,
    pub site: String,
    pub ball: String,
    pub bank: u32,
    pub iostandard: String,
    /// True when `iostandard` is the default (no `IOSTANDARD` set).
    pub default_std: bool,
    pub drive_ma: Option<u32>,
    pub slew: Option<String>,
    pub pulltype: Option<String>,
}

/// Ports placed on IOBs, mapped onto `kind`'s balls.
pub fn placed_io_ports(
    design: &Design,
    placed: &Placed,
    dev: &Device,
    kind: PackageKind,
) -> Vec<PlacedIoPort> {
    let pkg = dev.bga_package(kind);
    let iob_ix: HashMap<&str, usize> = placed
        .packed
        .iobs
        .iter()
        .enumerate()
        .map(|(i, io)| (io.cell.as_str(), i))
        .collect();
    let mut out = Vec::new();
    for p in &design.ports {
        let name = resolve(p.name).to_string();
        let Some(net) = design.nets.iter().find(|n| n.name == p.name) else {
            continue;
        };
        let Some(ix) = net
            .endpoints
            .iter()
            .find_map(|e| iob_ix.get(resolve(e.cell)).copied())
        else {
            continue;
        };
        let Some(site) = placed.iob_sites.get(ix).copied() else {
            continue;
        };
        let Some(ball) = pkg.ball_for_site(site) else {
            continue;
        };
        let attr = |k: &str| p.attrs.get(k).map(str::to_string);
        let (iostandard, default_std) = match attr("IOSTANDARD") {
            Some(s) => (s, false),
            None => (DEFAULT_IOSTANDARD.to_string(), true),
        };
        out.push(PlacedIoPort {
            port: name,
            dir: match p.dir {
                PortDir::In => IoDirection::Input,
                PortDir::Out => IoDirection::Output,
                PortDir::Inout => IoDirection::Inout,
            },
            site: site.id(),
            ball: ball.ball.clone(),
            bank: ball.bank.unwrap_or(0),
            iostandard,
            default_std,
            drive_ma: attr("DRIVE").and_then(|d| d.trim().parse().ok()),
            slew: attr("SLEW").map(|s| s.to_ascii_uppercase()),
            pulltype: attr("PULLTYPE"),
        });
    }
    out
}

/// `report_ssn` over the placed ports. `toggle_mhz` is the switching rate
/// assumed for outputs (the fastest design clock is a sound choice).
pub fn report_ssn_design(
    design: &Design,
    placed: &Placed,
    dev: &Device,
    toggle_mhz: f64,
) -> Result<(PackageKind, Vec<PlacedIoPort>, SsnReport), String> {
    let kind = dev.default_package_kind();
    let ports = placed_io_ports(design, placed, dev, kind);
    if ports.is_empty() {
        return Err("report_ssn: no top-level port is placed on an IOB".into());
    }
    let mut plan = IoPlan::new();
    for p in &ports {
        let mut a = IoPinAssignment::new(&p.port, Some(&p.ball), &p.iostandard, p.dir);
        a.drive_ma = p.drive_ma;
        a.slew = p.slew.clone();
        a.pulltype = p.pulltype.clone();
        a.toggle_rate_mhz = toggle_mhz.max(1.0);
        plan.add(a);
    }
    let pkg = dev.bga_package(kind);
    Ok((kind, ports, analyze_ssn(&pkg, &plan)))
}

/// `write_ibis` text for the placed ports.
pub fn write_ibis_design(
    design: &Design,
    placed: &Placed,
    dev: &Device,
    file_name: &str,
) -> Result<(PackageKind, Vec<PlacedIoPort>, String), String> {
    let kind = dev.default_package_kind();
    let ports = placed_io_ports(design, placed, dev, kind);
    let pins: Vec<IbisPin> = ports
        .iter()
        .map(|p| IbisPin {
            ball: p.ball.clone(),
            signal: p.port.clone(),
            iostandard: p.iostandard.clone(),
            drive_ma: p.drive_ma,
            fast: p.slew.as_deref() == Some("FAST"),
        })
        .collect();
    let pkg = dev.bga_package(kind);
    let component = dev.part.split('-').next().unwrap_or(&dev.part).to_string();
    let text = write_ibis(&pkg, &component, file_name, &pins)?;
    Ok((kind, ports, text))
}

/// `report_control_sets` with slice counts from the placement when given.
pub fn report_control_sets_design(
    design: &Design,
    placed: Option<&Placed>,
    dev: &Device,
) -> ControlSetReport {
    let sites: Option<HashMap<String, (u32, u32)>> = placed.map(|pl| {
        pl.packed
            .lutffs
            .iter()
            .zip(&pl.lutff_sites)
            .filter(|(lf, _)| !lf.ff_cell.is_empty())
            .map(|(lf, (s, _))| (lf.ff_cell.clone(), (s.x, s.y)))
            .collect()
    });
    helion_pack::report_control_sets(design, sites.as_ref(), dev.n_ble.max(1) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Mode, Session};

    fn implemented() -> (Device, Session) {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::structural_counter();
        d.set_port_attr("led", "IOSTANDARD", "LVCMOS18").unwrap();
        d.set_port_attr("led", "DRIVE", "8").unwrap();
        let mut s = Session::new(Mode::Project);
        s.impl_design(d, &dev).unwrap();
        (dev, s)
    }

    #[test]
    fn ssn_maps_placed_ports_to_balls_with_their_properties() {
        let (dev, s) = implemented();
        let (kind, ports, rep) =
            report_ssn_design(s.design.as_ref().unwrap(), s.placed.as_ref().unwrap(), &dev, 100.0)
                .unwrap();
        assert_eq!(kind, dev.default_package_kind());
        let led = ports.iter().find(|p| p.port == "led").unwrap();
        assert_eq!(led.iostandard, "LVCMOS18");
        assert!(!led.default_std);
        assert_eq!(led.drive_ma, Some(8));
        let entry = rep.pin_entry("led").expect("led analyzed");
        assert_eq!(entry.ball, led.ball);
        assert!(entry.noise_budget_mv > 0.0);
    }

    #[test]
    fn ibis_lists_the_placed_ports() {
        let (dev, s) = implemented();
        let (_, ports, text) =
            write_ibis_design(s.design.as_ref().unwrap(), s.placed.as_ref().unwrap(), &dev, "counter.ibs")
                .unwrap();
        assert!(!ports.is_empty());
        assert!(text.contains("[Model]         LVCMOS18_8S"));
        assert!(text.contains("[File Name]     counter.ibs"));
    }

    #[test]
    fn control_sets_use_placed_slices() {
        let (dev, s) = implemented();
        let r = report_control_sets_design(s.design.as_ref().unwrap(), s.placed.as_ref(), &dev);
        assert!(r.placed);
        assert_eq!(r.total_ffs(), 4, "counter has four flops");
        assert_eq!(r.total(), 1, "one clock, no enable/reset");
    }
}

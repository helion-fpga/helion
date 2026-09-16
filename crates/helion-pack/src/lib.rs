//! Pack logical cells into Helion site primitives (LUTFF + IOB + MAC27 + BRAM18 + ILA).

use helion_device::{Device, Site};
use helion_ir::{CellKind, Design};
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct Packed {
    pub lutffs: Vec<PackedLutFf>,
    pub iobs: Vec<PackedIob>,
    pub macs: Vec<PackedMac>,
    pub brams: Vec<PackedBram>,
}

#[derive(Clone, Debug)]
pub struct PackedBram {
    pub cell: String,
    /// INIT words from IR attr `INIT` (comma-separated hex).
    pub init: Vec<u64>,
}

#[derive(Clone, Debug)]
pub struct PackedLutFf {
    pub lut_cell: String,
    pub ff_cell: String,
    pub init: u64,
    /// LUT pin → FF cell whose Q drives it (local cluster).
    pub lut_pins: Vec<(u8, String)>,
    /// Q net of the packed FF (IOB matching).
    pub q_net: String,
}

#[derive(Clone, Debug)]
pub struct PackedIob {
    pub cell: String,
    pub from_net: String,
    /// Optional `IOB_XxYy` from IR port LOC.
    pub loc: Option<String>,
    /// UG893 I/O Ports `DRIVE` (mA), copied from the PAD port.
    pub drive: Option<String>,
    /// UG893 I/O Ports `SLEW`.
    pub slew: Option<String>,
    /// UG893 I/O Ports `PULLTYPE`.
    pub pulltype: Option<String>,
    /// UG893 I/O Ports `DIFF_TERM`.
    pub diff_term: Option<String>,
    /// UG893 I/O Ports `IN_TERM`.
    pub in_term: Option<String>,
}

#[derive(Clone, Debug)]
pub struct PackedMac {
    pub cell: String,
}

/// LUTFF occupancy per HAD CLB site ID (`CLB_X2Y1` → N). Derived from
/// placement sites, not a decorative pie.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PackingSummary {
    pub sites: BTreeMap<String, usize>,
}

impl PackingSummary {
    /// Group placed LUTFF clusters by [`Site::id`].
    pub fn from_lutff_sites(sites: &[(Site, u8)]) -> Self {
        let mut m = BTreeMap::new();
        for (site, _) in sites {
            *m.entry(site.id()).or_insert(0) += 1;
        }
        Self { sites: m }
    }

    pub fn lutff_in(&self, site_id: &str) -> usize {
        self.sites.get(site_id).copied().unwrap_or(0)
    }
}

pub fn pack(design: &Design, _dev: &Device) -> Result<Packed, String> {
    // Ibex-scale: linear scans via Design::net_on are O(n^3). Index once.
    let pins = design.pin_index();
    let mut d_driver: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    let mut q_driver: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for c in &design.cells {
        if matches!(c.kind, CellKind::Hff) {
            if let Some(d) = pins.net_on(&c.name, "D") {
                d_driver.entry(d).or_insert(c.name.as_str());
            }
            if let Some(q) = pins.net_on(&c.name, "Q") {
                q_driver.insert(q, c.name.as_str());
            }
        }
    }
    let mut lutffs = Vec::new();
    let mut used_ff = std::collections::HashSet::new();
    for c in &design.cells {
        let CellKind::Lut6 { init } = c.kind else {
            continue;
        };
        let Some(o_net) = pins.net_on(&c.name, "O") else {
            continue;
        };
        // Comb LUTs (no FF on O) pack as LUT-only: empty ff_cell, q_net = O so
        // IOB/route match the LUT output. Registered LUTs keep the FF cluster.
        let ff_name = d_driver
            .get(o_net)
            .copied()
            .filter(|n| !used_ff.contains(*n));
        if let Some(n) = ff_name {
            used_ff.insert(n.to_string());
        }
        let mut lut_pins = Vec::new();
        const IPINS: [&str; 6] = ["I0", "I1", "I2", "I3", "I4", "I5"];
        for (pin, key) in IPINS.iter().enumerate() {
            if let Some(net) = pins.net_on(&c.name, key) {
                if let Some(src) = q_driver.get(net) {
                    lut_pins.push((pin as u8, (*src).to_string()));
                }
            }
        }
        let (ff_cell, q_net) = if let Some(n) = ff_name {
            (n.to_string(), pins.net_on(n, "Q").unwrap_or("").to_string())
        } else {
            (String::new(), o_net.to_string())
        };
        lutffs.push(PackedLutFf {
            lut_cell: c.name.clone(),
            ff_cell,
            init,
            lut_pins,
            q_net,
        });
    }
    let mut iobs = Vec::new();
    for c in &design.cells {
        if matches!(c.kind, CellKind::IobOut) {
            let net = pins
                .net_on(&c.name, "I")
                .ok_or_else(|| format!("IOB {} has no I net", c.name))?;
            let pad = pins.net_on(&c.name, "PAD").unwrap_or("");
            let port = design.ports.iter().find(|p| p.name == pad);
            let loc = port.and_then(|p| p.attrs.get("LOC").map(|s| s.to_string()));
            iobs.push(PackedIob {
                cell: c.name.clone(),
                from_net: net.to_string(),
                loc,
                drive: port.and_then(|p| p.attrs.get("DRIVE").map(|s| s.to_string())),
                slew: port.and_then(|p| p.attrs.get("SLEW").map(|s| s.to_string())),
                pulltype: port.and_then(|p| p.attrs.get("PULLTYPE").map(|s| s.to_string())),
                diff_term: port.and_then(|p| p.attrs.get("DIFF_TERM").map(|s| s.to_string())),
                in_term: port.and_then(|p| p.attrs.get("IN_TERM").map(|s| s.to_string())),
            });
        }
    }
    let mut macs = Vec::new();
    for c in &design.cells {
        if matches!(c.kind, CellKind::Mac27) {
            macs.push(PackedMac {
                cell: c.name.clone(),
            });
        }
    }
    let mut brams = Vec::new();
    for c in &design.cells {
        if matches!(c.kind, CellKind::Bram18) {
            let init = c
                .attrs
                .get("INIT")
                .unwrap_or("")
                .split(',')
                .filter(|t| !t.is_empty())
                .map(|t| u64::from_str_radix(t.trim(), 16).unwrap_or(0))
                .collect();
            brams.push(PackedBram {
                cell: c.name.clone(),
                init,
            });
        }
    }
    Ok(Packed {
        lutffs,
        iobs,
        macs,
        brams,
    })
}

/// Copy HNF port DRIVE / SLEW / PULLTYPE / DIFF_TERM / IN_TERM onto packed IOBs
/// (post-pack set_property).
pub fn apply_iob_electrical(design: &Design, iobs: &mut [PackedIob]) {
    let pins = design.pin_index();
    let ports: std::collections::HashMap<&str, &helion_ir::Port> =
        design.ports.iter().map(|p| (p.name.as_str(), p)).collect();
    for iob in iobs.iter_mut() {
        let pad = pins.net_on(&iob.cell, "PAD").unwrap_or("");
        if let Some(p) = ports.get(pad) {
            iob.drive = p.attrs.get("DRIVE").map(|s| s.to_string());
            iob.slew = p.attrs.get("SLEW").map(|s| s.to_string());
            iob.pulltype = p.attrs.get("PULLTYPE").map(|s| s.to_string());
            iob.diff_term = p.attrs.get("DIFF_TERM").map(|s| s.to_string());
            iob.in_term = p.attrs.get("IN_TERM").map(|s| s.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_device::Device;
    use helion_ir::{CellKind, Design};

    #[test]
    fn packs_blinky_lutff() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_blinky(), &dev).unwrap();
        assert_eq!(p.lutffs.len(), 1);
        assert_eq!(p.iobs.len(), 1);
        assert_eq!(p.lutffs[0].init, 0x5555_5555_5555_5555);
        assert_eq!(p.lutffs[0].lut_pins, vec![(0, "u_ff".into())]);
        assert_eq!(p.lutffs[0].q_net, "q");
    }

    #[test]
    fn packs_counter_four_lutffs_with_pins() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(&Design::structural_counter(), &dev).unwrap();
        assert_eq!(p.lutffs.len(), 4);
        assert_eq!(p.lutffs[3].lut_pins.len(), 4);
        assert_eq!(p.iobs[0].from_net, "q3");
        assert_eq!(p.lutffs[3].q_net, "q3");
        assert!(p.iobs[0].drive.is_none());
        assert!(p.iobs[0].slew.is_none());
        assert!(p.iobs[0].pulltype.is_none());
        assert!(p.iobs[0].diff_term.is_none());
        assert!(p.iobs[0].in_term.is_none());
        let mut d = Design::structural_counter();
        d.set_drive("led", "4").unwrap();
        d.set_slew("led", "FAST").unwrap();
        d.set_pulltype("led", "PULLUP").unwrap();
        d.set_diff_term("led", "TRUE").unwrap();
        d.set_in_term("led", "UNTUNED_SPLIT_50").unwrap();
        let p = pack(&d, &dev).unwrap();
        assert_eq!(p.iobs[0].drive.as_deref(), Some("4"));
        assert_eq!(p.iobs[0].slew.as_deref(), Some("FAST"));
        assert_eq!(p.iobs[0].pulltype.as_deref(), Some("PULLUP"));
        assert_eq!(p.iobs[0].diff_term.as_deref(), Some("TRUE"));
        assert_eq!(p.iobs[0].in_term.as_deref(), Some("UNTUNED_SPLIT_50"));
    }

    #[test]
    fn packs_comb_lut_without_ff() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::new("comb");
        d.add_port("a", helion_ir::PortDir::In);
        d.add_port("y", helion_ir::PortDir::Out);
        d.add_cell(
            "u_lut",
            CellKind::Lut6 {
                init: 0x5555_5555_5555_5555,
            },
        );
        d.add_cell("u_iob", CellKind::IobOut);
        d.connect("a", "u_lut", "I0");
        d.connect("n", "u_lut", "O");
        d.connect("n", "u_iob", "I");
        d.connect("y", "u_iob", "PAD");
        let p = pack(&d, &dev).unwrap();
        assert_eq!(p.lutffs.len(), 1);
        assert!(
            p.lutffs[0].ff_cell.is_empty(),
            "comb LUT must not invent an FF"
        );
        assert_eq!(p.lutffs[0].q_net, "n");
        assert_eq!(p.iobs[0].from_net, "n");
    }

    #[test]
    fn packs_mac27() {
        let dev = Device::load_part("HL10T-DSP1").unwrap();
        let mut d = Design::new("mac");
        d.add_cell("u_mac", CellKind::Mac27);
        let p = pack(&d, &dev).unwrap();
        assert_eq!(p.macs.len(), 1);
        assert!(p.lutffs.is_empty());
    }

    #[test]
    fn pack_thousands_of_lutff_pairs_is_subquadratic() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::new("wide");
        d.add_port("clk", helion_ir::PortDir::In);
        const N: u32 = 3_000;
        for i in 0..N {
            let lut = format!("lut{i}");
            let ff = format!("ff{i}");
            d.add_cell(
                &lut,
                CellKind::Lut6 {
                    init: 0x5555_5555_5555_5555,
                },
            );
            d.add_cell(&ff, CellKind::Hff);
            d.connect("clk", &ff, "CLK");
            d.connect(format!("d{i}"), &lut, "O");
            d.connect(format!("d{i}"), &ff, "D");
            d.connect(format!("q{i}"), &ff, "Q");
            d.connect(format!("q{i}"), &lut, "I0");
        }
        let t0 = std::time::Instant::now();
        let p = pack(&d, &dev).unwrap();
        let ms = t0.elapsed().as_millis();
        assert_eq!(p.lutffs.len(), N as usize, "every LUT/FF pair packs");
        assert!(
            p.lutffs
                .iter()
                .all(|l| !l.ff_cell.is_empty() && !l.q_net.is_empty()),
            "clusters keep FF and Q net"
        );
        assert!(
            ms < 2000,
            "pack of {N} LUT/FF pairs must stay subquadratic, took {ms}ms"
        );
    }

    #[test]
    fn packs_bram18() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::new("ram");
        d.add_cell("u_bram", CellKind::Bram18);
        let p = pack(&d, &dev).unwrap();
        assert_eq!(p.brams.len(), 1);
    }

    #[test]
    fn packing_summary_groups_lutff_sites_by_had_id() {
        use helion_device::SiteKind;
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let s0 = Site {
            x: dev.clb_x0,
            y: dev.clb_y0,
            kind: SiteKind::Clb,
        };
        let s1 = Site {
            x: dev.clb_x0,
            y: dev.clb_y0 + 1,
            kind: SiteKind::Clb,
        };
        assert!(dev.contains_site(s0) && dev.contains_site(s1));
        let sites = vec![(s0, 0), (s0, 1), (s0, 2), (s1, 0)];
        let sum = PackingSummary::from_lutff_sites(&sites);
        assert_eq!(sum.lutff_in(&s0.id()), 3);
        assert_eq!(sum.lutff_in(&s1.id()), 1);
        assert_eq!(dev.site_by_id(&s0.id()), Some(s0));
        assert_eq!(dev.site_by_id(&s1.id()), Some(s1));
        assert_eq!(sum.sites.len(), 2);
    }
}

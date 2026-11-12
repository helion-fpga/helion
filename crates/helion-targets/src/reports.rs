//! Family-accurate implementation reports for non-Helion parts: resource
//! utilization from the family packer against the part's real capacities,
//! the family timing model, design-rule checks against the family's package
//! pins and capacities, and the die geometry / placement for the Device
//! view. Every number here comes from the family crate (helion-ice40 /
//! helion-ecp5) or the family's `TimingModel` in helion-backend, never from
//! the Helion HL10T device model.

use std::collections::{BTreeMap, HashMap};

use helion_backend::TimingModel;
use helion_ir::{Design, resolve};

use crate::{Family, family_of, insert_io_buffers};

/// One utilization row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UtilRow {
    /// Family resource name (`LUT4`, `EBR (4 Kb)`, ...).
    pub resource: &'static str,
    pub used: usize,
    pub available: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FamilyUtilization {
    pub family: Family,
    pub part: String,
    pub rows: Vec<UtilRow>,
}

impl FamilyUtilization {
    pub fn row(&self, resource: &str) -> Option<&UtilRow> {
        self.rows.iter().find(|r| r.resource == resource)
    }
}

/// HX8K capacities (Lattice FPGA-DS-02029): 7680 logic cells (one LUT4 +
/// one DFF each), 32 sysMEM EBR (4 Kb), 2 PLLs on CT256, 8 global buffers,
/// 206 user I/O on CT256. HX parts have no MAC16 / SPRAM.
const ICE40_HX8K: [(&str, usize); 6] = [
    ("LUT4", 7680),
    ("DFF", 7680),
    ("EBR (4 Kb)", 32),
    ("I/O (CT256)", 206),
    ("PLL", 2),
    ("Global buffer", 8),
];

fn family_part(part: &str) -> Result<Family, String> {
    match family_of(part) {
        Some(f @ (Family::Ice40 | Family::Ecp5)) => Ok(f),
        Some(f) => Err(format!("{part} is a {} part: no family report here", f.label())),
        None => Err(format!("no backend implements part {part}")),
    }
}

fn padded(design: &Design) -> Result<Design, String> {
    let mut d = design.clone();
    insert_io_buffers(&mut d)?;
    Ok(d)
}

/// The family packers refuse ports without a package pin. Logic, register,
/// RAM and DSP counts do not depend on which pin a port lands on, so for
/// *counting* give each unpinned port a distinct free pin of the package
/// (the DRC reports those ports as errors for implementation).
fn pinned_for_count(design: &Design, family: Family) -> Result<Design, String> {
    let package = if family == Family::Ecp5 { "CABGA381" } else { "CT256" };
    let mut pins = known_pins(family, package).unwrap_or_default();
    pins.sort();
    let used: Vec<String> = design
        .ports
        .iter()
        .filter_map(|p| p.attrs.get("LOC").map(|v| v.to_string().to_ascii_uppercase()))
        .collect();
    let mut free = pins.into_iter().filter(|p| !used.contains(&p.to_ascii_uppercase()));
    let mut d = design.clone();
    let unpinned: Vec<String> =
        d.ports.iter().filter(|p| p.attrs.get("LOC").is_none()).map(|p| resolve(p.name).to_string()).collect();
    for port in unpinned {
        let pin = free.next().ok_or_else(|| format!("more ports than {package} pins the {} flow knows", family.label()))?;
        let _ = d.set_port_attr(&port, "LOC", pin);
    }
    Ok(d)
}

/// Count sites of an ECP5 class over the whole device.
fn ecp5_sites(dev: &helion_ecp5::Device, pred: impl Fn(&str) -> bool) -> usize {
    dev.tiles().iter().map(|t| t.sites.iter().filter(|s| pred(&s.name)).count()).sum()
}

/// Utilization of `design` packed for `part` (iCE40 / ECP5).
pub fn utilization(design: &Design, part: &str) -> Result<FamilyUtilization, String> {
    let family = family_part(part)?;
    let d = padded(&pinned_for_count(design, family)?)?;
    let rows = match family {
        Family::Ice40 => {
            let p = helion_ice40::pack::pack(&d)?;
            let used = [p.luts.len(), p.ffs.len(), p.rams.len(), p.ios.len(), p.plls.len(), p.clks.len()];
            let mut rows: Vec<UtilRow> = ICE40_HX8K
                .iter()
                .zip(used)
                .map(|((r, a), u)| UtilRow { resource: r, used: u, available: *a })
                .collect();
            if !p.mac16s.is_empty() {
                rows.push(UtilRow { resource: "MAC16", used: p.mac16s.len(), available: 0 });
            }
            rows
        }
        Family::Ecp5 => {
            let p = helion_ecp5::pack::pack(&d)?;
            let dev = helion_ecp5::Device::global()?;
            let c = dev.counts();
            let pins = dev.package_pins(&p.package).map(|m| m.len()).unwrap_or(c.bonded_pios);
            let mult18 = ecp5_sites(dev, |n| n.starts_with("MULT18_"));
            vec![
                UtilRow { resource: "LUT4", used: p.luts.len(), available: c.lut4 },
                UtilRow { resource: "FF", used: p.ffs.len(), available: c.ff },
                UtilRow { resource: "EBR (18 Kb)", used: p.rams.len(), available: c.ebr_bels },
                UtilRow { resource: "I/O", used: p.ios.len(), available: pins },
                UtilRow { resource: "MULT18", used: p.dsps.len(), available: mult18 },
                UtilRow { resource: "PLL", used: p.plls.len(), available: c.pll_bels },
                UtilRow { resource: "Global clock (DCC)", used: p.clks.len(), available: c.dcc_bels },
            ]
        }
        _ => unreachable!("family_part"),
    };
    Ok(FamilyUtilization { family, part: part.to_string(), rows })
}

/// The family's timing model (delays in ps) and a description of how far
/// it can be trusted.
pub fn timing_model(part: &str) -> Option<(TimingModel, &'static str)> {
    Some(match family_of(part)? {
        Family::Helion => (TimingModel::hl10t(), "HL10T gold timing model"),
        Family::Ice40 => (
            TimingModel::ice40_hx8k(),
            "iCE40-HX8K delay model (helion-backend TimingModel::ice40_hx8k): datasheet-anchored, not yet correlated against nextpnr/iCEcube2",
        ),
        Family::Ecp5 => (
            TimingModel::ecp5_85f(),
            "ECP5 LFE5U-85F delay model (helion-backend TimingModel::ecp5_85f): order-of-magnitude placeholders, not yet correlated against nextpnr/Diamond",
        ),
        Family::Xilinx7 => (
            TimingModel::xilinx7(),
            "Xilinx 7-series (-1) delay model (helion-backend TimingModel::xilinx7): preliminary, not correlated against Vivado",
        ),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrcSeverity {
    Error,
    Warning,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FamilyDrc {
    pub id: &'static str,
    pub severity: DrcSeverity,
    pub objects: String,
    pub message: String,
}

/// Package pins the family flow can place a `LOC` on.
fn known_pins(family: Family, package: &str) -> Option<Vec<String>> {
    match family {
        Family::Ice40 => Some(helion_ice40::device::ct256_pin_map().keys().map(|k| k.to_string()).collect()),
        Family::Ecp5 => helion_ecp5::Device::global()
            .ok()?
            .package_pins(package)
            .map(|m| m.keys().cloned().collect()),
        _ => None,
    }
}

/// Design-rule checks for `design` on `part`: capacity, package pins, and
/// whether the family packer accepts the netlist at all.
pub fn drc(design: &Design, part: &str) -> Result<Vec<FamilyDrc>, String> {
    let family = family_part(part)?;
    let mut out = Vec::new();
    match utilization(design, part) {
        Ok(u) => {
            for r in &u.rows {
                if r.used > r.available {
                    out.push(FamilyDrc {
                        id: "UTLZ-1",
                        severity: DrcSeverity::Error,
                        objects: r.resource.to_string(),
                        message: format!("{} {} used, {} has {}", r.used, r.resource, part, r.available),
                    });
                }
            }
        }
        Err(e) => out.push(FamilyDrc {
            id: "PACK-1",
            severity: DrcSeverity::Error,
            objects: resolve(design.name).to_string(),
            message: format!("{} packer rejects the netlist: {e}", family.label()),
        }),
    }
    let package = if family == Family::Ecp5 { "CABGA381" } else { "CT256" };
    let pins = known_pins(family, package).unwrap_or_default();
    let mut by_pin: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut unplaced = Vec::new();
    for p in &design.ports {
        let name = resolve(p.name).to_string();
        match p.attrs.get("LOC").or_else(|| p.attrs.get("PACKAGE_PIN")) {
            Some(pin) => {
                let pin = pin.to_string();
                if !pins.iter().any(|k| k.eq_ignore_ascii_case(&pin)) {
                    out.push(FamilyDrc {
                        id: "IO-2",
                        severity: DrcSeverity::Error,
                        objects: name.clone(),
                        message: format!("pin {pin} is not a {package} pin the {} flow can place", family.label()),
                    });
                }
                by_pin.entry(pin.to_ascii_uppercase()).or_default().push(name);
            }
            None => unplaced.push(name),
        }
    }
    for (pin, ports) in by_pin {
        if ports.len() > 1 {
            out.push(FamilyDrc {
                id: "IO-3",
                severity: DrcSeverity::Error,
                objects: ports.join(", "),
                message: format!("{} ports share pin {pin}", ports.len()),
            });
        }
    }
    if !unplaced.is_empty() {
        unplaced.sort();
        out.push(FamilyDrc {
            id: "IO-1",
            severity: DrcSeverity::Error,
            objects: unplaced.join(", "),
            message: format!(
                "{} port(s) have no package pin; the {} packer needs a LOC/PACKAGE_PIN for every port",
                unplaced.len(),
                family.label()
            ),
        });
    }
    Ok(out)
}

/// Tile class for the Device view.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TileClass {
    Logic,
    Ram,
    Dsp,
    Io,
    Clock,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DieTile {
    pub x: u32,
    pub y: u32,
    pub class: TileClass,
}

/// A family die: tile grid in the family crate's coordinates (iCE40
/// `(x, y)` with y up; ECP5 `(col, row)` with row down).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DieGrid {
    pub family: Family,
    pub part: String,
    pub cols: u32,
    pub rows: u32,
    /// True when row 0 is the top edge (ECP5); false when it is the bottom
    /// (iCE40).
    pub row0_top: bool,
    pub tiles: Vec<DieTile>,
}

impl DieGrid {
    pub fn count(&self, class: TileClass) -> usize {
        self.tiles.iter().filter(|t| t.class == class).count()
    }
}

/// Die geometry from the family crate's device model.
pub fn die(part: &str) -> Result<DieGrid, String> {
    let family = family_part(part)?;
    match family {
        Family::Ice40 => {
            use helion_ice40::device::{MAX_X, MAX_Y, TileKind, tile_kind};
            let mut tiles = Vec::new();
            for y in 0..=MAX_Y {
                for x in 0..=MAX_X {
                    if let Some(k) = tile_kind(x, y) {
                        let class = match k {
                            TileKind::Logic => TileClass::Logic,
                            TileKind::Ramb | TileKind::Ramt => TileClass::Ram,
                            _ => TileClass::Io,
                        };
                        tiles.push(DieTile { x: x as u32, y: y as u32, class });
                    }
                }
            }
            Ok(DieGrid { family, part: part.to_string(), cols: MAX_X as u32 + 1, rows: MAX_Y as u32 + 1, row0_top: false, tiles })
        }
        Family::Ecp5 => {
            use helion_ecp5::device::SiteClass;
            let dev = helion_ecp5::Device::global()?;
            let mut at: HashMap<(u32, u32), TileClass> = HashMap::new();
            for t in dev.tiles() {
                for s in &t.sites {
                    let c = match s.class {
                        SiteClass::Slice => TileClass::Logic,
                        SiteClass::Ebr => TileClass::Ram,
                        SiteClass::Dsp => TileClass::Dsp,
                        SiteClass::Pio | SiteClass::IoLogic => TileClass::Io,
                        SiteClass::Pll | SiteClass::Dcc | SiteClass::Dcs | SiteClass::ClkDiv | SiteClass::Eclk => TileClass::Clock,
                        _ => TileClass::Other,
                    };
                    let e = at.entry((t.col, t.row)).or_insert(c);
                    if c < *e {
                        *e = c;
                    }
                }
            }
            let mut tiles: Vec<DieTile> = at.into_iter().map(|((x, y), class)| DieTile { x, y, class }).collect();
            tiles.sort_by_key(|t| (t.y, t.x));
            Ok(DieGrid {
                family,
                part: part.to_string(),
                cols: helion_ecp5::device::GRID_COLS,
                rows: helion_ecp5::device::GRID_ROWS,
                row0_top: true,
                tiles,
            })
        }
        _ => unreachable!("family_part"),
    }
}

/// One placed primitive in die coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlacedPrim {
    pub name: String,
    pub x: u32,
    pub y: u32,
    pub class: TileClass,
}

/// Run the family packer + placer and return primitive positions.
pub fn placement(design: &Design, part: &str) -> Result<Vec<PlacedPrim>, String> {
    let family = family_part(part)?;
    let d = padded(design)?;
    let mut out = Vec::new();
    match family {
        Family::Ice40 => {
            let p = helion_ice40::pack::pack(&d)?;
            let pl = helion_ice40::place::place(&p)?;
            for l in &pl.luts {
                out.push(PlacedPrim { name: l.name.clone(), x: l.x as u32, y: l.y as u32, class: TileClass::Logic });
            }
            for f in &pl.ffs {
                out.push(PlacedPrim { name: f.name.clone(), x: f.x as u32, y: f.y as u32, class: TileClass::Logic });
            }
            for i in &pl.ios {
                out.push(PlacedPrim { name: i.port.clone(), x: i.x as u32, y: i.y as u32, class: TileClass::Io });
            }
            for r in &pl.rams {
                out.push(PlacedPrim { name: r.name.clone(), x: r.x as u32, y: r.y as u32, class: TileClass::Ram });
            }
        }
        Family::Ecp5 => {
            let b = helion_ecp5::Ecp5Backend::open(part)?;
            use helion_backend::Backend;
            let p = b.pack(&d)?;
            let pl = b.place(&p)?;
            for l in &pl.luts {
                out.push(PlacedPrim { name: l.name.clone(), x: l.site.tile_col, y: l.site.tile_row, class: TileClass::Logic });
            }
            for f in &pl.ffs {
                out.push(PlacedPrim { name: f.name.clone(), x: f.site.tile_col, y: f.site.tile_row, class: TileClass::Logic });
            }
            for i in &pl.ios {
                out.push(PlacedPrim { name: i.port.clone(), x: i.site.tile_col, y: i.site.tile_row, class: TileClass::Io });
            }
            for e in &pl.ebrs {
                out.push(PlacedPrim { name: e.name.clone(), x: e.site.tile_col, y: e.site.tile_row, class: TileClass::Ram });
            }
            for m in &pl.dsps {
                out.push(PlacedPrim { name: m.name.clone(), x: m.site.tile_col, y: m.site.tile_row, class: TileClass::Dsp });
            }
        }
        _ => unreachable!("family_part"),
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLINKY: &str = "module blinky(input wire clk, output wire led);\n  reg [7:0] c = 0;\n  always @(posedge clk) c <= c + 8'd1;\n  assign led = c[7];\nendmodule\n";

    fn blinky() -> Design {
        helion_sv::synth_sv(BLINKY, "blinky.sv").expect("synth")
    }

    #[test]
    fn ice40_utilization_counts_lut4_dff_io_against_hx8k() {
        let u = utilization(&blinky(), "ice40-hx8k").unwrap();
        assert_eq!(u.family, Family::Ice40);
        let lut = u.row("LUT4").unwrap();
        assert_eq!(lut.available, 7680);
        assert!(lut.used >= 8, "8-bit counter needs LUT4s: {u:?}");
        assert_eq!(u.row("DFF").unwrap().used, 8);
        assert_eq!(u.row("I/O (CT256)").unwrap().used, 2);
        assert_eq!(u.row("EBR (4 Kb)").unwrap().available, 32);
    }

    #[test]
    fn ecp5_utilization_uses_the_trellis_device_counts() {
        let u = utilization(&blinky(), "LFE5U-85F").unwrap();
        let dev = helion_ecp5::Device::global().unwrap();
        assert_eq!(u.row("LUT4").unwrap().available, dev.counts().lut4);
        assert!(u.row("LUT4").unwrap().available > 80_000);
        assert_eq!(u.row("FF").unwrap().used, 8);
        assert!(u.row("MULT18").unwrap().available > 100);
    }

    #[test]
    fn timing_models_are_per_family() {
        let (ice, note) = timing_model("ice40-hx8k").unwrap();
        assert_eq!(ice, TimingModel::ice40_hx8k());
        assert!(note.contains("not yet correlated"));
        assert_eq!(timing_model("LFE5U-85F").unwrap().0, TimingModel::ecp5_85f());
        assert_ne!(ice, TimingModel::hl10t());
        assert!(timing_model("nope").is_none());
    }

    #[test]
    fn drc_flags_bad_and_duplicate_pins_and_unplaced_ports() {
        let mut d = blinky();
        crate::apply_pin_constraints(&mut d, &[("clk".into(), "J3".into()), ("led".into(), "ZZ99".into())]);
        let v = drc(&d, "ice40-hx8k").unwrap();
        assert!(v.iter().any(|x| x.id == "IO-2" && x.objects == "led"), "{v:?}");
        let mut d = blinky();
        crate::apply_pin_constraints(&mut d, &[("clk".into(), "B5".into()), ("led".into(), "B5".into())]);
        let v = drc(&d, "ice40-hx8k").unwrap();
        assert!(v.iter().any(|x| x.id == "IO-3"), "{v:?}");
        let v = drc(&blinky(), "ice40-hx8k").unwrap();
        assert!(v.iter().any(|x| x.id == "IO-1" && x.severity == DrcSeverity::Error), "{v:?}");
        let mut d = blinky();
        crate::apply_pin_constraints(&mut d, &[("clk".into(), "J3".into()), ("led".into(), "B5".into())]);
        let v = drc(&d, "ice40-hx8k").unwrap();
        assert!(v.is_empty(), "{v:?}");
    }

    #[test]
    fn die_geometry_and_placement_come_from_the_family_models() {
        let g = die("ice40-hx8k").unwrap();
        assert_eq!((g.cols, g.rows), (34, 34));
        // 32x32 interior minus two RAM columns; 2 x 32 RAM tiles.
        assert_eq!(g.count(TileClass::Logic), 30 * 32);
        assert_eq!(g.count(TileClass::Ram), 64);
        let e = die("LFE5U-85F").unwrap();
        assert_eq!((e.cols, e.rows), (126 + 1, 95 + 1));
        assert!(e.count(TileClass::Logic) > 1000);
        assert!(e.count(TileClass::Dsp) > 0 && e.count(TileClass::Ram) > 0);
        let mut d = blinky();
        crate::apply_pin_constraints(&mut d, &[("clk".into(), "J3".into()), ("led".into(), "B5".into())]);
        let p = placement(&d, "ice40-hx8k").unwrap();
        assert!(p.iter().filter(|c| c.class == TileClass::Logic).count() >= 8);
        let io = p.iter().find(|c| c.class == TileClass::Io && c.name == "led").unwrap();
        // B5 is IO tile (7, 33) in the CT256 map.
        assert_eq!((io.x, io.y), (7, 33));
        for c in &p {
            assert!(g.tiles.iter().any(|t| t.x == c.x && t.y == c.y), "{c:?} off-die");
        }
        assert!(die("HL10T-C32-1").is_err());
    }
}

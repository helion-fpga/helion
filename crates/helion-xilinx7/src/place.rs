//! Place stage: [`Packed7`] → [`Placed7`].
//!
//! Affinity and architecture-legality placer for 7-series sites:
//! - IOBs go to their package pins.
//! - Clock buffers (`BUFG`, `BUFGCE`, `BUFGCTRL`, `BUFIO`, `BUFR`) and CMTs
//!   (`MMCME2_ADV`, `PLLE2_ADV`) go to dedicated clock / CMT tiles.
//! - `CARRY4` chains (`CO[3] -> CI`), `DSP48E1` cascades (`PCOUT -> PCIN`,
//!   `ACOUT -> ACIN`, `BCOUT -> BCIN`), and `RAMB36E1` cascades
//!   (`CASCADEOUTA/B -> CASCADEINA/B`) are placed into vertically contiguous
//!   column-aligned sites (`X` constant, `Y` strictly ascending).
//! - `ISERDESE2`, `OSERDESE2`, `IDELAYE2`, `ODELAYE2`, `IDELAYCTRL`, `IDDR`,
//!   and `ODDR` enforce I/O bank and Master/Slave tile co-location rules.
//! - `SRL16E` and `SRLC32E` shift-register LUTs are placed exclusively into
//!   `SLICEM` sites.
//! - `MUXF7` (`F7AMUX`/`F7BMUX`) and `MUXF8` (`F8MUX`) co-locate in the same
//!   slice as their feeding LUTs/F7 muxes.

use crate::pack::{
    BramKind, CmtKind, IobDir, IodelayKind, MuxfKind, Packed7, PackedBram, PackedCarry, PackedDsp,
    SrlKind,
};
use std::collections::{HashMap, HashSet};

/// What the placer needs from the device model. Implemented by the real DB
/// (`crate::xdevice::XDevice`) and by test fakes.
pub trait DeviceQuery {
    /// `(tile, site)` for a package pin, e.g. `"E3"`.
    fn iob_site_for_pin(&self, pin: &str) -> Option<(String, String)>;
    /// A BUFGCTRL site: `(tile, site)`.
    fn bufg_site(&self) -> Option<(String, String)>;
    /// All BUFGCTRL sites `(tile, site)` in deterministic order.
    fn bufg_sites(&self) -> Vec<(String, String)> {
        self.bufg_site().into_iter().collect()
    }
    /// A RAMB18E1 site: `(tile, site)`.
    fn ramb_site(&self) -> Option<(String, String)>;
    /// All RAMB18E1 sites `(tile, site)` in column-major `(X, Y)` order.
    fn ramb18_sites(&self) -> Vec<(String, String)> {
        if let Some((t, s)) = self.ramb_site() {
            let mut v = vec![(t, s)];
            for y in 1..16 {
                v.push((format!("BRAM_L_X6Y{}", (y / 2) * 5), format!("RAMB18_X0Y{y}")));
            }
            v
        } else {
            Vec::new()
        }
    }
    /// All RAMB36E1 sites `(tile, site)` in column-major `(X, Y)` order.
    fn ramb36_sites(&self) -> Vec<(String, String)> {
        if let Some((t, _)) = self.ramb_site() {
            let mut v = vec![(t, "RAMB36_X0Y0".to_string())];
            for y in 1..8 {
                v.push((format!("BRAM_L_X6Y{}", y * 5), format!("RAMB36_X0Y{y}")));
            }
            v
        } else {
            Vec::new()
        }
    }
    /// A DSP48E1 site: `(tile, site)`.
    fn dsp_site(&self) -> Option<(String, String)>;
    /// All DSP48E1 sites `(tile, site)` in column-major `(X, Y)` order.
    fn dsp_sites(&self) -> Vec<(String, String)> {
        if let Some((t, s)) = self.dsp_site() {
            let mut v = vec![(t, s)];
            for y in 1..16 {
                v.push((format!("DSP_L_X9Y{}", (y / 2) * 5), format!("DSP48_X0Y{y}")));
            }
            v
        } else {
            Vec::new()
        }
    }
    /// Grid coordinates of a tile (for distance estimates).
    fn tile_xy(&self, tile: &str) -> Option<(i32, i32)>;
    /// A free SLICEL/SLICEM site near `(x, y)`; `used` holds taken site names.
    fn slice_near(&self, x: i32, y: i32, used: &HashSet<String>) -> Option<(String, String)>;
    /// A free `SLICEM` site near `(x, y)` (required for `SRL16E` / `SRLC32E`).
    fn slicem_near(&self, x: i32, y: i32, used: &HashSet<String>) -> Option<(String, String)> {
        for i in 0..100 {
            let site = format!("SLICE_X2Y{i}");
            if !used.contains(&site) {
                let _ = (x, y);
                return Some((format!("CLBLM_L_X2Y{i}"), site));
            }
        }
        self.slice_near(x, y, used)
    }
    /// Contiguous column of `count` free SLICE sites `(tile, site)` at constant `X` and
    /// ascending `Y..Y+count`.
    fn slice_column(&self, count: usize, used: &HashSet<String>) -> Option<Vec<(String, String)>> {
        let mut chain = Vec::with_capacity(count);
        for base_y in 0..200 {
            chain.clear();
            let mut ok = true;
            for dy in 0..count {
                let y = base_y + dy;
                let site = format!("SLICE_X0Y{y}");
                if used.contains(&site) {
                    ok = false;
                    break;
                }
                chain.push((format!("CLBLL_L_X2Y{y}"), site));
            }
            if ok {
                return Some(chain);
            }
        }
        None
    }
    /// CMT (`MMCME2_ADV` / `PLLE2_ADV`) sites.
    fn cmt_sites(&self, kind: CmtKind) -> Vec<(String, String)> {
        match kind {
            CmtKind::MmcmE2Adv => vec![
                (
                    "CMT_TOP_R_LOWER_B_X112Y52".to_string(),
                    "MMCME2_ADV_X0Y0".to_string(),
                ),
                (
                    "CMT_TOP_R_LOWER_B_X112Y104".to_string(),
                    "MMCME2_ADV_X0Y1".to_string(),
                ),
            ],
            CmtKind::PllE2Adv => vec![
                (
                    "CMT_TOP_R_UPPER_T_X112Y68".to_string(),
                    "PLLE2_ADV_X0Y0".to_string(),
                ),
                (
                    "CMT_TOP_R_UPPER_T_X112Y120".to_string(),
                    "PLLE2_ADV_X0Y1".to_string(),
                ),
            ],
        }
    }
    /// Bank number for a package pin (`"E3"` -> `"35"`).
    fn pin_bank(&self, _pin: &str) -> Option<String> {
        Some("35".to_string())
    }
    /// Claim one caller-pinned slice site (chain-head anchor). Returns the
    /// `(tile, site)` when the name is a real free slice site, else `None`;
    /// the caller fails loudly. General mechanism: tests and flows pin
    /// golden columns with it, never the placer itself.
    fn anchor_slice(&self, site: &str, used: &HashSet<String>) -> Option<(String, String)>;
    /// The free slice directly above `site` up the carry column: the slice
    /// whose CIN is driven by `site`'s COUT through the dedicated cascade
    /// wire. 7-series order, proven by the blinky golden chain (X71Y39 to
    /// Y45): ascending slice name Y, X0 before X1 within a tile. Returns
    /// `None` when the neighbor is taken or the column ends; the caller
    /// must fail loudly, since a non-adjacent cascade is unroutable.
    fn slice_above(&self, site: &str, used: &HashSet<String>) -> Option<(String, String)>;
    /// Every slice site as `(tile, site, grid x, grid y)`. Empty (the
    /// default) disables placement refinement.
    fn all_slices(&self) -> Vec<(String, String, i32, i32)> {
        Vec::new()
    }
    /// Names of the SLICEM sites (LUT RAM goes only there).
    fn slicem_sites(&self) -> HashSet<String> {
        HashSet::new()
    }
}

/// A placed LUT: tile + slice site + BEL.
#[derive(Clone, Debug)]
pub struct PlacedLut {
    pub name: String,
    pub init: u64,
    pub tile: String,
    pub site: String,
    /// `"A6LUT"` .. `"D6LUT"`.
    pub bel: String,
}

/// A placed FF.
#[derive(Clone, Debug)]
pub struct PlacedFf {
    pub name: String,
    pub tile: String,
    pub site: String,
    /// `"AFF"` .. `"DFF"`.
    pub bel: String,
}

/// A placed IOB.
#[derive(Clone, Debug)]
pub struct PlacedIob {
    pub name: String,
    pub port: String,
    pub dir: IobDir,
    pub tile: String,
    pub site: String,
}

/// A placed clock buffer (`BUFGCTRL`, `BUFG`, `BUFGCE`, `BUFIO`, `BUFR`).
#[derive(Clone, Debug)]
pub struct PlacedBufg {
    pub name: String,
    pub tile: String,
    pub site: String,
}

/// A placed `RAMB18E1` or `RAMB36E1`.
#[derive(Clone, Debug)]
pub struct PlacedBram {
    pub name: String,
    pub tile: String,
    pub site: String,
    pub read_width_a: u8,
    pub read_width_b: u8,
    pub write_width_a: u8,
    pub write_width_b: u8,
}

/// A placed `DSP48E1`.
#[derive(Clone, Debug)]
pub struct PlacedDsp {
    pub name: String,
    pub tile: String,
    pub site: String,
    pub opmode: u8,
    pub alumode: u8,
}

/// A placed `CARRY4` (in a SLICE).
#[derive(Clone, Debug)]
pub struct PlacedCarry {
    pub name: String,
    pub tile: String,
    pub site: String,
}

/// A placed `MMCME2_ADV` or `PLLE2_ADV`.
#[derive(Clone, Debug)]
pub struct PlacedCmt {
    pub name: String,
    pub kind: CmtKind,
    pub tile: String,
    pub site: String,
}

/// A placed `IDDR` (`ILOGICE2`) or `ODDR` (`OLOGICE2`).
#[derive(Clone, Debug)]
pub struct PlacedIoddr {
    pub name: String,
    pub is_input: bool,
    pub tile: String,
    pub site: String,
    pub bank: String,
}

/// A placed `ISERDESE2` or `OSERDESE2`.
#[derive(Clone, Debug)]
pub struct PlacedSerdes {
    pub name: String,
    pub is_input: bool,
    pub tile: String,
    pub site: String,
    pub bank: String,
    pub serdes_mode: String,
}

/// A placed `IDELAYE2`, `ODELAYE2`, or `IDELAYCTRL`.
#[derive(Clone, Debug)]
pub struct PlacedIodelay {
    pub name: String,
    pub kind: IodelayKind,
    pub tile: String,
    pub site: String,
    pub bank: String,
    pub iodelay_group: String,
}

/// A placed `SRL16E` or `SRLC32E` in a `SLICEM` site.
#[derive(Clone, Debug)]
pub struct PlacedSrl {
    pub name: String,
    pub kind: SrlKind,
    pub init: u32,
    pub tile: String,
    pub site: String,
    /// `"A6LUT"` .. `"D6LUT"`.
    pub bel: String,
    pub is_clk_inverted: bool,
}

/// A placed `MUXF7` or `MUXF8`.
#[derive(Clone, Debug)]
pub struct PlacedMuxf {
    pub name: String,
    pub kind: MuxfKind,
    pub tile: String,
    pub site: String,
    /// `"F7AMUX"`, `"F7BMUX"`, or `"F8MUX"`.
    pub bel: String,
}

/// A placed `GTPE2_CHANNEL`.
#[derive(Clone, Debug)]
pub struct PlacedGtpChannel {
    pub name: String,
    pub tile: String,
    pub site: String,
}

/// A placed `GTPE2_COMMON`.
#[derive(Clone, Debug)]
pub struct PlacedGtpCommon {
    pub name: String,
    pub tile: String,
    pub site: String,
}

/// A placed `IBUFDS_GTE2`.
#[derive(Clone, Debug)]
pub struct PlacedIbufdsGte2 {
    pub name: String,
    pub tile: String,
    pub site: String,
}

/// A placed `PCIE_2_1`.
#[derive(Clone, Debug)]
pub struct PlacedPcie {
    pub name: String,
    pub tile: String,
    pub site: String,
}

/// A placed `XADC`.
#[derive(Clone, Debug)]
pub struct PlacedXadc {
    pub name: String,
    pub tile: String,
    pub site: String,
}

/// A placed 7-Series configuration primitive (`BSCANE2`, `ICAPE2`, `DNA_PORT`, `FRAME_ECCE2`, `STARTUPE2`, `USR_ACCESSE2`, `CAPTUREE2`).
#[derive(Clone, Debug)]
pub struct PlacedCfgPrim {
    pub name: String,
    pub kind: crate::pack::CfgPrimKind,
    pub tile: String,
    pub site: String,
}

/// Backend-owned placed netlist for xc7a35t.
#[derive(Clone, Debug, Default)]
pub struct Placed7 {
    pub packed: Packed7,
    pub luts: Vec<PlacedLut>,
    pub ffs: Vec<PlacedFf>,
    pub iobs: Vec<PlacedIob>,
    pub bufgs: Vec<PlacedBufg>,
    pub brams: Vec<PlacedBram>,
    pub dsps: Vec<PlacedDsp>,
    pub carries: Vec<PlacedCarry>,
    pub cmts: Vec<PlacedCmt>,
    pub ioddrs: Vec<PlacedIoddr>,
    pub serdes: Vec<PlacedSerdes>,
    pub iodelays: Vec<PlacedIodelay>,
    pub srls: Vec<PlacedSrl>,
    pub lutrams: Vec<crate::lutram::PlacedLutRam>,
    pub muxfs: Vec<PlacedMuxf>,
    pub gtp_channels: Vec<PlacedGtpChannel>,
    pub gtp_commons: Vec<PlacedGtpCommon>,
    pub ibufds_gte2s: Vec<PlacedIbufdsGte2>,
    pub pcies: Vec<PlacedPcie>,
    pub xadcs: Vec<PlacedXadc>,
    pub cfg_prims: Vec<PlacedCfgPrim>,
    /// Routed tie-offs of block RAM and DSP inputs held at 0:
    /// `(net, site, site pin, is_driver)`, see [`crate::bram::tie0_endpoints`].
    pub tie_eps: Vec<(String, String, String, bool)>,
}

/// FF control set: CLK, CE, SR nets and SR mode. Slice-wide in 7-series.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CtrlSet {
    pub clk: String,
    pub ce: Option<String>,
    pub sr: Option<String>,
    pub sr_async: bool,
    /// Falling-edge clock (the slice's clock inverter).
    pub clk_inv: bool,
}

impl CtrlSet {
    pub fn of(ff: &crate::pack::PackedFf, packed: &Packed7) -> Self {
        CtrlSet {
            clk: ff.clk_net.clone(),
            ce: ff.ce_net.clone(),
            sr: ff.sr_net.clone(),
            sr_async: ff.sr_async && ff.sr_net.is_some(),
            clk_inv: packed.negedge_ffs.contains(&ff.name),
        }
    }
}

/// Where a placed FF's D input comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FfDSource {
    /// The same-letter LUT of the same slice (`xFFMUX.O6`, site-internal).
    LutO6,
    /// The same-letter carry sum bit of the same slice (`xFFMUX.XOR`).
    CarryXor,
    /// Anything else: routed through the fabric into the letter's bypass
    /// pin (`AX`..`DX`, `xFFMUX.xX`).
    Bypass,
}

/// Decide the D source of a placed FF. Route and emit both use this, so
/// the routed pin and the programmed FF mux always agree.
pub fn ff_d_source(ff: &PlacedFf, placed: &Placed7) -> FfDSource {
    let letter = ff.bel.chars().next().unwrap_or('A');
    let Some(pff) = placed.packed.ffs.iter().find(|f| f.name == ff.name) else {
        return FfDSource::Bypass;
    };
    let li = BEL_LETTERS.iter().position(|c| *c == letter).unwrap_or(0);
    for pc in &placed.carries {
        if pc.site != ff.site {
            continue;
        }
        if let Some(c) = placed.packed.carries.iter().find(|c| c.name == pc.name) {
            if c.o_nets[li].as_deref() == Some(pff.d_net.as_str()) {
                return FfDSource::CarryXor;
            }
        }
    }
    for lut in &placed.luts {
        if lut.site == ff.site && lut.bel.starts_with(letter) {
            if let Some(pl) = placed.packed.luts.iter().find(|l| l.name == lut.name) {
                if pl.output_net == pff.d_net {
                    return FfDSource::LutO6;
                }
            }
        }
    }
    FfDSource::Bypass
}

/// Order packed carries into chains by CO-to-CI net linkage (ZB-08).
///
/// Each chain runs head to tail: the head is the group whose CI net is not
/// driven by any other group CO in this design. Linkage is by net name,
/// never by cell naming. Two chains may share one CI net (one const source
/// feeding two CYINITs); a mid-chain fork or a cycle leaves a group
/// unreachable from any head and fails loudly.
fn order_carry_chains(carries: &[PackedCarry]) -> Result<Vec<Vec<&PackedCarry>>, String> {
    let by_ci: HashMap<&str, &PackedCarry> =
        carries.iter().map(|c| (c.ci_net.as_str(), c)).collect();
    let co_nets: HashSet<&str> = carries
        .iter()
        .filter_map(|c| c.co_net.as_deref())
        .collect();
    let mut chains = Vec::new();
    let mut placed: HashSet<&str> = HashSet::new();
    for head in carries.iter().filter(|c| !co_nets.contains(c.ci_net.as_str())) {
        let mut chain = Vec::new();
        let mut cur: Option<&PackedCarry> = Some(head);
        while let Some(c) = cur {
            if !placed.insert(c.name.as_str()) {
                return Err(format!("place: CARRY4 chain cycles at {}", c.name));
            }
            chain.push(c);
            cur = c.co_net.as_deref().and_then(|co| by_ci.get(co).copied());
        }
        chains.push(chain);
    }
    if placed.len() != carries.len() {
        let missing = carries
            .iter()
            .find(|c| !placed.contains(c.name.as_str()))
            .map(|c| c.name.as_str())
            .unwrap_or("?");
        return Err(format!(
            "place: CARRY4 {missing} is not reachable from any chain head (cycle?)"
        ));
    }
    Ok(chains)
}

/// BEL letters within a slice: A, B, C, D.
const BEL_LETTERS: [char; 4] = ['A', 'B', 'C', 'D'];

/// Parse `(x, y)` coordinates from a site name like `"SLICE_X12Y34"` or `"DSP48_X0Y5"`.
pub fn parse_site_xy(site: &str) -> Option<(i32, i32)> {
    let x_pos = site.rfind("_X")? + 2;
    let y_pos = site[x_pos..].find('Y').map(|i| x_pos + i)?;
    let x = site[x_pos..y_pos].parse::<i32>().ok()?;
    let y = site[y_pos + 1..].parse::<i32>().ok()?;
    Some((x, y))
}

/// Place packed cells onto device sites.
///
/// Enforces column alignment for `CARRY4`, `DSP48E1`, and `RAMB36E1` cascades,
/// `SLICEM`-only placement for `SRL16E`/`SRLC32E`, and I/O bank legality for
/// `IDDR`/`ODDR`/`ISERDESE2`/`OSERDESE2`/`IDELAYE2`/`ODELAYE2`/`IDELAYCTRL`.
pub fn place(packed: &Packed7, device: &dyn DeviceQuery) -> Result<Placed7, String> {
    place_with_pools(packed, device, (device.ramb18_sites(), device.ramb36_sites(), device.dsp_sites()))
}

/// [`place`] with the hard-block site pools given: `(RAMB18 sites, RAMB36
/// sites, DSP48E1 sites)`, each `(tile, site)` column by column (the
/// backend passes the part's full lists from
/// [`crate::device::Device::bram18_sites`] and friends).
pub fn place_with_pools(
    packed: &Packed7,
    device: &dyn DeviceQuery,
    pools: (Vec<(String, String)>, Vec<(String, String)>, Vec<(String, String)>),
) -> Result<Placed7, String> {
    let (b18, b36, dsp_pool) = pools;
    let bram_pools = (b18, b36);
    let mut out = Placed7 {
        packed: packed.clone(),
        ..Default::default()
    };
    let mut used_sites: HashSet<String> = HashSet::new();

    // 1. IOBs to package pins.
    let mut anchor: Option<(i32, i32)> = None;
    let mut net_to_iob: HashMap<String, (String, String, String)> = HashMap::new(); // net -> (tile, site, bank)
    for iob in &packed.iobs {
        let pin = iob.loc.as_deref().ok_or_else(|| {
            format!(
                "place: IOB {} (port {}) has no PACKAGE_PIN LOC",
                iob.name, iob.port
            )
        })?;
        let (tile, site) = device.iob_site_for_pin(pin).ok_or_else(|| {
            format!(
                "place: no IOB site for package pin {pin} (port {})",
                iob.port
            )
        })?;
        if anchor.is_none() {
            anchor = device.tile_xy(&tile);
        }
        let bank = device.pin_bank(pin).unwrap_or_else(|| "35".to_string());
        used_sites.insert(site.clone());
        net_to_iob.insert(iob.net.clone(), (tile.clone(), site.clone(), bank));
        out.iobs.push(PlacedIob {
            name: iob.name.clone(),
            port: iob.port.clone(),
            dir: iob.dir,
            tile,
            site,
        });
    }

    // 2. Clock buffers (BUFG / BUFGCE / BUFGCTRL / BUFIO / BUFR).
    let bufg_candidates = device.bufg_sites();
    for (idx, bufg) in packed.bufgs.iter().enumerate() {
        match bufg.kind {
            crate::pack::ClockBufKind::Bufio => {
                let bank = bufg.bank.as_deref().unwrap_or("35");
                let site = format!("BUFIO_X0Y{}", idx % 4);
                let tile = format!("HCLK_IOI3_X43Y{}", if bank == "35" { 78 } else { 26 });
                used_sites.insert(site.clone());
                out.bufgs.push(PlacedBufg {
                    name: bufg.name.clone(),
                    tile,
                    site,
                });
            }
            crate::pack::ClockBufKind::Bufr => {
                let bank = bufg.bank.as_deref().unwrap_or("35");
                let site = format!("BUFR_X0Y{}", idx % 4);
                let tile = format!("HCLK_IOI3_X43Y{}", if bank == "35" { 78 } else { 26 });
                used_sites.insert(site.clone());
                out.bufgs.push(PlacedBufg {
                    name: bufg.name.clone(),
                    tile,
                    site,
                });
            }
            _ => {
                let (tile, site) = bufg_candidates
                    .iter()
                    .find(|(_, s)| !used_sites.contains(s))
                    .cloned()
                    .or_else(|| device.bufg_site())
                    .ok_or_else(|| format!("place: no BUFGCTRL site for {}", bufg.name))?;
                used_sites.insert(site.clone());
                out.bufgs.push(PlacedBufg {
                    name: bufg.name.clone(),
                    tile,
                    site,
                });
            }
        }
    }

    // 2a. CMTs (MMCME2_ADV / PLLE2_ADV).
    for cmt in &packed.cmts {
        let candidates = device.cmt_sites(cmt.kind);
        let (tile, site) = candidates
            .into_iter()
            .find(|(_, s)| !used_sites.contains(s))
            .ok_or_else(|| format!("place: no free {:?} site for {}", cmt.kind, cmt.name))?;
        used_sites.insert(site.clone());
        out.cmts.push(PlacedCmt {
            name: cmt.name.clone(),
            kind: cmt.kind,
            tile,
            site,
        });
    }

    // 2b. Block RAMs (RAMB18E1 and RAMB36E1) with column-aligned cascade placement.
    place_brams(&packed.brams, &bram_pools, &mut used_sites, &mut out.brams)?;

    // 2c. DSP48E1s with column-aligned PCOUT->PCIN / ACOUT->ACIN / BCOUT->BCIN cascade placement.
    place_dsps(&packed.dsps, &dsp_pool, &mut used_sites, &mut out.dsps)?;

    // 2c2. Drivers shared by many block RAMs/DSPs are copied per group of
    // blocks in a column (see `replicate_hard_block_drivers`); the logic
    // below places the copies.
    let site_xy: HashMap<String, (i32, i32)> = out
        .brams
        .iter()
        .map(|b| (&b.name, &b.site))
        .chain(out.dsps.iter().map(|d| (&d.name, &d.site)))
        .filter_map(|(n, s)| parse_site_xy(s).map(|xy| (n.clone(), xy)))
        .collect();
    let mut replicated = packed.clone();
    crate::pack::replicate_hard_block_drivers(&mut replicated, &|n| site_xy.get(n).copied());
    out.packed = replicated.clone();
    let packed = &replicated;

    // 2d. CARRY4s, ordered into chains by CO-to-CI net linkage (ZB-08).
    // The first chain head honors `packed.carry_anchor` when set (a pinned
    // site for convergence measurements); every other head goes on a free
    // slice near the anchor. Every next group goes directly above its
    // predecessor so the dedicated COUT to CIN cascade wire reaches. A
    // broken column fails loudly: scattering chained groups would leave
    // the CO nets unroutable.
    let (ax, ay) = anchor.unwrap_or((0, 0));
    let mut carry_anchor = packed.carry_anchor.clone();
    // Slice grid by site-name coordinates, for adder columns.
    let slice_grid: HashMap<(i32, i32), (String, String, i32, i32)> = device
        .all_slices()
        .into_iter()
        .filter_map(|(tile, site, gx, gy)| crate::xdevice::parse_slice_xy(&site).map(|xy| (xy, (tile, site, gx, gy))))
        .collect();
    let prim = crate::pack::prim_carries(packed);
    for chain in order_carry_chains(&packed.carries)? {
        // An adder or primitive chain is a plain cascade: COUT of
        // SLICE_XxYy feeds CIN of SLICE_XxY(y+1) (x7v reads it so). It takes
        // the free column run nearest the anchor.
        let column = packed.adder_carries.contains(&chain[0].name) || prim.contains(chain[0].name.as_str());
        if column && !slice_grid.is_empty() {
            let len = chain.len() as i32;
            let mut best: Option<(i32, (i32, i32))> = None;
            for (&(x, y), (_, _, gx, gy)) in &slice_grid {
                let d = (gx - ax).abs() + (gy - ay).abs();
                if best.map(|(bd, bxy)| (d, (x, y)) >= (bd, bxy)).unwrap_or(false) {
                    continue;
                }
                let free = (0..len).all(|k| slice_grid.get(&(x, y + k)).map(|s| !used_sites.contains(&s.1)).unwrap_or(false));
                if free {
                    best = Some((d, (x, y)));
                }
            }
            let Some((_, (x, y))) = best else {
                return Err(format!("place: no free column of {len} slices for the carry chain of {}", chain[0].name));
            };
            for (k, carry) in chain.iter().enumerate() {
                let (tile, site, _, _) = slice_grid[&(x, y + k as i32)].clone();
                used_sites.insert(site.clone());
                out.carries.push(PlacedCarry { name: carry.name.clone(), tile, site });
            }
            continue;
        }
        let mut prev_site: Option<String> = None;
        for carry in chain {
            let (tile, site) = match &prev_site {
                None => match carry_anchor.take() {
                    Some(pinned) => device.anchor_slice(&pinned, &used_sites).ok_or_else(|| {
                        format!(
                            "place: carry anchor site {pinned} is not a free slice (CARRY4 {})",
                            carry.name
                        )
                    })?,
                    None => device.slice_near(ax, ay, &used_sites).ok_or_else(|| {
                        format!("place: no SLICE site for CARRY4 {}", carry.name)
                    })?,
                },
                Some(prev) => device.slice_above(prev, &used_sites).ok_or_else(|| {
                    format!(
                        "place: no free slice above {} for chained CARRY4 {}",
                        prev, carry.name
                    )
                })?,
            };
            used_sites.insert(site.clone());
            prev_site = Some(site.clone());
            out.carries.push(PlacedCarry {
                name: carry.name.clone(),
                tile,
                site,
            });
        }
    }

    // 2d2. Carry co-placement slots (ZB-04): each used carry bit reserves
    // its BEL letter in its slice; the O-driven FF and the S-driving LUT
    // of bit j take letter j in the carry slice, so S/DI/O stay
    // site-internal. Unused letters of partial groups stay open for
    // generic fill. LUT and FF BELs are distinct, so slots track by class.
    let packed_carry_by_name: HashMap<&str, &PackedCarry> = packed
        .carries
        .iter()
        .map(|c| (c.name.as_str(), c))
        .collect();
    // BEL occupancy per site: site -> letters used.
    let mut site_bels: HashMap<String, HashSet<char>> = HashMap::new();
    // Allocate a (tile, site, BEL-letter), reusing a slice with free BELs
    // before opening a new one.
    let mut site_tile: HashMap<String, String> = HashMap::new();
    let mut o_slot: HashMap<&str, (String, String, char)> = HashMap::new();
    let mut s_slot: HashMap<&str, (String, String, char)> = HashMap::new();
    for pc in &out.carries {
        let Some(packed_c) = packed_carry_by_name.get(pc.name.as_str()) else {
            continue;
        };
        site_bels.entry(pc.site.clone()).or_default();
        site_tile.insert(pc.site.clone(), pc.tile.clone());
        for j in 0..4 {
            let letter = BEL_LETTERS[j];
            if packed_c.o_nets[j].is_some() || packed_c.s_nets[j].is_some() {
                site_bels.get_mut(&pc.site).unwrap().insert(letter);
            }
            if let Some(o) = &packed_c.o_nets[j] {
                o_slot.insert(o.as_str(), (pc.tile.clone(), pc.site.clone(), letter));
            }
            if let Some(s) = &packed_c.s_nets[j] {
                s_slot.insert(s.as_str(), (pc.tile.clone(), pc.site.clone(), letter));
            }
        }
    }

    // 2e. I/O logic primitives: IDELAYE2 / ODELAYE2 / IDELAYCTRL, IDDR / ODDR, ISERDESE2 / OSERDESE2.
    place_io_primitives(packed, device, &net_to_iob, &mut used_sites, &mut out)?;

    // 2f. SRL16E / SRLC32E in SLICEM sites (with Q31->D cascade co-location).
    place_srls(
        packed,
        device,
        (ax, ay),
        &mut used_sites,
        &mut site_bels,
        &mut site_tile,
        &mut out.srls,
    )?;

    // 2f2. LUT RAM, a SLICEM each (refined with the logic below).
    out.lutrams = crate::lutram::place(packed, device, (ax, ay), &mut used_sites)?;

    // 2g. GTPE2_COMMON, GTPE2_CHANNEL, IBUFDS_GTE2, PCIE_2_1, XADC, and CFG_CENTER_MID primitives.
    for (i, gtc) in packed.gtp_commons.iter().enumerate() {
        if i >= 2 {
            return Err(format!("place: out of GTPE2_COMMON sites for {}", gtc.name));
        }
        out.gtp_commons.push(PlacedGtpCommon {
            name: gtc.name.clone(),
            tile: format!("GTP_COMMON_X113Y{}", 26 + i * 52),
            site: format!("GTPE2_COMMON_X0Y{i}"),
        });
    }
    for (i, gch) in packed.gtp_channels.iter().enumerate() {
        if i >= 4 {
            return Err(format!(
                "place: out of GTPE2_CHANNEL sites for {}",
                gch.name
            ));
        }
        out.gtp_channels.push(PlacedGtpChannel {
            name: gch.name.clone(),
            tile: format!("GTP_CHANNEL_{}_X113Y{}", i % 4, 10 + i * 12),
            site: format!("GTPE2_CHANNEL_X0Y{i}"),
        });
    }
    for (i, ibuf) in packed.ibufds_gte2s.iter().enumerate() {
        if i >= 2 {
            return Err(format!(
                "place: out of IBUFDS_GTE2 sites for {}",
                ibuf.name
            ));
        }
        out.ibufds_gte2s.push(PlacedIbufdsGte2 {
            name: ibuf.name.clone(),
            tile: "GTP_COMMON_X113Y26".to_string(),
            site: format!("IBUFDS_GTE2_X0Y{i}"),
        });
    }
    for (i, pcie) in packed.pcies.iter().enumerate() {
        if i >= 1 {
            return Err(format!("place: out of PCIE_2_1 sites for {}", pcie.name));
        }
        out.pcies.push(PlacedPcie {
            name: pcie.name.clone(),
            tile: "PCIE_BOT_X113Y25".to_string(),
            site: "PCIE_2_1_X0Y0".to_string(),
        });
    }
    for (i, xadc) in packed.xadcs.iter().enumerate() {
        if i >= 1 {
            return Err(format!("place: out of XADC sites for {}", xadc.name));
        }
        out.xadcs.push(PlacedXadc {
            name: xadc.name.clone(),
            tile: "CFG_CENTER_MID_X51Y76".to_string(),
            site: "XADC_X0Y0".to_string(),
        });
    }
    for cfg in &packed.cfg_prims {
        let site = match cfg.kind {
            crate::pack::CfgPrimKind::Bscane2 => {
                format!("BSCAN_X0Y{}", cfg.jtag_chain.saturating_sub(1))
            }
            crate::pack::CfgPrimKind::Icape2 => "ICAP_X0Y0".to_string(),
            crate::pack::CfgPrimKind::DnaPort => "DNA_PORT_X0Y0".to_string(),
            crate::pack::CfgPrimKind::FrameEcce2 => "FRAME_ECC_X0Y0".to_string(),
            crate::pack::CfgPrimKind::Startupe2 => "STARTUP_X0Y0".to_string(),
            crate::pack::CfgPrimKind::UsrAccesse2 => "USR_ACCESS_X0Y0".to_string(),
            crate::pack::CfgPrimKind::Capturee2 => "CAPTURE_X0Y0".to_string(),
        };
        if !used_sites.insert(site.clone()) {
            return Err(format!(
                "place: configuration site {site} already occupied (cell {})",
                cfg.name
            ));
        }
        out.cfg_prims.push(PlacedCfgPrim {
            name: cfg.name.clone(),
            kind: cfg.kind,
            tile: "CFG_CENTER_MID_X51Y76".to_string(),
            site,
        });
    }
    let mut carry_lut_slots: HashSet<(String, char)> = HashSet::new();
    let mut carry_ff_slots: HashSet<(String, char)> = HashSet::new();
    let adder_sites: HashSet<String> = out.carries.iter().filter(|c| packed.adder_carries.contains(&c.name)).map(|c| c.site.clone()).collect();

    // 3. Pair FFs with their driving LUTs (FF.D == LUT.O) into one slice.
    let lut_by_output: HashMap<&str, usize> = packed
        .luts
        .iter()
        .enumerate()
        .map(|(i, l)| (l.output_net.as_str(), i))
        .collect();
    let mut lut_placed: HashSet<usize> = HashSet::new();

    // Allocate a (tile, site, BEL-letter), reusing a slice with free BELs
    // before opening a new one. (site_bels/site_tile were declared with
    // the carry slots above so carry slices join the same occupancy map.)
    // Foreign logic stays out of chain slices: the oracle chain tiles
    // hold chain logic only, and packing strays into transit gaps crowded
    // the head INT until the const-1 route failed loudly. Reuse passes run
    // in sorted site order (HashMap iteration is per-process random, and
    // unsorted reuse made bitstreams nondeterministic run to run).
    let carry_sites: HashSet<String> =
        out.carries.iter().map(|c| c.site.clone()).collect();
    // Fresh-slice search center: the chain head tile when carries exist
    // (foreign logic in a carry design is carry support, e.g. the CYINIT
    // const-1, and colocating it keeps its route short; the IOB anchor
    // once flung it 70 columns west past sparse fabric until its head-CI
    // route failed loudly), else the IOB anchor as before.
    let (fx, fy) = out
        .carries
        .first()
        .and_then(|c| device.tile_xy(&c.tile))
        .unwrap_or((ax, ay));
    // Control set per slice: every FF in a slice shares CLK, CE, SR and
    // the SR mode (FFSYNC is slice-wide), so an FF only joins a slice whose
    // FFs have the same set. LUT-only slices have none yet.
    let mut site_ctrl: HashMap<String, CtrlSet> = HashMap::new();
    let alloc = |used: &mut HashSet<String>,
                     site_bels: &mut HashMap<String, HashSet<char>>,
                     site_tile: &mut HashMap<String, String>,
                     site_ctrl: &mut HashMap<String, CtrlSet>,
                     ctrl: Option<&CtrlSet>,
                     device: &dyn DeviceQuery|
     -> Result<(String, String, char), String> {
        let mut sites: Vec<&String> = site_bels.keys().collect();
        sites.sort();
        let fits = |site: &str| match (ctrl, site_ctrl.get(site)) {
            (Some(c), Some(have)) => c == have,
            _ => true,
        };
        // Pass 1: open slices without carry cells, in sorted order.
        for site in sites.iter() {
            if carry_sites.contains(*site) || !fits(site) {
                continue;
            }
            let bels = &site_bels[*site];
            if bels.len() < 4 {
                if let Some(letter) = BEL_LETTERS.iter().find(|c| !bels.contains(c)) {
                    let tile = site_tile.get(*site).cloned().unwrap_or_default();
                    if let Some(c) = ctrl {
                        site_ctrl.insert((*site).clone(), c.clone());
                    }
                    return Ok((tile, (*site).clone(), *letter));
                }
            }
        }
        // Pass 2: a fresh slice near the search center, rather than
        // spilling foreign logic into carry-slice gaps.
        if let Some((tile, site)) = device.slice_near(fx, fy, used) {
            used.insert(site.clone());
            site_tile.insert(site.clone(), tile.clone());
            site_bels.entry(site.clone()).or_default();
            if let Some(c) = ctrl {
                site_ctrl.insert(site.clone(), c.clone());
            }
            return Ok((tile, site, 'A'));
        }
        // Pass 3 (last resort, dense packing with no fresh slice): reuse
        // a carry-slice gap in sorted order.
        for site in sites.iter() {
            if !fits(site) {
                continue;
            }
            let bels = &site_bels[*site];
            if bels.len() < 4 {
                if let Some(letter) = BEL_LETTERS.iter().find(|c| !bels.contains(c)) {
                    let tile = site_tile.get(*site).cloned().unwrap_or_default();
                    if let Some(c) = ctrl {
                        site_ctrl.insert((*site).clone(), c.clone());
                    }
                    return Ok((tile, (*site).clone(), *letter));
                }
            }
        }
        Err("place: no free SLICEL site".to_string())
    };

    // A fresh slice that nothing else joins (all its BEL letters taken).
    //
    // `pref` is the point to search from. Replicated hard-block drivers
    // (`*_rep{g}`, from `pack::replicate_hard_block_drivers`) pass the
    // centroid of the RAMs/DSPs their group feeds, so a copy lands next to the
    // blocks it drives rather than at the global fresh-slice anchor (G60: a
    // copy 128 tiles from its RAMs). Everything else keeps the old anchor.
    let lone = |pref: Option<(i32, i32)>,
                used: &mut HashSet<String>,
                site_bels: &mut HashMap<String, HashSet<char>>,
                site_tile: &mut HashMap<String, String>|
     -> Result<(String, String, char), String> {
        let (px, py) = pref.unwrap_or((fx, fy));
        let (tile, site) = device.slice_near(px, py, used).ok_or("place: no free SLICEL site")?;
        used.insert(site.clone());
        site_tile.insert(site.clone(), tile.clone());
        site_bels.insert(site.clone(), BEL_LETTERS.iter().copied().collect());
        Ok((tile, site, 'A'))
    };

    // Where to put a replicated driver copy: next to the blocks of its group.
    //
    // A group is the RAMs/DSPs sharing one column that a single copy drives,
    // so the target is one of its own blocks, not the group centroid: a
    // centroid pulls a copy between two columns (G60 measured 137 tiles from
    // the farthest RAM). We pick the group's block in the *median* row of its
    // dominant column — the copy then sits inside the span it drives, which is
    // what the <= 20 tiles guard measures. `slice_near` searches the whole die,
    // so a wrong column is not corrected later.
    let group_centre: HashMap<usize, (i32, i32)> = {
        let site_of: HashMap<&str, (i32, i32)> = out
            .brams
            .iter()
            .map(|b| (b.name.as_str(), &b.tile))
            .chain(out.dsps.iter().map(|d| (d.name.as_str(), &d.tile)))
            .filter_map(|(n, t)| device.tile_xy(t).map(|xy| (n, xy)))
            .collect();
        packed
            .copy_groups
            .iter()
            .filter_map(|(g, members)| {
                let mut pts: Vec<(i32, i32)> =
                    members.iter().filter_map(|m| site_of.get(m.as_str()).copied()).collect();
                if pts.is_empty() {
                    return None;
                }
                // Dominant column = the one with the most members (ties: lower x).
                let mut by_x: HashMap<i32, usize> = HashMap::new();
                for (x, _) in &pts {
                    *by_x.entry(*x).or_insert(0) += 1;
                }
                let col = *by_x.iter().max_by_key(|(x, n)| (**n, -*x)).map(|(x, _)| x)?;
                pts.retain(|(x, _)| *x == col);
                pts.sort_by_key(|(_, y)| *y);
                // Median row of that column: a copy here bounds the worst
                // distance to the group's ends.
                let (x, y) = pts[pts.len() / 2];
                Some((*g, (x, y)))
            })
            .collect()
    };

    // Place LUTs feeding MUXF7 / MUXF8 first so wide muxes share one slice.
    place_muxf_trees(
        packed,
        device,
        (ax, ay),
        &lut_by_output,
        &mut lut_placed,
        &mut used_sites,
        &mut site_bels,
        &mut site_tile,
        &mut out.luts,
        &mut out.muxfs,
    )?;

    for (fi, ff) in packed.ffs.iter().enumerate() {
        let _ = fi;
        // Carry-driven FF: same slice and letter as its carry bit, so the
        // O-to-D path stays site-internal. A double-booked slot fails loud.
        let ctrl = CtrlSet::of(ff, packed);
        // A sum bit of an adder may also be registered by a flop that
        // cannot share the slice (another control set, or a second flop on
        // the same bit): that flop is placed like any other and takes the
        // sum through the fabric.
        let adder_clash = o_slot.get(ff.d_net.as_str()).is_some_and(|(_, site, letter)| {
            adder_sites.contains(site.as_str())
                && (carry_ff_slots.contains(&(site.clone(), *letter)) || site_ctrl.get(site).is_some_and(|have| *have != ctrl))
        });
        if let Some((tile, site, letter)) = o_slot.get(ff.d_net.as_str()).filter(|_| !adder_clash) {
            if !carry_ff_slots.insert((site.clone(), *letter)) {
                return Err(format!(
                    "place: FF {} collides on {site}.{letter}FF in carry slice",
                    ff.name
                ));
            }
            match site_ctrl.get(site) {
                Some(have) if *have != ctrl => {
                    return Err(format!("place: FF {} needs a different clock/CE/SR than the other FFs of carry slice {site}", ff.name));
                }
                _ => {
                    site_ctrl.insert(site.clone(), ctrl.clone());
                }
            }
            out.ffs.push(PlacedFf {
                name: ff.name.clone(),
                tile: tile.clone(),
                site: site.clone(),
                bel: format!("{letter}FF"),
            });
            continue;
        }
        // Find driving LUT, if any, and not yet placed.
        let lut_idx = lut_by_output.get(ff.d_net.as_str()).copied().filter(|i| !lut_placed.contains(i));
        let (tile, site, letter) = if packed.lone_cells.contains(&ff.name) {
            let pref = crate::xdevice::rep_group(&ff.name).and_then(|g| group_centre.get(&g).copied());
            lone(pref, &mut used_sites, &mut site_bels, &mut site_tile)?
        } else {
            alloc(&mut used_sites, &mut site_bels, &mut site_tile, &mut site_ctrl, Some(&ctrl), device)?
        };
        site_bels.get_mut(&site).unwrap().insert(letter);
        if let Some(li) = lut_idx {
            lut_placed.insert(li);
            let lut = &packed.luts[li];
            out.luts.push(PlacedLut {
                name: lut.name.clone(),
                init: lut.init,
                tile: tile.clone(),
                site: site.clone(),
                bel: format!("{letter}6LUT"),
            });
        }
        out.ffs.push(PlacedFf {
            name: ff.name.clone(),
            tile,
            site,
            bel: format!("{letter}FF"),
        });
    }

    // Remaining unpaired LUTs.
    for (li, lut) in packed.luts.iter().enumerate() {
        if lut_placed.contains(&li) {
            continue;
        }
        // S-driving LUT: same slice and letter as its carry bit, so the
        // LUT-O-to-S path stays site-internal. Shares the letter with the
        // bit FF (LUT and FF BELs are distinct); two LUTs on one slot fail.
        if let Some((tile, site, letter)) = s_slot.get(lut.output_net.as_str()) {
            if !carry_lut_slots.insert((site.clone(), *letter)) {
                return Err(format!(
                    "place: LUT {} collides on {site}.{letter}6LUT in carry slice",
                    lut.name
                ));
            }
            out.luts.push(PlacedLut {
                name: lut.name.clone(),
                init: lut.init,
                tile: tile.clone(),
                site: site.clone(),
                bel: format!("{letter}6LUT"),
            });
            continue;
        }
        let (tile, site, letter) = if packed.lone_cells.contains(&lut.name) {
            let pref = crate::xdevice::rep_group(&lut.name).and_then(|g| group_centre.get(&g).copied());
            lone(pref, &mut used_sites, &mut site_bels, &mut site_tile)?
        } else {
            alloc(&mut used_sites, &mut site_bels, &mut site_tile, &mut site_ctrl, None, device)?
        };
        site_bels.get_mut(&site).unwrap().insert(letter);
        out.luts.push(PlacedLut {
            name: lut.name.clone(),
            init: lut.init,
            tile,
            site,
            bel: format!("{letter}6LUT"),
        });
    }

    crate::place_sa::refine(&mut out, device);
    // Seat replicated hard-block driver copies back inside their own group's
    // column. The annealer's cost is a per-net bounding box, and a copy plus a
    // column of RAMs already spans ~20 rows, so the copy's own x has almost no
    // gradient and it drifts (G60: 97-137 tiles out). A copy is a lone cell in
    // a slice of its own, so moving it after annealing cannot displace anything.
    reseat_hard_block_copies(&mut out, device);
    Ok(out)
}

/// Move each `*_rep{g}` lone cell to the free slice nearest its group's blocks,
/// preferring the group's dominant column. Run after annealing.
fn reseat_hard_block_copies(out: &mut Placed7, device: &dyn DeviceQuery) {
    let groups = out.packed.copy_groups.clone();
    if groups.is_empty() {
        return;
    }
    // Group target: median row of the group's dominant column.
    let ram_xy: HashMap<&str, (i32, i32)> = out
        .brams
        .iter()
        .map(|b| (b.name.as_str(), &b.tile))
        .chain(out.dsps.iter().map(|d| (d.name.as_str(), &d.tile)))
        .filter_map(|(n, t)| device.tile_xy(t).map(|xy| (n, xy)))
        .collect();
    let mut target: HashMap<usize, (i32, i32)> = HashMap::new();
    for (g, members) in &groups {
        let mut pts: Vec<(i32, i32)> =
            members.iter().filter_map(|m| ram_xy.get(m.as_str()).copied()).collect();
        if pts.is_empty() {
            continue;
        }
        let mut by_x: HashMap<i32, usize> = HashMap::new();
        for (x, _) in &pts {
            *by_x.entry(*x).or_insert(0) += 1;
        }
        let Some(col) = by_x.iter().max_by_key(|(x, n)| (**n, -*x)).map(|(x, _)| *x) else { continue };
        pts.retain(|(x, _)| *x == col);
        pts.sort_by_key(|(_, y)| *y);
        target.insert(*g, pts[pts.len() / 2]);
    }
    if target.is_empty() {
        return;
    }
    // Sites held by everything that must not move.
    let mut taken: HashSet<String> = out
        .luts
        .iter()
        .map(|l| l.site.clone())
        .chain(out.ffs.iter().map(|f| f.site.clone()))
        .chain(out.brams.iter().map(|b| b.site.clone()))
        .chain(out.dsps.iter().map(|d| d.site.clone()))
        .chain(out.carries.iter().map(|c| c.site.clone()))
        .chain(out.srls.iter().map(|s| s.site.clone()))
        .chain(out.lutrams.iter().map(|r| r.site.clone()))
        .chain(out.muxfs.iter().map(|m| m.site.clone()))
        .collect();
    // Lone copies own a whole slice, so release their current sites first.
    let lone: HashSet<String> = out.packed.lone_cells.clone();
    for f in out.ffs.iter().filter(|f| lone.contains(&f.name)) {
        taken.remove(&f.site);
    }
    for l in out.luts.iter().filter(|l| lone.contains(&l.name)) {
        taken.remove(&l.site);
    }
    let seat = |name: &str, taken: &mut HashSet<String>| -> Option<(String, String)> {
        let g = crate::xdevice::rep_group(name)?;
        let (tx, ty) = *target.get(&g)?;
        let (tile, site) = device.slice_near(tx, ty, taken)?;
        taken.insert(site.clone());
        Some((tile, site))
    };
    for f in out.ffs.iter_mut() {
        if !lone.contains(&f.name) {
            continue;
        }
        if let Some((tile, site)) = seat(&f.name, &mut taken) {
            f.tile = tile;
            f.site = site;
        }
    }
    for l in out.luts.iter_mut() {
        if !lone.contains(&l.name) {
            continue;
        }
        if let Some((tile, site)) = seat(&l.name, &mut taken) {
            l.tile = tile;
            l.site = site;
        }
    }
}

fn place_brams(
    brams: &[PackedBram],
    pools: &(Vec<(String, String)>, Vec<(String, String)>),
    used_sites: &mut HashSet<String>,
    out_brams: &mut Vec<PlacedBram>,
) -> Result<(), String> {
    if brams.is_empty() {
        return Ok(());
    }
    let (ramb18_pool, ramb36_pool) = pools;

    // Build cascade chains among BRAMs via CASCADEOUTA->CASCADEINA or CASCADEOUTB->CASCADEINB.
    let mut out_a_map: HashMap<&str, usize> = HashMap::new();
    let mut out_b_map: HashMap<&str, usize> = HashMap::new();
    for (i, b) in brams.iter().enumerate() {
        if let Some(na) = &b.cascade_out_a {
            out_a_map.insert(na.as_str(), i);
        }
        if let Some(nb) = &b.cascade_out_b {
            out_b_map.insert(nb.as_str(), i);
        }
    }
    let mut prev_of = vec![None; brams.len()];
    let mut next_of = vec![None; brams.len()];
    for (i, b) in brams.iter().enumerate() {
        let pred = b
            .cascade_in_a
            .as_deref()
            .and_then(|n| out_a_map.get(n).copied())
            .or_else(|| {
                b.cascade_in_b
                    .as_deref()
                    .and_then(|n| out_b_map.get(n).copied())
            });
        if let Some(p) = pred {
            prev_of[i] = Some(p);
            next_of[p] = Some(i);
        }
    }

    let mut placed_idx: HashMap<usize, PlacedBram> = HashMap::new();
    for start in 0..brams.len() {
        if prev_of[start].is_some() || placed_idx.contains_key(&start) {
            continue;
        }
        let mut chain = vec![start];
        let mut cur = start;
        while let Some(nxt) = next_of[cur] {
            if chain.contains(&nxt) {
                return Err(format!(
                    "place: cyclic BRAM cascade involving {}",
                    brams[nxt].name
                ));
            }
            chain.push(nxt);
            cur = nxt;
        }

        let is_36 = brams[start].kind == BramKind::Ramb36e1;
        let pool = if is_36 { ramb36_pool } else { ramb18_pool };
        let window = find_column_window(pool, chain.len(), used_sites).ok_or_else(|| {
            format!(
                "place: no contiguous {:?} column of length {} for {}",
                brams[start].kind,
                chain.len(),
                brams[start].name
            )
        })?;
        for (&bi, (tile, site)) in chain.iter().zip(window.into_iter()) {
            used_sites.insert(site.clone());
            // A RAMB36_X<x>Y<y> occupies RAMB18_X<x>Y<2y> and RAMB18_X<x>Y<2y+1>.
            if let Some((x, y)) = parse_site_xy(&site) {
                if site.starts_with("RAMB36_") {
                    used_sites.insert(format!("RAMB18_X{x}Y{}", y * 2));
                    used_sites.insert(format!("RAMB18_X{x}Y{}", y * 2 + 1));
                } else if site.starts_with("RAMB18_") {
                    used_sites.insert(format!("RAMB36_X{x}Y{}", y / 2));
                }
            }
            let b = &brams[bi];
            placed_idx.insert(
                bi,
                PlacedBram {
                    name: b.name.clone(),
                    tile,
                    site,
                    read_width_a: b.read_width_a,
                    read_width_b: b.read_width_b,
                    write_width_a: b.write_width_a,
                    write_width_b: b.write_width_b,
                },
            );
        }
    }

    for i in 0..brams.len() {
        if let Some(pb) = placed_idx.remove(&i) {
            out_brams.push(pb);
        }
    }
    Ok(())
}

fn place_dsps(
    dsps: &[PackedDsp],
    pool: &[(String, String)],
    used_sites: &mut HashSet<String>,
    out_dsps: &mut Vec<PlacedDsp>,
) -> Result<(), String> {
    if dsps.is_empty() {
        return Ok(());
    }
    let mut pcout_map: HashMap<&str, usize> = HashMap::new();
    let mut acout_map: HashMap<&str, usize> = HashMap::new();
    let mut bcout_map: HashMap<&str, usize> = HashMap::new();
    let mut ccout_map: HashMap<&str, usize> = HashMap::new();
    for (i, d) in dsps.iter().enumerate() {
        if let Some(n) = &d.pcout_net {
            pcout_map.insert(n.as_str(), i);
        }
        if let Some(n) = &d.acout_net {
            acout_map.insert(n.as_str(), i);
        }
        if let Some(n) = &d.bcout_net {
            bcout_map.insert(n.as_str(), i);
        }
        if let Some(n) = &d.carrycascout_net {
            ccout_map.insert(n.as_str(), i);
        }
    }

    let mut prev_of = vec![None; dsps.len()];
    let mut next_of = vec![None; dsps.len()];
    for (i, d) in dsps.iter().enumerate() {
        let pred = d
            .pcin_net
            .as_deref()
            .and_then(|n| pcout_map.get(n).copied())
            .or_else(|| d.acin_net.as_deref().and_then(|n| acout_map.get(n).copied()))
            .or_else(|| d.bcin_net.as_deref().and_then(|n| bcout_map.get(n).copied()))
            .or_else(|| {
                d.carrycascin_net
                    .as_deref()
                    .and_then(|n| ccout_map.get(n).copied())
            });
        if let Some(p) = pred {
            prev_of[i] = Some(p);
            next_of[p] = Some(i);
        }
    }

    let mut placed_idx: HashMap<usize, PlacedDsp> = HashMap::new();
    for start in 0..dsps.len() {
        if prev_of[start].is_some() || placed_idx.contains_key(&start) {
            continue;
        }
        let mut chain = vec![start];
        let mut cur = start;
        while let Some(nxt) = next_of[cur] {
            if chain.contains(&nxt) {
                return Err(format!(
                    "place: cyclic DSP48E1 cascade involving {}",
                    dsps[nxt].name
                ));
            }
            chain.push(nxt);
            cur = nxt;
        }
        let window = find_column_window(pool, chain.len(), used_sites).ok_or_else(|| {
            format!(
                "place: no contiguous DSP48E1 column of length {} for {}",
                chain.len(),
                dsps[start].name
            )
        })?;
        for (&di, (tile, site)) in chain.iter().zip(window.into_iter()) {
            used_sites.insert(site.clone());
            let d = &dsps[di];
            placed_idx.insert(
                di,
                PlacedDsp {
                    name: d.name.clone(),
                    tile,
                    site,
                    opmode: d.opmode,
                    alumode: d.alumode,
                },
            );
        }
    }

    for i in 0..dsps.len() {
        if let Some(pd) = placed_idx.remove(&i) {
            out_dsps.push(pd);
        }
    }
    Ok(())
}

/// Find a contiguous vertical sequence of `len` free sites in `pool` having the
/// same `X` coordinate and consecutive `Y` coordinates (`y, y+1, ...`).
///
/// The search is in a fixed order — columns by ascending `X`, rows by ascending
/// `Y` — so the same netlist always gets the same sites (contract R2). `len ==
/// 1` used to return the first free entry in pool order with no column
/// reasoning, which both ignored the column ordering the rest of this function
/// defines and made the choice depend on pool construction order (G9/D2).
fn find_column_window(
    pool: &[(String, String)],
    len: usize,
    used: &HashSet<String>,
) -> Option<Vec<(String, String)>> {
    if len == 0 {
        return Some(Vec::new());
    }
    let mut by_col: HashMap<i32, Vec<(i32, String, String)>> = HashMap::new();
    // Sites whose name has no (X, Y): usable only by a length-1 request, and
    // only after every addressable site, so they never displace real columns.
    let mut unplaced: Vec<(String, String)> = Vec::new();
    for (tile, site) in pool {
        if used.contains(site) {
            continue;
        }
        match parse_site_xy(site) {
            Some((x, y)) => by_col.entry(x).or_default().push((y, tile.clone(), site.clone())),
            None => unplaced.push((tile.clone(), site.clone())),
        }
    }
    let mut cols: Vec<i32> = by_col.keys().copied().collect();
    cols.sort();
    for col in cols {
        let Some(entries) = by_col.get_mut(&col) else { continue };
        entries.sort_by_key(|(y, _, _)| *y);
        if entries.len() < len {
            continue;
        }
        for w in entries.windows(len) {
            let contiguous = w.windows(2).all(|pair| pair[1].0 == pair[0].0 + 1);
            if contiguous {
                return Some(
                    w.iter()
                        .map(|(_, t, s)| (t.clone(), s.clone()))
                        .collect(),
                );
            }
        }
    }
    // Nothing addressable fit; a single request may still take a named site.
    if len == 1 {
        if let Some(p) = unplaced.into_iter().next() {
            return Some(vec![p]);
        }
    }
    None
}

fn place_io_primitives(
    packed: &Packed7,
    device: &dyn DeviceQuery,
    net_to_iob: &HashMap<String, (String, String, String)>,
    used_sites: &mut HashSet<String>,
    out: &mut Placed7,
) -> Result<(), String> {
    // Track bank assignments per IODELAY_GROUP so IDELAYE2/ODELAYE2 and IDELAYCTRL match banks.
    let mut group_bank: HashMap<String, String> = HashMap::new();
    let mut next_io_y = 0i32;

    let resolve_io_coord = |loc: Option<&str>,
                            bank_hint: Option<&str>,
                            pad_net: Option<&str>,
                            next_y: &mut i32|
     -> Result<(String, i32, i32), String> {
        if let Some(pin) = loc {
            if let Some((tile, site)) = device.iob_site_for_pin(pin) {
                let bank = device.pin_bank(pin).unwrap_or_else(|| "35".to_string());
                let (x, y) = parse_site_xy(&site).unwrap_or((1, 0));
                let _ = tile;
                return Ok((bank, x, y));
            }
            return Err(format!("place: unknown package pin `{pin}` on I/O primitive"));
        }
        if let Some(net) = pad_net {
            if let Some((_, site, bank)) = net_to_iob.get(net) {
                let (x, y) = parse_site_xy(site).unwrap_or((1, 0));
                return Ok((bank.clone(), x, y));
            }
        }
        let bank = bank_hint.unwrap_or("35").to_string();
        let y = *next_y;
        *next_y += 2;
        Ok((bank, 1, y))
    };

    // 1. Place IDELAYE2 / ODELAYE2 first so we know their banks for IDELAYCTRL.
    for d in &packed.iodelays {
        if d.kind == IodelayKind::IdelayCtrl {
            continue;
        }
        let (bank, x, y) = resolve_io_coord(
            d.loc.as_deref(),
            d.bank.as_deref(),
            d.data_in_net.as_deref(),
            &mut next_io_y,
        )?;
        if let Some(prev_bank) = group_bank.get(&d.iodelay_group) {
            if prev_bank != &bank {
                return Err(format!(
                    "place: IODELAY_GROUP `{}` spans multiple I/O banks ({prev_bank} and {bank})",
                    d.iodelay_group
                ));
            }
        } else {
            group_bank.insert(d.iodelay_group.clone(), bank.clone());
        }
        let tile = format!("LIOI3_X0Y{}", (y / 2) * 2);
        let site = match d.kind {
            IodelayKind::IdelayE2 => format!("IDELAY_X{x}Y{y}"),
            IodelayKind::OdelayE2 => format!("ODELAY_X{x}Y{y}"),
            IodelayKind::IdelayCtrl => unreachable!(),
        };
        used_sites.insert(site.clone());
        out.iodelays.push(PlacedIodelay {
            name: d.name.clone(),
            kind: d.kind,
            tile,
            site,
            bank,
            iodelay_group: d.iodelay_group.clone(),
        });
    }

    // 2. Place IDELAYCTRLs in matching bank HCLK_IOI3 tiles.
    for (ci, d) in packed
        .iodelays
        .iter()
        .filter(|d| d.kind == IodelayKind::IdelayCtrl)
        .enumerate()
    {
        let bank = d
            .bank
            .clone()
            .or_else(|| group_bank.get(&d.iodelay_group).cloned())
            .unwrap_or_else(|| "35".to_string());
        if let Some(gb) = group_bank.get(&d.iodelay_group) {
            if gb != &bank {
                return Err(format!(
                    "place: IDELAYCTRL {} in bank {bank} conflicts with IODELAY_GROUP `{}` in bank {gb}",
                    d.name, d.iodelay_group
                ));
            }
        }
        let tile = format!("HCLK_IOI3_X43Y{}", 26 + (ci as i32) * 52);
        let site = format!("IDELAYCTRL_X0Y{ci}");
        used_sites.insert(site.clone());
        out.iodelays.push(PlacedIodelay {
            name: d.name.clone(),
            kind: d.kind,
            tile,
            site,
            bank,
            iodelay_group: d.iodelay_group.clone(),
        });
    }

    // 3. Place IDDR / ODDR into ILOGICE2 / OLOGICE2 sites.
    for ddr in &packed.ioddrs {
        let pad_net = if ddr.is_input {
            ddr.d_net.as_deref()
        } else {
            ddr.q_net.as_deref()
        };
        let (bank, x, mut y) = resolve_io_coord(
            ddr.loc.as_deref(),
            ddr.bank.as_deref(),
            pad_net,
            &mut next_io_y,
        )?;
        let prefix = if ddr.is_input { "ILOGIC" } else { "OLOGIC" };
        while used_sites.contains(&format!("{prefix}_X{x}Y{y}")) {
            y += 1;
        }
        let tile = format!("LIOI3_X0Y{}", (y / 2) * 2);
        let site = format!("{prefix}_X{x}Y{y}");
        used_sites.insert(site.clone());
        out.ioddrs.push(PlacedIoddr {
            name: ddr.name.clone(),
            is_input: ddr.is_input,
            tile,
            site,
            bank,
        });
    }

    // 4. Place ISERDESE2 / OSERDESE2, enforcing Master/Slave pair co-location in the same IOI3 tile.
    let mut shift_out_map: HashMap<&str, usize> = HashMap::new();
    for (i, s) in packed.serdes.iter().enumerate() {
        if let Some(n) = &s.shiftout1_net {
            shift_out_map.insert(n.as_str(), i);
        }
        if let Some(n) = &s.shiftout2_net {
            shift_out_map.insert(n.as_str(), i);
        }
    }
    let mut master_coords: HashMap<usize, (String, i32, i32)> = HashMap::new();
    let mut placed_serdes: HashMap<usize, PlacedSerdes> = HashMap::new();

    // First pass: place MASTER serdes.
    for (i, s) in packed.serdes.iter().enumerate() {
        if s.serdes_mode == "SLAVE" {
            continue;
        }
        let (bank, x, mut y) = resolve_io_coord(
            s.loc.as_deref(),
            s.bank.as_deref(),
            s.serial_net.as_deref(),
            &mut next_io_y,
        )?;
        // Master occupies even Y in IOI3 tile (`Y0` of the pair).
        y = (y / 2) * 2;
        let prefix = if s.is_input { "ILOGIC" } else { "OLOGIC" };
        while used_sites.contains(&format!("{prefix}_X{x}Y{y}")) {
            y += 2;
        }
        let tile = format!("LIOI3_X0Y{y}");
        let site = format!("{prefix}_X{x}Y{y}");
        used_sites.insert(site.clone());
        master_coords.insert(i, (bank.clone(), x, y));
        placed_serdes.insert(
            i,
            PlacedSerdes {
                name: s.name.clone(),
                is_input: s.is_input,
                tile,
                site,
                bank,
                serdes_mode: s.serdes_mode.clone(),
            },
        );
    }

    // Second pass: place SLAVE serdes immediately adjacent (`y + 1`) to its MASTER in the same IOI3 tile.
    for (i, s) in packed.serdes.iter().enumerate() {
        if s.serdes_mode != "SLAVE" {
            continue;
        }
        let master_idx = s
            .shiftin1_net
            .as_deref()
            .and_then(|n| shift_out_map.get(n).copied())
            .or_else(|| {
                s.shiftin2_net
                    .as_deref()
                    .and_then(|n| shift_out_map.get(n).copied())
            });
        let (bank, x, y_slave) = if let Some(mi) = master_idx {
            let (mbank, mx, my) = master_coords.get(&mi).cloned().ok_or_else(|| {
                format!("place: SLAVE SERDES {} references unplaced MASTER", s.name)
            })?;
            if let Some(sb) = &s.bank {
                if sb != &mbank {
                    return Err(format!(
                        "place: SLAVE SERDES {} in bank {sb} conflicts with MASTER in bank {mbank}",
                        s.name
                    ));
                }
            }
            (mbank, mx, my + 1)
        } else {
            let (b, x, y) = resolve_io_coord(
                s.loc.as_deref(),
                s.bank.as_deref(),
                s.serial_net.as_deref(),
                &mut next_io_y,
            )?;
            (b, x, (y / 2) * 2 + 1)
        };
        let tile = format!("LIOI3_X0Y{}", (y_slave / 2) * 2);
        let prefix = if s.is_input { "ILOGIC" } else { "OLOGIC" };
        let site = format!("{prefix}_X{x}Y{y_slave}");
        used_sites.insert(site.clone());
        placed_serdes.insert(
            i,
            PlacedSerdes {
                name: s.name.clone(),
                is_input: s.is_input,
                tile,
                site,
                bank,
                serdes_mode: s.serdes_mode.clone(),
            },
        );
    }

    for i in 0..packed.serdes.len() {
        if let Some(ps) = placed_serdes.remove(&i) {
            out.serdes.push(ps);
        }
    }

    Ok(())
}

fn place_srls(
    packed: &Packed7,
    device: &dyn DeviceQuery,
    anchor: (i32, i32),
    used_sites: &mut HashSet<String>,
    site_bels: &mut HashMap<String, HashSet<char>>,
    site_tile: &mut HashMap<String, String>,
    out_srls: &mut Vec<PlacedSrl>,
) -> Result<(), String> {
    if packed.srls.is_empty() {
        return Ok(());
    }
    // Order SRLs descending (D, C, B, A) when cascaded via Q31 -> D inside a SLICEM.
    let srl_letters = ['D', 'C', 'B', 'A'];
    let mut current_slicem: Option<(String, String, usize)> = None;

    for srl in &packed.srls {
        let (tile, site, letter_idx) = match &mut current_slicem {
            Some((t, s, idx)) if *idx < 4 => {
                let cur = *idx;
                *idx += 1;
                (t.clone(), s.clone(), cur)
            }
            _ => {
                let (t, s) = device
                    .slicem_near(anchor.0, anchor.1, used_sites)
                    .ok_or_else(|| format!("place: no free SLICEM site for SRL {}", srl.name))?;
                used_sites.insert(s.clone());
                site_tile.insert(s.clone(), t.clone());
                site_bels.entry(s.clone()).or_default();
                current_slicem = Some((t.clone(), s.clone(), 1));
                (t, s, 0)
            }
        };
        let letter = srl_letters[letter_idx];
        site_bels.entry(site.clone()).or_default().insert(letter);
        out_srls.push(PlacedSrl {
            name: srl.name.clone(),
            kind: srl.kind,
            init: srl.init,
            tile,
            site,
            bel: format!("{letter}6LUT"),
            is_clk_inverted: srl.is_clk_inverted,
        });
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn place_muxf_trees(
    packed: &Packed7,
    device: &dyn DeviceQuery,
    anchor: (i32, i32),
    lut_by_output: &HashMap<&str, usize>,
    lut_placed: &mut HashSet<usize>,
    used_sites: &mut HashSet<String>,
    site_bels: &mut HashMap<String, HashSet<char>>,
    site_tile: &mut HashMap<String, String>,
    out_luts: &mut Vec<PlacedLut>,
    out_muxfs: &mut Vec<PlacedMuxf>,
) -> Result<(), String> {
    if packed.muxfs.is_empty() {
        return Ok(());
    }
    let f7_by_out: HashMap<&str, usize> = packed
        .muxfs
        .iter()
        .enumerate()
        .filter(|(_, m)| m.kind == MuxfKind::Muxf7)
        .map(|(i, m)| (m.o_net.as_str(), i))
        .collect();
    let mut f7_placed: HashSet<usize> = HashSet::new();
    let mut placed_muxf_map: HashMap<usize, PlacedMuxf> = HashMap::new();

    // Place MUXF8 trees (each MUXF8 needs F7AMUX + F7BMUX in the same slice).
    for (m8_idx, m8) in packed.muxfs.iter().enumerate() {
        if m8.kind != MuxfKind::Muxf8 {
            continue;
        }
        let (tile, site) = device
            .slice_near(anchor.0, anchor.1, used_sites)
            .ok_or_else(|| format!("place: no free SLICE for MUXF8 {}", m8.name))?;
        used_sites.insert(site.clone());
        site_tile.insert(site.clone(), tile.clone());
        let bels = site_bels.entry(site.clone()).or_default();

        if let Some(&f7a) = f7_by_out.get(m8.i0_net.as_str()) {
            f7_placed.insert(f7a);
            let f7 = &packed.muxfs[f7a];
            place_f7_inputs(
                f7,
                ('A', 'B'),
                &tile,
                &site,
                packed,
                lut_by_output,
                lut_placed,
                bels,
                out_luts,
            );
            placed_muxf_map.insert(
                f7a,
                PlacedMuxf {
                    name: f7.name.clone(),
                    kind: MuxfKind::Muxf7,
                    tile: tile.clone(),
                    site: site.clone(),
                    bel: "F7AMUX".to_string(),
                },
            );
        }
        if let Some(&f7b) = f7_by_out.get(m8.i1_net.as_str()) {
            f7_placed.insert(f7b);
            let f7 = &packed.muxfs[f7b];
            place_f7_inputs(
                f7,
                ('C', 'D'),
                &tile,
                &site,
                packed,
                lut_by_output,
                lut_placed,
                bels,
                out_luts,
            );
            placed_muxf_map.insert(
                f7b,
                PlacedMuxf {
                    name: f7.name.clone(),
                    kind: MuxfKind::Muxf7,
                    tile: tile.clone(),
                    site: site.clone(),
                    bel: "F7BMUX".to_string(),
                },
            );
        }
        placed_muxf_map.insert(
            m8_idx,
            PlacedMuxf {
                name: m8.name.clone(),
                kind: MuxfKind::Muxf8,
                tile,
                site,
                bel: "F8MUX".to_string(),
            },
        );
    }

    // Place standalone MUXF7s.
    for (m7_idx, m7) in packed.muxfs.iter().enumerate() {
        if m7.kind != MuxfKind::Muxf7 || f7_placed.contains(&m7_idx) {
            continue;
        }
        let (tile, site) = device
            .slice_near(anchor.0, anchor.1, used_sites)
            .ok_or_else(|| format!("place: no free SLICE for MUXF7 {}", m7.name))?;
        used_sites.insert(site.clone());
        site_tile.insert(site.clone(), tile.clone());
        let bels = site_bels.entry(site.clone()).or_default();
        place_f7_inputs(
            m7,
            ('A', 'B'),
            &tile,
            &site,
            packed,
            lut_by_output,
            lut_placed,
            bels,
            out_luts,
        );
        placed_muxf_map.insert(
            m7_idx,
            PlacedMuxf {
                name: m7.name.clone(),
                kind: MuxfKind::Muxf7,
                tile,
                site,
                bel: "F7AMUX".to_string(),
            },
        );
    }

    for i in 0..packed.muxfs.len() {
        if let Some(pm) = placed_muxf_map.remove(&i) {
            out_muxfs.push(pm);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn place_f7_inputs(
    f7: &crate::pack::PackedMuxf,
    letters: (char, char),
    tile: &str,
    site: &str,
    packed: &Packed7,
    lut_by_output: &HashMap<&str, usize>,
    lut_placed: &mut HashSet<usize>,
    bels: &mut HashSet<char>,
    out_luts: &mut Vec<PlacedLut>,
) {
    for (net, letter) in [(&f7.i0_net, letters.0), (&f7.i1_net, letters.1)] {
        if let Some(&li) = lut_by_output.get(net.as_str()) {
            if lut_placed.insert(li) {
                bels.insert(letter);
                let lut = &packed.luts[li];
                out_luts.push(PlacedLut {
                    name: lut.name.clone(),
                    init: lut.init,
                    tile: tile.to_string(),
                    site: site.to_string(),
                    bel: format!("{letter}6LUT"),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::{PackedBufg, PackedCarry, PackedDsp, PackedFf, PackedIob, PackedLut};

    struct FakeDevice;
    impl DeviceQuery for FakeDevice {
        fn iob_site_for_pin(&self, pin: &str) -> Option<(String, String)> {
            match pin {
                "E3" => Some(("RIOB33_X0Y0".into(), "IOB_X0Y0".into())),
                "H5" => Some(("RIOB33_X0Y1".into(), "IOB_X0Y1".into())),
                _ => None,
            }
        }
        fn bufg_site(&self) -> Option<(String, String)> {
            Some(("CLK_BUFG_X0Y0".into(), "BUFGCTRL_X0Y0".into()))
        }
        fn ramb_site(&self) -> Option<(String, String)> {
            Some(("BRAM_X0Y0".into(), "RAMB18_X0Y0".into()))
        }
        fn dsp_site(&self) -> Option<(String, String)> {
            Some(("DSP_X0Y0".into(), "DSP48_X0Y0".into()))
        }
        fn tile_xy(&self, _tile: &str) -> Option<(i32, i32)> {
            Some((10, 10))
        }
        fn slice_near(
            &self,
            _x: i32,
            _y: i32,
            used: &HashSet<String>,
        ) -> Option<(String, String)> {
            for i in 0..100 {
                let site = format!("SLICE_X0Y{i}");
                if !used.contains(&site) {
                    return Some((format!("CLB_X0Y{i}"), site));
                }
            }
            None
        }
        // Same parity rule as the real device: X1 of the same tile after
        // X0, else X0 of the tile one row higher. Tile name follows site.
        fn slice_above(&self, site: &str, used: &HashSet<String>) -> Option<(String, String)> {
            let (x, y) = crate::xdevice::parse_slice_xy(site)?;
            let (tx, ty) = if x % 2 == 0 { (x + 1, y) } else { (x - 1, y + 1) };
            if !(0..100).contains(&ty) {
                return None;
            }
            let want = format!("SLICE_X{tx}Y{ty}");
            if used.contains(&want) {
                return None;
            }
            Some((format!("CLB_X{tx}Y{ty}"), want))
        }
        fn anchor_slice(&self, site: &str, used: &HashSet<String>) -> Option<(String, String)> {
            let (x, y) = crate::xdevice::parse_slice_xy(site)?;
            if !(0..100).contains(&y) || used.contains(site) {
                return None;
            }
            Some((format!("CLB_X{x}Y{y}"), site.to_string()))
        }
    }

    fn packed_blinky() -> Packed7 {
        Packed7 {
            luts: vec![PackedLut {
                name: "u_inv".into(),
                init: 0x5555_5555_5555_5555,
                input_nets: [Some("q".into()), None, None, None, None, None],
                output_net: "d".into(),
            }],
            ffs: vec![PackedFf {
                name: "u_ff".into(),
                d_net: "d".into(),
                q_net: "q".into(),
                clk_net: "clk_bufg".into(),
                ce_net: None,
                sr_net: None, init: false, srval: false, sr_async: false,
            }],
            iobs: vec![
                PackedIob {
                    name: "ibuf_clk".into(),
                    port: "clk".into(),
                    dir: IobDir::In,
                    net: "clk_ibuf".into(),
                    loc: Some("E3".into()),
                    iostandard: "LVCMOS33".into(),
                    drive: None,
                    slew: None,
                },
                PackedIob {
                    name: "u_obuf".into(),
                    port: "led".into(),
                    dir: IobDir::Out,
                    net: "q".into(),
                    loc: Some("H5".into()),
                    iostandard: "LVCMOS33".into(),
                    drive: None,
                    slew: None,
                },
            ],
            bufgs: vec![PackedBufg {
                name: "bufg_clk".into(),
                in_net: "clk_ibuf".into(),
                out_net: "clk_bufg".into(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn place_pairs_lut_ff_in_one_slice() {
        let placed = place(&packed_blinky(), &FakeDevice).unwrap();
        assert_eq!(placed.iobs.len(), 2);
        assert_eq!(placed.bufgs.len(), 1);
        assert_eq!(placed.luts.len(), 1);
        assert_eq!(placed.ffs.len(), 1);
        assert_eq!(placed.luts[0].site, placed.ffs[0].site);
        assert_eq!(placed.luts[0].bel, "A6LUT");
        assert_eq!(placed.ffs[0].bel, "AFF");
        assert!(placed.iobs.iter().any(|i| i.site == "IOB_X0Y0"));
        assert!(placed.iobs.iter().any(|i| i.site == "IOB_X0Y1"));
    }

    #[test]
    fn place_missing_pin_fails_loudly() {
        let mut p = packed_blinky();
        p.iobs[0].loc = Some("Z99".into());
        let err = place(&p, &FakeDevice).unwrap_err();
        assert!(err.contains("Z99"), "got: {err}");
    }

    #[test]
    fn place_carry_dsp_bram_cascades_are_column_aligned() {
        let mut p = Packed7::default();
        p.carries.push(PackedCarry {
            name: "c0".into(),
            cyinit: Some(false),
            co_net: Some("c0_co3".into()),
            ..Default::default()
        });
        p.carries.push(PackedCarry {
            name: "c1".into(),
            ci_net: "c0_co3".into(),
            ..Default::default()
        });
        p.dsps.push(PackedDsp {
            name: "d0".into(),
            pcout_net: Some("p_casc".into()),
            ..Default::default()
        });
        p.dsps.push(PackedDsp {
            name: "d1".into(),
            pcin_net: Some("p_casc".into()),
            ..Default::default()
        });
        let placed = place(&p, &FakeDevice).unwrap();
        // The second group sits on the slice the first one's COUT cascades
        // into (the fake column runs X0Y0, X1Y0, X0Y1, ...).
        assert_eq!(placed.carries[0].site, "SLICE_X0Y0");
        assert_eq!(
            Some(placed.carries[1].site.clone()),
            FakeDevice.slice_above("SLICE_X0Y0", &HashSet::new()).map(|s| s.1)
        );
        assert_eq!(placed.dsps[0].site, "DSP48_X0Y0");
        assert_eq!(placed.dsps[1].site, "DSP48_X0Y1");
    }

    /// The clock inverter is per slice: a falling-edge flop never shares a
    /// slice with a rising-edge one, and two of the same edge do share.
    #[test]
    fn place_keeps_clock_edges_in_separate_slices() {
        let mut p = packed_blinky();
        for (i, name) in ["u_ff_n", "u_ff_p"].iter().enumerate() {
            p.ffs.push(PackedFf {
                name: (*name).into(),
                d_net: format!("x{i}"),
                q_net: format!("y{i}"),
                clk_net: "clk_bufg".into(),
                ce_net: None,
                sr_net: None,
                init: false,
                srval: false,
                sr_async: false,
            });
        }
        p.negedge_ffs.insert("u_ff_n".into());
        let placed = place(&p, &FakeDevice).unwrap();
        let site = |n: &str| placed.ffs.iter().find(|f| f.name == n).unwrap().site.clone();
        assert_ne!(site("u_ff_n"), site("u_ff"));
        assert_ne!(site("u_ff_n"), site("u_ff_p"));
        assert_eq!(site("u_ff_p"), site("u_ff"));
    }

    fn packed_carry(name: &str, ci: &str, co: Option<&str>) -> PackedCarry {
        PackedCarry {
            name: name.into(),
            ci_net: ci.into(),
            cyinit: None,
            co_net: co.map(|s| s.into()),
            s_nets: Default::default(),
            di_nets: Default::default(),
            o_nets: Default::default(),
            ..Default::default()
        }
    }

    /// A 3-group chain (listed tail-first to prove ordering is by net
    /// linkage, not input order) must land on consecutive column slices:
    /// X0Y0, X1Y0, X0Y1.
    #[test]
    fn place_chained_carries_stack_up_the_column() {
        let mut p = packed_blinky();
        p.carries = vec![
            packed_carry("u_c2", "co1", None),
            packed_carry("u_c0", "one", Some("co0")),
            packed_carry("u_c1", "co0", Some("co1")),
        ];
        let placed = place(&p, &FakeDevice).unwrap();
        let sites: Vec<&str> = placed.carries.iter().map(|c| c.site.as_str()).collect();
        assert_eq!(sites, vec!["SLICE_X0Y0", "SLICE_X1Y0", "SLICE_X0Y1"]);
        let names: Vec<&str> = placed.carries.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["u_c0", "u_c1", "u_c2"]);
    }

    /// Carry co-placement: each bit FF (D from carry O) and each S-LUT (O
    /// to carry S) take their bit letter in the carry slice. The head
    /// CYINIT one-LUT places generically.
    #[test]
    fn place_carry_ff_and_slut_share_carry_slice() {
        let mut p = packed_blinky();
        p.carries = vec![
            packed_carry("u_c0", "one", Some("co0")),
            packed_carry("u_c1", "co0", None),
        ];
        // Full group 0 (bits 0-3) + partial group 1 (bits 0-1).
        for j in 0..4 {
            p.carries[0].o_nets[j] = Some(format!("o{j}"));
            p.carries[0].s_nets[j] = Some(format!("s{j}"));
        }
        for j in 0..2 {
            p.carries[1].o_nets[j] = Some(format!("o{}", 4 + j));
            p.carries[1].s_nets[j] = Some(format!("s{}", 4 + j));
        }
        for j in 0..6 {
            p.ffs.push(PackedFf {
                name: format!("u_ff{j}"),
                d_net: format!("o{j}"),
                q_net: format!("q{j}"),
                clk_net: "clk_bufg".into(),
                ce_net: None,
                sr_net: None, init: false, srval: false, sr_async: false,
            });
            p.luts.push(PackedLut {
                name: format!("u_s{j}"),
                init: 0xFFFF_FFFF_0000_0000,
                input_nets: [None, None, None, None, None, Some(format!("q{j}"))],
                output_net: format!("s{j}"),
            });
        }
        p.luts.push(PackedLut {
            name: "u_one".into(),
            init: u64::MAX,
            input_nets: [None, None, None, None, None, None],
            output_net: "one".into(),
        });
        let placed = place(&p, &FakeDevice).unwrap();
        let ff = |n: &str| placed.ffs.iter().find(|f| f.name == n).unwrap();
        let lut = |n: &str| placed.luts.iter().find(|l| l.name == n).unwrap();
        // Bit j takes BEL letter j in the group slice.
        assert_eq!((ff("u_ff0").site.as_str(), ff("u_ff0").bel.as_str()), ("SLICE_X0Y0", "AFF"));
        assert_eq!((ff("u_ff3").site.as_str(), ff("u_ff3").bel.as_str()), ("SLICE_X0Y0", "DFF"));
        assert_eq!((ff("u_ff4").site.as_str(), ff("u_ff4").bel.as_str()), ("SLICE_X1Y0", "AFF"));
        assert_eq!((ff("u_ff5").site.as_str(), ff("u_ff5").bel.as_str()), ("SLICE_X1Y0", "BFF"));
        assert_eq!((lut("u_s0").site.as_str(), lut("u_s0").bel.as_str()), ("SLICE_X0Y0", "A6LUT"));
        assert_eq!((lut("u_s2").site.as_str(), lut("u_s2").bel.as_str()), ("SLICE_X0Y0", "C6LUT"));
        assert_eq!((lut("u_s4").site.as_str(), lut("u_s4").bel.as_str()), ("SLICE_X1Y0", "A6LUT"));
        // Carry-bit FFs stay co-placed in carry slices; the generic
        // LUT+FF pair opens a fresh slice instead of a carry gap.
        for f in &placed.ffs {
            if f.name == "u_ff" {
                assert_ne!(f.site, "SLICE_X0Y0");
                assert_ne!(f.site, "SLICE_X1Y0");
            } else if f.name.starts_with("u_ff") {
                assert!(f.site == "SLICE_X0Y0" || f.site == "SLICE_X1Y0", "{}", f.site);
            }
        }
        // The generic LUT+FF pair above already proves foreign logic
        // opens a fresh slice instead of a carry gap (chain tiles hold
        // chain logic only, as in the oracle).
    }

    /// A pinned chain-head anchor is honored: the head lands on the pinned
    /// site and the chain walks up from there. The mechanism is general;
    /// convergence runs pin golden columns with it.
    #[test]
    fn place_carry_anchor_pins_chain_head() {
        let mut p = packed_blinky();
        p.carry_anchor = Some("SLICE_X0Y5".into());
        p.carries = vec![
            packed_carry("u_c0", "one", Some("co0")),
            packed_carry("u_c1", "co0", None),
        ];
        let placed = place(&p, &FakeDevice).unwrap();
        let sites: Vec<&str> = placed.carries.iter().map(|c| c.site.as_str()).collect();
        assert_eq!(sites, vec!["SLICE_X0Y5", "SLICE_X1Y5"]);
    }

    /// A garbage anchor fails loudly, never silently near.
    #[test]
    fn place_carry_bad_anchor_fails_loudly() {
        let mut p = packed_blinky();
        p.carry_anchor = Some("NOPE".into());
        p.carries = vec![packed_carry("u_c0", "one", None)];
        let err = place(&p, &FakeDevice).unwrap_err();
        assert!(err.contains("NOPE") && err.contains("anchor"), "got: {err}");
    }

    /// A carry cycle has no head: fail loudly instead of scattering.
    #[test]
    fn place_carry_cycle_fails_loudly() {
        let mut p = packed_blinky();
        p.carries = vec![
            packed_carry("u_c0", "b", Some("a")),
            packed_carry("u_c1", "a", Some("b")),
        ];
        let err = place(&p, &FakeDevice).unwrap_err();
        assert!(err.contains("chain head"), "got: {err}");
    }

    /// Fake column rule mirrors the device: X1 of the same tile after X0,
    /// X0 of the next row after X1; taken neighbor yields None.
    #[test]
    /// G55: the cascade neighbour rule, checked on the real `xc7z020` grid
    /// rather than a fake one.
    ///
    /// `slice_above` is "X1 of the same tile after X0, else X0 of the name-Y
    /// above": X0 -> X1 within a tile, then X1 -> X0 of the next tile up. The
    /// router accepts both that column form and the paired (same-tile) form.
    /// What this pins is that the neighbour a legacy chain asks for always
    /// exists on a real part and always walks one column without repeating a
    /// site or leaving the grid - a chain that crosses into the next column
    /// would not cascade in silicon at all. Whether the paired form is legal
    /// is a bitstream question (needs the board, R9); if it turns out not to
    /// be, the fix belongs in route.rs, not here, so this guard must keep
    /// asserting the column rule.
    #[test]
    fn real_slice_above_walks_one_column() {
        use crate::device::{default_db_root, Device};
        use crate::xdevice::{build_node_map, XDevice};

        let db_root = default_db_root();
        let mut device = Device::load_xc7z020(&db_root).expect("load xc7z020");
        let maps = build_node_map(&mut device);
        let xdev = XDevice::new(&device, maps);
        let used = HashSet::new();

        // Start from an X0 in the middle of the die and walk 8 links up.
        let mut site = "SLICE_X71Y44".to_string();
        let mut seen: Vec<String> = vec![site.clone()];
        for step in 0..8 {
            let (tile, next) = xdev
                .slice_above(&site, &used)
                .unwrap_or_else(|| panic!("no neighbour above {site} at step {step}"));
            assert!(!seen.contains(&next), "column walk revisited {next}");
            // The rule is X0 -> X1 within a tile, then X1 -> X0 of the tile
            // above, so X alternates within the CLB column and Y only moves
            // on the X1 -> X0 step. Asserting "same X" would be wrong: the
            // two halves of a CLB have different X names.
            let (sx, sy) = crate::place::parse_site_xy(&site).unwrap();
            let (nx, ny) = crate::place::parse_site_xy(&next).unwrap();
            let (want_x, want_y) = if sx % 2 == 0 { (sx + 1, sy) } else { (sx - 1, sy + 1) };
            assert_eq!((nx, ny), (want_x, want_y), "wrong neighbour: {site} -> {next}");
            // Never a step down, and never more than one row: the chain only
            // ever climbs.
            assert!(ny >= sy, "cascade went down: {site} -> {next}");
            assert!(ny - sy <= 1, "cascade skipped a row: {site} -> {next}");
            // The neighbour is a real site, and in the CLB tile we reported.
            assert!(
                device.slices().iter().any(|(_, s)| s == &next),
                "{next} is not a slice on the part"
            );
            assert!(
                device.slices().iter().any(|(t, s)| s == &next && t == &tile),
                "{next} is not in the tile we reported ({tile})"
            );
            seen.push(next.clone());
            site = next;
        }
    }

    fn fake_slice_above_follows_column_rule() {
        let dev = FakeDevice;
        let used = HashSet::new();
        assert_eq!(
            dev.slice_above("SLICE_X0Y0", &used),
            Some(("CLB_X1Y0".into(), "SLICE_X1Y0".into()))
        );
        assert_eq!(
            dev.slice_above("SLICE_X1Y0", &used),
            Some(("CLB_X0Y1".into(), "SLICE_X0Y1".into()))
        );
        let mut taken = HashSet::new();
        taken.insert("SLICE_X1Y0".into());
        assert_eq!(dev.slice_above("SLICE_X0Y0", &taken), None);
    }
}

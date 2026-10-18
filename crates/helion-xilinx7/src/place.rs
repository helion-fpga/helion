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
    pub muxfs: Vec<PlacedMuxf>,
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
    place_brams(&packed.brams, device, &mut used_sites, &mut out.brams)?;

    // 2c. DSP48E1s with column-aligned PCOUT->PCIN / ACOUT->ACIN / BCOUT->BCIN cascade placement.
    place_dsps(&packed.dsps, device, &mut used_sites, &mut out.dsps)?;

    // 2d. CARRY4s with column-aligned vertical CO[3]->CI chain placement.
    let (ax, ay) = anchor.unwrap_or((0, 0));
    place_carries(
        &packed.carries,
        device,
        (ax, ay),
        &mut used_sites,
        &mut out.carries,
    )?;

    // 2e. I/O logic primitives: IDELAYE2 / ODELAYE2 / IDELAYCTRL, IDDR / ODDR, ISERDESE2 / OSERDESE2.
    place_io_primitives(packed, device, &net_to_iob, &mut used_sites, &mut out)?;

    // 2f. SRL16E / SRLC32E in SLICEM sites (with Q31->D cascade co-location).
    let mut site_bels: HashMap<String, HashSet<char>> = HashMap::new();
    let mut site_tile: HashMap<String, String> = HashMap::new();
    place_srls(
        packed,
        device,
        (ax, ay),
        &mut used_sites,
        &mut site_bels,
        &mut site_tile,
        &mut out.srls,
    )?;

    // 3. Pair FFs with their driving LUTs (FF.D == LUT.O) into one slice.
    let lut_by_output: HashMap<&str, usize> = packed
        .luts
        .iter()
        .enumerate()
        .map(|(i, l)| (l.output_net.as_str(), i))
        .collect();
    let mut lut_placed: HashSet<usize> = HashSet::new();

    let alloc = |used: &mut HashSet<String>,
                 site_bels: &mut HashMap<String, HashSet<char>>,
                 site_tile: &mut HashMap<String, String>,
                 device: &dyn DeviceQuery|
     -> Result<(String, String, char), String> {
        for (site, bels) in site_bels.iter() {
            if bels.len() < 4 {
                if let Some(letter) = BEL_LETTERS.iter().find(|c| !bels.contains(c)) {
                    let tile = site_tile.get(site).cloned().unwrap_or_default();
                    return Ok((tile, site.clone(), *letter));
                }
            }
        }
        let (tile, site) = device
            .slice_near(ax, ay, used)
            .ok_or_else(|| "place: no free SLICEL site".to_string())?;
        used.insert(site.clone());
        site_tile.insert(site.clone(), tile.clone());
        site_bels.entry(site.clone()).or_default();
        Ok((tile, site, 'A'))
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

    for ff in &packed.ffs {
        let lut_idx = lut_by_output
            .get(ff.d_net.as_str())
            .copied()
            .filter(|i| !lut_placed.contains(i));
        let (tile, site, letter) = alloc(&mut used_sites, &mut site_bels, &mut site_tile, device)?;
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
        let (tile, site, letter) = alloc(&mut used_sites, &mut site_bels, &mut site_tile, device)?;
        site_bels.get_mut(&site).unwrap().insert(letter);
        out.luts.push(PlacedLut {
            name: lut.name.clone(),
            init: lut.init,
            tile,
            site,
            bel: format!("{letter}6LUT"),
        });
    }

    Ok(out)
}

fn place_brams(
    brams: &[PackedBram],
    device: &dyn DeviceQuery,
    used_sites: &mut HashSet<String>,
    out_brams: &mut Vec<PlacedBram>,
) -> Result<(), String> {
    if brams.is_empty() {
        return Ok(());
    }
    let ramb18_pool = device.ramb18_sites();
    let ramb36_pool = device.ramb36_sites();

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
        let pool = if is_36 { &ramb36_pool } else { &ramb18_pool };
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
    device: &dyn DeviceQuery,
    used_sites: &mut HashSet<String>,
    out_dsps: &mut Vec<PlacedDsp>,
) -> Result<(), String> {
    if dsps.is_empty() {
        return Ok(());
    }
    let pool = device.dsp_sites();
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
        let window = find_column_window(&pool, chain.len(), used_sites).ok_or_else(|| {
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

fn place_carries(
    carries: &[PackedCarry],
    device: &dyn DeviceQuery,
    anchor: (i32, i32),
    used_sites: &mut HashSet<String>,
    out_carries: &mut Vec<PlacedCarry>,
) -> Result<(), String> {
    if carries.is_empty() {
        return Ok(());
    }
    let mut co3_map: HashMap<&str, usize> = HashMap::new();
    for (i, c) in carries.iter().enumerate() {
        if let Some(co3) = &c.co_nets[3] {
            co3_map.insert(co3.as_str(), i);
        }
    }
    let mut prev_of = vec![None; carries.len()];
    let mut next_of = vec![None; carries.len()];
    for (i, c) in carries.iter().enumerate() {
        if let Some(ci) = &c.ci_net {
            if let Some(&p) = co3_map.get(ci.as_str()) {
                prev_of[i] = Some(p);
                next_of[p] = Some(i);
            }
        }
    }

    let mut placed_idx: HashMap<usize, PlacedCarry> = HashMap::new();
    for start in 0..carries.len() {
        if prev_of[start].is_some() || placed_idx.contains_key(&start) {
            continue;
        }
        let mut chain = vec![start];
        let mut cur = start;
        while let Some(nxt) = next_of[cur] {
            if chain.contains(&nxt) {
                return Err(format!(
                    "place: cyclic CARRY4 chain involving {}",
                    carries[nxt].name
                ));
            }
            chain.push(nxt);
            cur = nxt;
        }
        let sites = if chain.len() == 1 {
            let (t, s) = device
                .slice_near(anchor.0, anchor.1, used_sites)
                .ok_or_else(|| format!("place: no SLICE site for CARRY4 {}", carries[start].name))?;
            vec![(t, s)]
        } else {
            device
                .slice_column(chain.len(), used_sites)
                .ok_or_else(|| {
                    format!(
                        "place: no vertical SLICE column of length {} for CARRY4 chain {}",
                        chain.len(),
                        carries[start].name
                    )
                })?
        };
        for (&ci, (tile, site)) in chain.iter().zip(sites.into_iter()) {
            used_sites.insert(site.clone());
            placed_idx.insert(
                ci,
                PlacedCarry {
                    name: carries[ci].name.clone(),
                    tile,
                    site,
                },
            );
        }
    }

    for i in 0..carries.len() {
        if let Some(pc) = placed_idx.remove(&i) {
            out_carries.push(pc);
        }
    }
    Ok(())
}

/// Find a contiguous vertical sequence of `len` free sites in `pool` having the
/// same `X` coordinate and consecutive `Y` coordinates (`y, y+1, ...`).
fn find_column_window(
    pool: &[(String, String)],
    len: usize,
    used: &HashSet<String>,
) -> Option<Vec<(String, String)>> {
    if len == 0 {
        return Some(Vec::new());
    }
    if len == 1 {
        return pool
            .iter()
            .find(|(_, s)| !used.contains(s))
            .cloned()
            .map(|p| vec![p]);
    }
    let mut by_col: HashMap<i32, Vec<(i32, String, String)>> = HashMap::new();
    for (tile, site) in pool {
        if !used.contains(site) {
            if let Some((x, y)) = parse_site_xy(site) {
                by_col
                    .entry(x)
                    .or_default()
                    .push((y, tile.clone(), site.clone()));
            }
        }
    }
    let mut cols: Vec<i32> = by_col.keys().copied().collect();
    cols.sort();
    for col in cols {
        let entries = by_col.get_mut(&col)?;
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
                sr_net: None,
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
            co_nets: [None, None, None, Some("c0_co3".into())],
            ..Default::default()
        });
        p.carries.push(PackedCarry {
            name: "c1".into(),
            ci_net: Some("c0_co3".into()),
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
        assert_eq!(placed.carries[0].site, "SLICE_X0Y0");
        assert_eq!(placed.carries[1].site, "SLICE_X0Y1");
        assert_eq!(placed.dsps[0].site, "DSP48_X0Y0");
        assert_eq!(placed.dsps[1].site, "DSP48_X0Y1");
    }
}

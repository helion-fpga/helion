//! Place stage: [`Packed7`] → [`Placed7`].
//!
//! Greedy affinity placer for 7-series sites. IOBs go to their package pins,
//! BUFGCTRLs to clock tiles, and LUT/FF pairs sharing a `D`/`O` net are
//! packed into one slice (LUT→`x6LUT`, FF→`xFF`, x = A..D). Unpaired cells
//! get their own slice. The placer queries the device through
//! [`DeviceQuery`] so the algorithm stays independent of the DB layout.

use crate::pack::{IobDir, Packed7, PackedCarry};
use std::collections::{HashMap, HashSet};

/// What the placer needs from the device model. Implemented by the real DB
/// (`crate::db`) and by test fakes.
pub trait DeviceQuery {
    /// `(tile, site)` for a package pin, e.g. `"E3"`.
    fn iob_site_for_pin(&self, pin: &str) -> Option<(String, String)>;
    /// A BUFGCTRL site: `(tile, site)`.
    fn bufg_site(&self) -> Option<(String, String)>;
    /// A RAMB18E1 site: `(tile, site)`.
    fn ramb_site(&self) -> Option<(String, String)>;
    /// A DSP48E1 site: `(tile, site)`.
    fn dsp_site(&self) -> Option<(String, String)>;
    /// Grid coordinates of a tile (for distance estimates).
    fn tile_xy(&self, tile: &str) -> Option<(i32, i32)>;
    /// A free SLICEL site near `(x, y)`; `used` holds taken site names.
    fn slice_near(&self, x: i32, y: i32, used: &HashSet<String>) -> Option<(String, String)>;
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

/// A placed BUFGCTRL.
#[derive(Clone, Debug)]
pub struct PlacedBufg {
    pub name: String,
    pub tile: String,
    pub site: String,
}

/// A placed RAMB18E1.
#[derive(Clone, Debug)]
pub struct PlacedBram {
    pub name: String,
    pub tile: String,
    pub site: String,
}

/// A placed DSP48E1.
#[derive(Clone, Debug)]
pub struct PlacedDsp {
    pub name: String,
    pub tile: String,
    pub site: String,
}

/// A placed CARRY4 (in a SLICE).
#[derive(Clone, Debug)]
pub struct PlacedCarry {
    pub name: String,
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

/// Place packed cells onto device sites.
///
/// Errors loudly when a required site class is missing (no silent drops).
pub fn place(packed: &Packed7, device: &dyn DeviceQuery) -> Result<Placed7, String> {
    let mut out = Placed7 {
        packed: packed.clone(),
        ..Default::default()
    };
    let mut used_sites: HashSet<String> = HashSet::new();

    // 1. IOBs to package pins.
    let mut anchor: Option<(i32, i32)> = None;
    for iob in &packed.iobs {
        let pin = iob.loc.as_deref().ok_or_else(|| {
            format!("place: IOB {} (port {}) has no PACKAGE_PIN LOC", iob.name, iob.port)
        })?;
        let (tile, site) = device.iob_site_for_pin(pin).ok_or_else(|| {
            format!("place: no IOB site for package pin {pin} (port {})", iob.port)
        })?;
        if anchor.is_none() {
            anchor = device.tile_xy(&tile);
        }
        used_sites.insert(site.clone());
        out.iobs.push(PlacedIob {
            name: iob.name.clone(),
            port: iob.port.clone(),
            dir: iob.dir,
            tile,
            site,
        });
    }

    // 2. BUFGCTRLs.
    for bufg in &packed.bufgs {
        let (tile, site) = device
            .bufg_site()
            .ok_or_else(|| format!("place: no BUFGCTRL site for {}", bufg.name))?;
        used_sites.insert(site.clone());
        out.bufgs.push(PlacedBufg {
            name: bufg.name.clone(),
            tile,
            site,
        });
    }

    // 2b. RAMB18E1s.
    for bram in &packed.brams {
        let (tile, site) = device
            .ramb_site()
            .ok_or_else(|| format!("place: no RAMB18E1 site for {}", bram.name))?;
        used_sites.insert(site.clone());
        out.brams.push(PlacedBram {
            name: bram.name.clone(),
            tile,
            site,
        });
    }

    // 2c. DSP48E1s.
    for dsp in &packed.dsps {
        let (tile, site) = device
            .dsp_site()
            .ok_or_else(|| format!("place: no DSP48E1 site for {}", dsp.name))?;
        used_sites.insert(site.clone());
        out.dsps.push(PlacedDsp {
            name: dsp.name.clone(),
            tile,
            site,
        });
    }

    // 2d. CARRY4s, ordered into chains by CO-to-CI net linkage (ZB-08).
    // The first chain head honors `packed.carry_anchor` when set (a pinned
    // site for convergence measurements); every other head goes on a free
    // slice near the anchor. Every next group goes directly above its
    // predecessor so the dedicated COUT to CIN cascade wire reaches. A
    // broken column fails loudly: scattering chained groups would leave
    // the CO nets unroutable.
    let (ax, ay) = anchor.unwrap_or((0, 0));
    let mut carry_anchor = packed.carry_anchor.clone();
    for chain in order_carry_chains(&packed.carries)? {
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
    let mut carry_lut_slots: HashSet<(String, char)> = HashSet::new();
    let mut carry_ff_slots: HashSet<(String, char)> = HashSet::new();

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
    for (fi, ff) in packed.ffs.iter().enumerate() {
        let _ = fi;
        // Carry-driven FF: same slice and letter as its carry bit, so the
        // O-to-D path stays site-internal. A double-booked slot fails loud.
        if let Some((tile, site, letter)) = o_slot.get(ff.d_net.as_str()) {
            if !carry_ff_slots.insert((site.clone(), *letter)) {
                return Err(format!(
                    "place: FF {} collides on {site}.{letter}FF in carry slice",
                    ff.name
                ));
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
        let (tile, site, letter) = alloc(&mut used_sites, &mut site_bels, &mut site_tile, device)?;
        site_bels.get_mut(&site).unwrap().insert(letter);
        if let Some(li) = lut_idx {
            // Pair: LUT takes the same letter's LUT BEL, FF takes the FF BEL.
            // (Same letter keeps the intra-slice route trivial.)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::{PackedBufg, PackedCarry, PackedFf, PackedIob, PackedLut};

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
        fn slice_near(&self, _x: i32, _y: i32, used: &HashSet<String>) -> Option<(String, String)> {
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
            }],
            brams: vec![],
            dsps: vec![],
            carries: vec![],
            carry_anchor: None,
        }
    }

    #[test]
    fn place_pairs_lut_ff_in_one_slice() {
        let placed = place(&packed_blinky(), &FakeDevice).unwrap();
        assert_eq!(placed.iobs.len(), 2);
        assert_eq!(placed.bufgs.len(), 1);
        // LUT+FF paired: same site, A6LUT + AFF.
        assert_eq!(placed.luts.len(), 1);
        assert_eq!(placed.ffs.len(), 1);
        assert_eq!(placed.luts[0].site, placed.ffs[0].site);
        assert_eq!(placed.luts[0].bel, "A6LUT");
        assert_eq!(placed.ffs[0].bel, "AFF");
        // IOBs on their pins.
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

    fn packed_carry(name: &str, ci: &str, co: Option<&str>) -> PackedCarry {
        PackedCarry {
            name: name.into(),
            ci_net: ci.into(),
            co_net: co.map(|s| s.into()),
            s_nets: Default::default(),
            di_nets: Default::default(),
            o_nets: Default::default(),
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
                sr_net: None,
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
        // No S/FF cell spilled to a foreign slice.
        for f in &placed.ffs {
            if f.name.starts_with("u_ff") {
                assert!(f.site == "SLICE_X0Y0" || f.site == "SLICE_X1Y0", "{}", f.site);
            }
        }
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

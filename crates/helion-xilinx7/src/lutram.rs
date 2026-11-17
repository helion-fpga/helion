//! LUT RAM (distributed RAM) in SLICEM sites: `RAM32M`, `RAM64M`,
//! `RAM32X1S`/`D`, `RAM64X1S`/`D`, `RAM128X1S`/`D` and `RAM256X1S`, from
//! pack to FASM. Each primitive takes one SLICEM of its own.
//!
//! The slice image of each primitive (which LUT holds what, site pins,
//! features) is the one Vivado 2017.3 builds on the xc7z020, read from its
//! routed checkpoints (`tools/svcheck/tests_hard/lutram_prims.sv` and
//! PicoRV32's register file) and from X-Ray's decode of its bitstreams:
//!
//! * Every LUT RAM writes at the address on the D LUT's pins `D1..D6`
//!   (`WA1..WA6`); `WA7` is `CX` (`WA7USED`), `WA8` is `BX` (`WA8USED`).
//!   The write enable comes in on `CE` (`WEMUX.CE`), the clock on `CLK`.
//! * 64 deep (`xLUT.RAM`): pins `x1..x6` read, `INIT` is the contents; the
//!   data comes in on the LUT's `xI` pin (`xLUT.DI1MUX.xI`; the D LUT's `DI`
//!   always), or, with no `DI1MUX` bit, from the LUT above (C from D, B from
//!   C, A from B).
//! * 32 deep (`xLUT.SMALL` too): `x6` is left unrouted (reads 1); `O6` is
//!   `INIT[63:32]` written from the `xX` pin, `O5` is `INIT[31:0]` written
//!   from `xI` and leaves on `xMUX` (`xOUTMUX.O5`). `RAM32M` puts `DOx[1]`
//!   on O6 and `DOx[0]` on O5; `RAM32X1S`/`D` use O6 only.
//! * Deeper: `RAM128X1S`/`D` keep `INIT[63:0]` in D (and B) and
//!   `INIT[127:64]` in C (and A), joined by `F7BMUX` (select `CX`, out on
//!   `CMUX`) and `F7AMUX` (select `AX`, out on `AMUX`); `RAM256X1S` fills
//!   D, C, B, A from the bottom and adds `F8MUX` (select `BX`, out on
//!   `BMUX`).

use crate::place::{DeviceQuery, Placed7};
use helion_ir::{resolve, Attrs, PinIndex};
use std::collections::{HashMap, HashSet};

/// Slow-corner delays and setup times (ps) from X-Ray's
/// `timings/CLBLM_L.sdf` (`LUT_OR_MEM5LRAM`/`LUT_OR_MEM6LRAM`, `F7AMUX`,
/// `F8MUX`): address to O5 or O6, clock to O, and a wide mux on top.
const READ_PS: u32 = 150;
const CKO_PS: u32 = 1188;
const F7_PS: u32 = 225;
const F8_PS: u32 = 98;
const SETUP_WE_PS: u32 = 654;
const SETUP_DI_PS: u32 = 453;
const SETUP_WA_PS: u32 = 362;
const SETUP_WA78_PS: u32 = 633;

/// A packed LUT RAM primitive as the SLICEM it fills.
#[derive(Clone, Debug)]
pub struct PackedLutRam {
    pub name: String,
    pub module: String,
    pub clk: String,
    /// Input site pins with their nets; a pin the write is timed on has its
    /// setup time.
    pub ins: Vec<(String, String, Option<u32>)>,
    /// Output site pins with their nets, the input pins that reach each one
    /// through the read, and that read's delay.
    pub outs: Vec<(String, String, Vec<String>, u32)>,
    /// Site features (after `TILE.SLICEM_X0.`).
    pub features: Vec<String>,
    /// LUT contents, A..D (`None`: the LUT is not used).
    pub init: [Option<u64>; 4],
}

/// A placed LUT RAM.
#[derive(Clone, Debug)]
pub struct PlacedLutRam {
    pub name: String,
    pub tile: String,
    pub site: String,
}

pub fn is_lutram(module: &str) -> bool {
    matches!(module, "RAM32M" | "RAM64M" | "RAM32X1S" | "RAM32X1D" | "RAM64X1S" | "RAM64X1D" | "RAM128X1S" | "RAM128X1D" | "RAM256X1S")
}

/// `INIT` bits from `N'hXX..`, `N'bXX..` or plain hex, `n` long (missing: 0).
fn init_bits(attrs: &Attrs, key: &str, n: usize, cell: &str) -> Result<Vec<bool>, String> {
    let mut out = vec![false; n];
    let Some(raw) = attrs.get(key) else { return Ok(out) };
    let s = raw.trim();
    let (radix, digits) = match s.find('\'') {
        Some(i) => match s[i + 1..].trim_start_matches(['s', 'S']).split_at(1) {
            (r, d) if r.eq_ignore_ascii_case("h") => (16, d),
            (r, d) if r.eq_ignore_ascii_case("b") => (2, d),
            _ => return Err(format!("pack: {cell}: {key} = {raw} is not hex or binary")),
        },
        None => (16, s.trim_start_matches("0x")),
    };
    let step = if radix == 16 { 4 } else { 1 };
    for (k, c) in digits.chars().rev().filter(|c| *c != '_').enumerate() {
        let v = c.to_digit(radix).ok_or_else(|| format!("pack: {cell}: bad digit '{c}' in {key} = {raw}"))?;
        for b in 0..step {
            if (v >> b) & 1 == 1 {
                let i = k * step as usize + b;
                if i >= n {
                    return Err(format!("pack: {cell}: {key} = {raw} is wider than {n} bits"));
                }
                out[i] = true;
            }
        }
    }
    Ok(out)
}

fn word(bits: &[bool]) -> u64 {
    bits.iter().enumerate().fold(0, |v, (i, b)| v | (*b as u64) << i)
}

/// Pack a LUT RAM primitive instance (`module` per [`is_lutram`]).
pub fn pack(name: &str, module: &str, pins: &PinIndex, attrs: &Attrs) -> Result<PackedLutRam, String> {
    let net = |p: &str| pins.net_on(name, p).map(|s| resolve(s).to_string());
    let need = |p: &str| net(p).ok_or_else(|| format!("pack: {module} {name} has no {p} net"));
    let mut r = PackedLutRam {
        name: name.to_string(),
        module: module.to_string(),
        clk: need("WCLK")?,
        ins: vec![("CE".into(), need("WE")?, Some(SETUP_WE_PS))],
        outs: Vec::new(),
        features: vec!["WEMUX.CE".into()],
        init: [None; 4],
    };
    let lut_pins = |l: char, n: usize| -> Vec<String> { (1..=n).map(|i| format!("{l}{i}")).collect() };
    // LUT `l` reading at `addr` (pin bases, e.g. "ADDRA" or "A"); the D LUT's pins are the write address too.
    let mut read = |r: &mut PackedLutRam, l: char, addr: &str, n: usize| -> Result<(), String> {
        for i in 0..n {
            let p = format!("{addr}{i}");
            if let Some(nt) = net(&p) {
                let setup = (l == 'D').then_some(SETUP_WA_PS);
                r.ins.push((format!("{l}{}", i + 1), nt, setup));
            } else if l == 'D' {
                return Err(format!("pack: {module} {name} has no {p} net (the write address)"));
            }
        }
        r.features.push(format!("{l}LUT.RAM"));
        Ok(())
    };
    let li = |l: char| (l as u8 - b'A') as usize;
    match module {
        "RAM32M" | "RAM64M" => {
            let small = module == "RAM32M";
            for l in ['A', 'B', 'C', 'D'] {
                let outs: Vec<(String, String)> = if small {
                    [(format!("DO{l}1"), l.to_string()), (format!("DO{l}0"), format!("{l}MUX"))].into_iter().filter_map(|(p, sp)| net(&p).map(|n| (sp, n))).collect()
                } else {
                    net(&format!("DO{l}")).map(|n| (l.to_string(), n)).into_iter().collect()
                };
                if outs.is_empty() && l != 'D' {
                    continue; // nothing reads it: the LUT stays free
                }
                read(&mut r, l, &format!("ADDR{l}"), if small { 5 } else { 6 })?;
                let init = init_bits(attrs, &format!("INIT_{l}"), 64, name)?;
                let (di, dx) = if small { (format!("DI{l}0"), Some(format!("DI{l}1"))) } else { (format!("DI{l}"), None) };
                if let Some(n) = net(&di) {
                    r.ins.push((format!("{l}I"), n, Some(SETUP_DI_PS)));
                    if l != 'D' {
                        r.features.push(format!("{l}LUT.DI1MUX.{l}I"));
                    }
                }
                if let Some(n) = dx.and_then(|p| net(&p)) {
                    r.ins.push((format!("{l}X"), n, Some(SETUP_DI_PS)));
                }
                let reads = lut_pins(l, if small { 5 } else { 6 });
                for (sp, n) in outs {
                    if sp.ends_with("MUX") {
                        r.features.push(format!("{l}OUTMUX.O5"));
                    }
                    r.outs.push((sp, n, reads.clone(), READ_PS));
                }
                r.init[li(l)] = Some(if small {
                    // DOx[0] of address a is INIT_x[2a] (O5 half), DOx[1] is INIT_x[2a+1] (O6 half).
                    (0..32).fold(0u64, |v, a| v | (init[2 * a] as u64) << a | (init[2 * a + 1] as u64) << (32 + a))
                } else {
                    word(&init)
                });
                if small {
                    r.features.push(format!("{l}LUT.SMALL"));
                }
            }
        }
        "RAM32X1S" | "RAM32X1D" | "RAM64X1S" | "RAM64X1D" => {
            let small = module.starts_with("RAM32");
            let n = if small { 5 } else { 6 };
            let init = word(&init_bits(attrs, "INIT", 1 << n, name)?);
            let init = if small { init << 32 } else { init };
            let d = need("D")?;
            let dual = module.ends_with('D');
            let ports: &[(char, &str, &str)] = if dual { &[('D', "A", "SPO"), ('C', "DPRA", "DPO")] } else { &[('D', "A", "O")] };
            for &(l, addr, out) in ports {
                read(&mut r, l, addr, n)?;
                if small {
                    r.features.push(format!("{l}LUT.SMALL"));
                    r.ins.push((format!("{l}X"), d.clone(), Some(SETUP_DI_PS)));
                } else if l == 'D' {
                    r.ins.push(("DI".into(), d.clone(), Some(SETUP_DI_PS)));
                }
                if let Some(o) = net(out) {
                    r.outs.push((l.to_string(), o, lut_pins(l, n), READ_PS));
                }
                r.init[li(l)] = Some(init);
            }
        }
        "RAM128X1S" | "RAM128X1D" | "RAM256X1S" => {
            let deep = module == "RAM256X1S";
            let init = init_bits(attrs, "INIT", if deep { 256 } else { 128 }, name)?;
            let half = |k: usize| word(&init[64 * k..64 * k + 64]);
            r.ins.push(("DI".into(), need("D")?, Some(SETUP_DI_PS)));
            // LUTs at the write address: D (bottom 64), C, then B and A for 256.
            let wl: &[char] = if deep { &['D', 'C', 'B', 'A'] } else { &['D', 'C'] };
            for (k, &l) in wl.iter().enumerate() {
                read(&mut r, l, "A", 6)?;
                r.init[li(l)] = Some(half(k));
            }
            r.ins.push(("CX".into(), need("A6")?, Some(SETUP_WA78_PS)));
            r.features.push("WA7USED".into());
            let wpins: Vec<String> = wl.iter().flat_map(|&l| lut_pins(l, 6)).collect();
            if deep {
                r.ins.push(("AX".into(), need("A6")?, None));
                r.ins.push(("BX".into(), need("A7")?, Some(SETUP_WA78_PS)));
                r.features.push("WA8USED".into());
                r.features.push("BOUTMUX.F8".into());
                let mut reads = wpins;
                reads.extend(["AX", "BX", "CX"].map(String::from));
                if let Some(o) = net("O") {
                    r.outs.push(("BMUX".into(), o, reads, READ_PS + F7_PS + F8_PS));
                }
            } else {
                let spo = if module == "RAM128X1S" { "O" } else { "SPO" };
                r.features.push("COUTMUX.F7".into());
                let mut reads = wpins;
                reads.push("CX".into());
                if let Some(o) = net(spo) {
                    r.outs.push(("CMUX".into(), o, reads, READ_PS + F7_PS));
                }
                if module == "RAM128X1D" {
                    // Second read port: B (bottom 64) and A, at DPRA.
                    for (k, l) in [(0, 'B'), (1, 'A')] {
                        read(&mut r, l, "DPRA", 6)?;
                        r.init[li(l)] = Some(half(k));
                    }
                    r.ins.push(("AX".into(), need("DPRA6")?, None));
                    r.features.push("AOUTMUX.F7".into());
                    let mut reads: Vec<String> = ['A', 'B'].iter().flat_map(|&l| lut_pins(l, 6)).collect();
                    reads.push("AX".into());
                    if let Some(o) = net("DPO") {
                        r.outs.push(("AMUX".into(), o, reads, READ_PS + F7_PS));
                    }
                }
            }
        }
        _ => return Err(format!("pack: {name}: {module} is not a LUT RAM")),
    }
    if r.init.iter().all(|i| i.is_none()) {
        return Err(format!("pack: {module} {name}: no LUT used"));
    }
    Ok(r)
}

/// Place every LUT RAM in a SLICEM of its own, nearest `(x, y)` first.
pub fn place(packed: &crate::pack::Packed7, device: &dyn DeviceQuery, (x, y): (i32, i32), used: &mut HashSet<String>) -> Result<Vec<PlacedLutRam>, String> {
    if packed.lutrams.is_empty() {
        return Ok(Vec::new());
    }
    let slicem = device.slicem_sites();
    let mut free: Vec<(i32, String, String)> = device
        .all_slices()
        .into_iter()
        .filter(|s| slicem.contains(&s.1) && !used.contains(&s.1))
        .map(|(t, s, gx, gy)| ((gx - x).abs() + (gy - y).abs(), t, s))
        .collect();
    free.sort();
    if free.len() < packed.lutrams.len() {
        return Err(format!("place: {} LUT RAMs need SLICEM sites, {} are free", packed.lutrams.len(), free.len()));
    }
    Ok(packed
        .lutrams
        .iter()
        .zip(free)
        .map(|(r, (_, tile, site))| {
            used.insert(site.clone());
            PlacedLutRam { name: r.name.clone(), tile, site }
        })
        .collect())
}

fn site_of<'a>(placed: &'a Placed7, name: &str) -> Option<&'a PlacedLutRam> {
    placed.lutrams.iter().find(|p| p.name == name)
}

/// Routing endpoints: `(net, site, site pin, is_driver)`.
pub fn endpoints(placed: &Placed7) -> Vec<(String, String, String, bool)> {
    let mut v = Vec::new();
    for r in &placed.packed.lutrams {
        let Some(p) = site_of(placed, &r.name) else { continue };
        v.push((r.clk.clone(), p.site.clone(), "CLK".into(), false));
        for (pin, net, _) in &r.ins {
            v.push((net.clone(), p.site.clone(), pin.clone(), false));
        }
        for (pin, net, _, _) in &r.outs {
            v.push((net.clone(), p.site.clone(), pin.clone(), true));
        }
    }
    v
}

/// A LUT RAM output for timing: launched by the clock (`cko_ps`) and
/// reached from the read address pins (`ins`, through `read_ps`).
pub struct ReadArc<'a> {
    pub name: &'a str,
    pub site: &'a str,
    pub clk: &'a str,
    pub ins: Vec<(&'a str, &'a str)>,
    pub read_ps: u32,
    pub cko_ps: u32,
}

/// Every LUT RAM output net with its arc.
pub fn read_arcs(placed: &Placed7) -> HashMap<&str, ReadArc<'_>> {
    let mut m = HashMap::new();
    for r in &placed.packed.lutrams {
        let Some(p) = site_of(placed, &r.name) else { continue };
        for (_, net, reads, read_ps) in &r.outs {
            let ins = r.ins.iter().filter(|(pin, _, _)| reads.contains(pin)).map(|(pin, n, _)| (n.as_str(), pin.as_str())).collect();
            let cko_ps = CKO_PS + read_ps - READ_PS;
            m.insert(net.as_str(), ReadArc { name: &r.name, site: &p.site, clk: &r.clk, ins, read_ps: *read_ps, cko_ps });
        }
    }
    m
}

/// Write-side timing endpoints: `(net, site, site pin, setup ps, clock net)`.
pub fn write_ends(placed: &Placed7) -> Vec<(&str, &str, &str, u32, &str)> {
    let mut v = Vec::new();
    for r in &placed.packed.lutrams {
        let Some(p) = site_of(placed, &r.name) else { continue };
        for (pin, net, setup) in &r.ins {
            if let Some(s) = setup {
                v.push((net.as_str(), p.site.as_str(), pin.as_str(), *s, r.clk.as_str()));
            }
        }
    }
    v
}

/// FASM lines for every placed LUT RAM.
pub fn fasm(placed: &Placed7, device: &dyn crate::emit::EmitDevice) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    for r in &placed.packed.lutrams {
        let p = site_of(placed, &r.name).ok_or_else(|| format!("emit: LUT RAM {} was not placed", r.name))?;
        let rel = device.site_rel(&p.tile, &p.site).ok_or_else(|| format!("emit: no rel site for {} {}", p.tile, p.site))?;
        if !rel.starts_with("SLICEM") {
            return Err(format!("emit: LUT RAM {} is on {} ({rel}), not a SLICEM", r.name, p.site));
        }
        for (i, init) in r.init.iter().enumerate() {
            if let Some(v) = init {
                lines.push(format!("{}.{rel}.{}LUT.INIT[63:0] = 64'h{v:016X}", p.tile, (b'A' + i as u8) as char));
            }
        }
        for f in &r.features {
            lines.push(format!("{}.{rel}.{f}", p.tile));
        }
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_ir::{CellKind, Design};

    fn cell(module: &str, conns: &[(&str, &str)], attrs: &[(&str, &str)]) -> PackedLutRam {
        let mut d = Design::new("t");
        d.add_cell("u", CellKind::BlackBox { module: helion_ir::intern(module) });
        for (k, v) in attrs {
            d.cells.last_mut().unwrap().attrs.set(*k, *v);
        }
        for (pin, net) in conns {
            d.connect(net, "u", pin);
        }
        let pins = d.pin_index();
        pack("u", module, &pins, &d.cells[0].attrs).unwrap()
    }

    fn feats(r: &PackedLutRam) -> Vec<&str> {
        let mut f: Vec<&str> = r.features.iter().map(|s| s.as_str()).collect();
        f.sort();
        f
    }

    #[test]
    fn ram32m_matches_vivado() {
        // Vivado's u_32m of lutram_prims: INIT_A 0123456789ABCDEF gives ALUT 0505AFAF_11BB11BB.
        let mut c = vec![("WCLK", "clk"), ("WE", "we")];
        let names: Vec<(String, String)> = ['A', 'B', 'C', 'D']
            .iter()
            .flat_map(|l| {
                (0..5).map(move |i| (format!("ADDR{l}{i}"), format!("a{l}{i}"))).chain((0..2).flat_map(move |j| [(format!("DI{l}{j}"), format!("d{l}{j}")), (format!("DO{l}{j}"), format!("q{l}{j}"))]))
            })
            .collect();
        c.extend(names.iter().map(|(a, b)| (a.as_str(), b.as_str())));
        let r = cell("RAM32M", &c, &[("INIT_A", "64'h0123456789ABCDEF"), ("INIT_D", "64'h8796A5B4C3D2E1F0")]);
        assert_eq!(r.init[0], Some(0x0505_AFAF_11BB_11BB));
        assert_eq!(r.init[3], Some(0x99CC_99CC_3636_9C9C));
        let f = feats(&r);
        for x in ["ALUT.DI1MUX.AI", "ALUT.RAM", "ALUT.SMALL", "AOUTMUX.O5", "DLUT.RAM", "DLUT.SMALL", "DOUTMUX.O5", "WEMUX.CE"] {
            assert!(f.contains(&x), "{x} missing from {f:?}");
        }
        assert!(!f.iter().any(|x| x.starts_with("DLUT.DI1MUX")));
        let pin = |p: &str| r.ins.iter().find(|x| x.0 == p).map(|x| x.1.as_str());
        assert_eq!((pin("AI"), pin("AX"), pin("DX"), pin("D5"), pin("A6")), (Some("dA0"), Some("dA1"), Some("dD1"), Some("aD4"), None));
        assert!(r.outs.iter().any(|o| o.0 == "AMUX" && o.1 == "qA0") && r.outs.iter().any(|o| o.0 == "A" && o.1 == "qA1"));
    }

    #[test]
    fn deep_rams_use_the_wide_muxes() {
        let mut c: Vec<(String, String)> = (0..8).map(|i| (format!("A{i}"), format!("a{i}"))).collect();
        c.extend([("D".into(), "d".into()), ("WCLK".into(), "clk".into()), ("WE".into(), "we".into()), ("O".into(), "q".into())]);
        let c: Vec<(&str, &str)> = c.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let init = "256'h0123456789ABCDEF1032547698BADCFE23016745AB89EFCD32107654BA98FEDC";
        let r = cell("RAM256X1S", &c, &[("INIT", init)]);
        assert_eq!(r.init, [Some(0x0123_4567_89AB_CDEF), Some(0x1032_5476_98BA_DCFE), Some(0x2301_6745_AB89_EFCD), Some(0x3210_7654_BA98_FEDC)]);
        assert_eq!(feats(&r), ["ALUT.RAM", "BLUT.RAM", "BOUTMUX.F8", "CLUT.RAM", "DLUT.RAM", "WA7USED", "WA8USED", "WEMUX.CE"]);
        let pin = |p: &str| r.ins.iter().find(|x| x.0 == p).map(|x| x.1.as_str());
        assert_eq!((pin("AX"), pin("CX"), pin("BX"), pin("DI")), (Some("a6"), Some("a6"), Some("a7"), Some("d")));
        assert_eq!(r.outs[0].0, "BMUX");
    }
}

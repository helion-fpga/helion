//! Design checkpoints (`.hck2`): full restorable snapshots of the design
//! database at a flow stage (Vivado `write_checkpoint` / `open_checkpoint`).
//!
//! Unlike the v1 `.hckp` format (HNF + knobs, re-impls on open), a v2
//! checkpoint stores the *exact* implementation state — netlist, constraints,
//! packed clusters, placement, routing, and bitstream — so restoring is a pure
//! deserialization and re-running reports yields bit-identical numbers.
//!
//! # Format (all integers little-endian)
//! ```text
//! magic      "HCK2" (4 bytes)
//! ver        u8 = 1
//! header_len u32
//! header:
//!   name         str   user name ("auto-post-route-1728..." or "baseline")
//!   created_unix u64   seconds since epoch
//!   stage        u8    1=post-synth 2=post-pack 3=post-place 4=post-route
//!   part         str   e.g. "HL10T-C32-1"
//!   strategy     u8    ImplStrategy discriminant
//!   wns_ps       i64   timing summary captured at save
//!   tns_ps       i64
//!   luts/ffs/iobs/dsps/brams u32 each
//! sections: repeated (tag u8, len u32, bytes)
//!   1 = HNF design text (UTF-8)
//!   2 = Constraints binary blob (helion_sta::Constraints::encode)
//!   3 = Packed clusters
//!   4 = Placed (embeds its Packed; section 3 omitted)
//!   5 = Routed (embeds Placed+Packed; sections 3/4 omitted)
//!   6 = Bitstream frames (only when bitgen ran)
//! ```
//! Strings are `u32 len + bytes`; lists are `u32 count + items`. Decoding is
//! total: malformed input returns `Err`, never panics.

use super::{ImplStrategy, Mode, PrjPblock, Session};
use helion_bits::Bitstream;
use helion_device::{Device, Site, SiteKind};
use helion_ir::Design;
use helion_pack::{Packed, PackedBram, PackedIob, PackedLutFf, PackedMac};
use helion_place::Placed;
use helion_route::{ImuxRoute, IobRoute, Routed};
use helion_sta::Constraints;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 4] = b"HCK2";
const VER: u8 = 1;

const TAG_HNF: u8 = 1;
const TAG_CONSTRAINTS: u8 = 2;
const TAG_PACKED: u8 = 3;
const TAG_PLACED: u8 = 4;
const TAG_ROUTED: u8 = 5;
const TAG_BITSTREAM: u8 = 6;

/// Flow stage a checkpoint was captured at.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckpointStage {
    PostSynth = 1,
    PostPack = 2,
    PostPlace = 3,
    PostRoute = 4,
}

impl CheckpointStage {
    pub fn label(self) -> &'static str {
        match self {
            Self::PostSynth => "post-synth",
            Self::PostPack => "post-pack",
            Self::PostPlace => "post-place",
            Self::PostRoute => "post-route",
        }
    }

    fn from_u8(v: u8) -> Result<Self, String> {
        match v {
            1 => Ok(Self::PostSynth),
            2 => Ok(Self::PostPack),
            3 => Ok(Self::PostPlace),
            4 => Ok(Self::PostRoute),
            _ => Err(format!("checkpoint: bad stage {v}")),
        }
    }
}

/// Timing summary captured at save time (for list display; reports are
/// re-derived from restored state, not stored).
#[derive(Clone, Copy, Debug, Default)]
pub struct CheckpointSummary {
    pub wns_ps: i64,
    pub tns_ps: i64,
}

/// Header-only view of a checkpoint file: cheap to list.
#[derive(Clone, Debug)]
pub struct CheckpointMeta {
    pub name: String,
    pub path: PathBuf,
    pub stage: CheckpointStage,
    pub created_unix: u64,
    pub part: String,
    pub strategy: ImplStrategy,
    pub luts: u32,
    pub ffs: u32,
    pub iobs: u32,
    pub dsps: u32,
    pub brams: u32,
    pub wns_ps: i64,
    pub tns_ps: i64,
    pub bytes: u64,
}

impl CheckpointMeta {
    pub fn cell_total(&self) -> u32 {
        self.luts
            .saturating_add(self.ffs)
            .saturating_add(self.iobs)
            .saturating_add(self.dsps)
            .saturating_add(self.brams)
    }

    /// Timing numbers (WNS/TNS) are only meaningful at post-route; earlier
    /// stages have no routed timing to report.
    pub fn has_timing(&self) -> bool {
        self.stage == CheckpointStage::PostRoute
    }
}

/// A restored checkpoint: session ready to use plus its constraints.
pub struct RestoredCheckpoint {
    pub session: Session,
    pub constraints: Constraints,
    pub meta: CheckpointMeta,
}

/// Numeric deltas between two checkpoints (b minus a).
#[derive(Clone, Debug, Default)]
pub struct CheckpointDiff {
    pub a_name: String,
    pub b_name: String,
    pub d_luts: i64,
    pub d_ffs: i64,
    pub d_iobs: i64,
    pub d_dsps: i64,
    pub d_brams: i64,
    pub d_wns_ps: i64,
    pub d_tns_ps: i64,
}

impl CheckpointDiff {
    fn row(label: &str, d: i64, unit: &str, out: &mut String) {
        let sign = if d >= 0 { "+" } else { "" };
        out.push_str(&format!("{label:<8} {sign}{d} {unit}\n"));
    }

    pub fn text(&self) -> String {
        let mut out = format!("diff {} -> {}\n", self.a_name, self.b_name);
        Self::row("LUT", self.d_luts, "", &mut out);
        Self::row("FF", self.d_ffs, "", &mut out);
        Self::row("IOB", self.d_iobs, "", &mut out);
        Self::row("DSP", self.d_dsps, "", &mut out);
        Self::row("BRAM", self.d_brams, "", &mut out);
        Self::row("WNS", self.d_wns_ps, "ps", &mut out);
        Self::row("TNS", self.d_tns_ps, "ps", &mut out);
        out
    }
}

pub fn diff_metas(a: &CheckpointMeta, b: &CheckpointMeta) -> CheckpointDiff {
    CheckpointDiff {
        a_name: a.name.clone(),
        b_name: b.name.clone(),
        d_luts: b.luts as i64 - a.luts as i64,
        d_ffs: b.ffs as i64 - a.ffs as i64,
        d_iobs: b.iobs as i64 - a.iobs as i64,
        d_dsps: b.dsps as i64 - a.dsps as i64,
        d_brams: b.brams as i64 - a.brams as i64,
        d_wns_ps: b.wns_ps - a.wns_ps,
        d_tns_ps: b.tns_ps - a.tns_ps,
    }
}

// ---------------------------------------------------------------------------
// low-level codec
// ---------------------------------------------------------------------------

struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    fn new() -> Self {
        Self { buf: Vec::new() }
    }
    fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn i64(&mut self, v: i64) {
        self.u64(v as u64);
    }
    fn f64(&mut self, v: f64) {
        self.u64(v.to_bits());
    }
    fn str(&mut self, s: &str) {
        let b = s.as_bytes();
        self.u32(b.len() as u32);
        self.buf.extend_from_slice(b);
    }
    fn opt_str(&mut self, s: &Option<String>) {
        match s {
            Some(v) => {
                self.u8(1);
                self.str(v);
            }
            None => self.u8(0),
        }
    }
    fn bytes(&mut self, b: &[u8]) {
        self.u32(b.len() as u32);
        self.buf.extend_from_slice(b);
    }
    fn section(&mut self, tag: u8, payload: &[u8]) {
        self.u8(tag);
        self.u32(payload.len() as u32);
        self.buf.extend_from_slice(payload);
    }
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.pos.checked_add(n).ok_or("checkpoint: length overflow")?;
        if end > self.buf.len() {
            return Err(format!(
                "checkpoint: truncated (need {n} bytes at offset {})",
                self.pos
            ));
        }
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, String> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Result<u64, String> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(u64::from_le_bytes(a))
    }
    fn i64(&mut self) -> Result<i64, String> {
        Ok(self.u64()? as i64)
    }
    fn f64(&mut self) -> Result<f64, String> {
        Ok(f64::from_bits(self.u64()?))
    }
    fn str(&mut self) -> Result<String, String> {
        let n = self.u32()? as usize;
        if n > 1 << 26 {
            return Err("checkpoint: string too long".into());
        }
        let b = self.take(n)?;
        std::str::from_utf8(b)
            .map(|s| s.to_string())
            .map_err(|e| format!("checkpoint: bad utf8: {e}"))
    }
    fn opt_str(&mut self) -> Result<Option<String>, String> {
        match self.u8()? {
            0 => Ok(None),
            1 => Ok(Some(self.str()?)),
            v => Err(format!("checkpoint: bad option tag {v}")),
        }
    }
    fn raw_bytes(&mut self) -> Result<&'a [u8], String> {
        let n = self.u32()? as usize;
        if n > 1 << 30 {
            return Err("checkpoint: blob too long".into());
        }
        self.take(n)
    }
    fn end(&self) -> Result<(), String> {
        if self.pos == self.buf.len() {
            Ok(())
        } else {
            Err(format!("checkpoint: {} trailing bytes", self.buf.len() - self.pos))
        }
    }
}

fn enc_site(w: &mut Writer, s: &Site) {
    w.u32(s.x);
    w.u32(s.y);
    w.u8(match s.kind {
        SiteKind::Clb => 0,
        SiteKind::Iob => 1,
        SiteKind::Clk => 2,
        SiteKind::Dsp => 3,
        SiteKind::Bram => 4,
    });
}

fn dec_site(r: &mut Reader) -> Result<Site, String> {
    let x = r.u32()?;
    let y = r.u32()?;
    let kind = match r.u8()? {
        0 => SiteKind::Clb,
        1 => SiteKind::Iob,
        2 => SiteKind::Clk,
        3 => SiteKind::Dsp,
        4 => SiteKind::Bram,
        v => return Err(format!("checkpoint: bad site kind {v}")),
    };
    Ok(Site { x, y, kind })
}

fn enc_packed(w: &mut Writer, p: &Packed) {
    w.u32(p.lutffs.len() as u32);
    for lf in &p.lutffs {
        w.str(&lf.lut_cell);
        w.str(&lf.ff_cell);
        w.u64(lf.init);
        w.u32(lf.lut_pins.len() as u32);
        for (pin, cell) in &lf.lut_pins {
            w.u8(*pin);
            w.str(cell);
        }
        w.str(&lf.q_net);
    }
    w.u32(p.iobs.len() as u32);
    for io in &p.iobs {
        w.str(&io.cell);
        w.str(&io.from_net);
        w.opt_str(&io.loc);
        w.opt_str(&io.drive);
        w.opt_str(&io.slew);
        w.opt_str(&io.pulltype);
        w.opt_str(&io.diff_term);
        w.opt_str(&io.in_term);
    }
    w.u32(p.macs.len() as u32);
    for m in &p.macs {
        w.str(&m.cell);
    }
    w.u32(p.brams.len() as u32);
    for b in &p.brams {
        w.str(&b.cell);
        w.u32(b.init.len() as u32);
        for word in &b.init {
            w.u64(*word);
        }
    }
}

fn dec_packed(r: &mut Reader) -> Result<Packed, String> {
    let n = r.u32()? as usize;
    let mut lutffs = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        let lut_cell = r.str()?;
        let ff_cell = r.str()?;
        let init = r.u64()?;
        let np = r.u32()? as usize;
        let mut lut_pins = Vec::with_capacity(np.min(8));
        for _ in 0..np {
            lut_pins.push((r.u8()?, r.str()?));
        }
        let q_net = r.str()?;
        lutffs.push(PackedLutFf {
            lut_cell,
            ff_cell,
            init,
            lut_pins,
            q_net,
        });
    }
    let n = r.u32()? as usize;
    let mut iobs = Vec::with_capacity(n.min(1 << 12));
    for _ in 0..n {
        iobs.push(PackedIob {
            cell: r.str()?,
            from_net: r.str()?,
            loc: r.opt_str()?,
            drive: r.opt_str()?,
            slew: r.opt_str()?,
            pulltype: r.opt_str()?,
            diff_term: r.opt_str()?,
            in_term: r.opt_str()?,
        });
    }
    let n = r.u32()? as usize;
    let mut macs = Vec::with_capacity(n.min(1 << 10));
    for _ in 0..n {
        macs.push(PackedMac { cell: r.str()? });
    }
    let n = r.u32()? as usize;
    let mut brams = Vec::with_capacity(n.min(1 << 10));
    for _ in 0..n {
        let cell = r.str()?;
        let ni = r.u32()? as usize;
        let mut init = Vec::with_capacity(ni.min(1 << 12));
        for _ in 0..ni {
            init.push(r.u64()?);
        }
        brams.push(PackedBram { cell, init });
    }
    Ok(Packed {
        lutffs,
        iobs,
        macs,
        brams,
    })
}

fn enc_placed(w: &mut Writer, p: &Placed) {
    let mut pw = Writer::new();
    enc_packed(&mut pw, &p.packed);
    w.bytes(&pw.buf);
    w.u32(p.lutff_sites.len() as u32);
    for (site, ble) in &p.lutff_sites {
        enc_site(w, site);
        w.u8(*ble);
    }
    w.u32(p.iob_sites.len() as u32);
    for s in &p.iob_sites {
        enc_site(w, s);
    }
    w.u32(p.mac_sites.len() as u32);
    for s in &p.mac_sites {
        enc_site(w, s);
    }
    w.u32(p.bram_sites.len() as u32);
    for s in &p.bram_sites {
        enc_site(w, s);
    }
    w.f64(p.timing_weight);
    w.f64(p.cost);
}

fn dec_placed(r: &mut Reader) -> Result<Placed, String> {
    let packed_bytes = r.raw_bytes()?;
    let mut pr = Reader::new(packed_bytes);
    let packed = dec_packed(&mut pr)?;
    pr.end()?;
    let n = r.u32()? as usize;
    let mut lutff_sites = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        lutff_sites.push((dec_site(r)?, r.u8()?));
    }
    let n = r.u32()? as usize;
    let mut iob_sites = Vec::with_capacity(n.min(1 << 12));
    for _ in 0..n {
        iob_sites.push(dec_site(r)?);
    }
    let n = r.u32()? as usize;
    let mut mac_sites = Vec::with_capacity(n.min(1 << 10));
    for _ in 0..n {
        mac_sites.push(dec_site(r)?);
    }
    let n = r.u32()? as usize;
    let mut bram_sites = Vec::with_capacity(n.min(1 << 10));
    for _ in 0..n {
        bram_sites.push(dec_site(r)?);
    }
    let timing_weight = r.f64()?;
    let cost = r.f64()?;
    Ok(Placed {
        packed,
        lutff_sites,
        iob_sites,
        mac_sites,
        bram_sites,
        timing_weight,
        cost,
    })
}

fn enc_iob_route(w: &mut Writer, ir: &IobRoute) {
    w.u32(ir.iob.0);
    w.u32(ir.iob.1);
    w.u32(ir.clb.0);
    w.u32(ir.clb.1);
    w.u8(ir.ble);
    w.u32(ir.hops);
    w.i64(ir.delay_ps);
    w.u32(ir.path.len() as u32);
    for (x, y) in &ir.path {
        w.u32(*x);
        w.u32(*y);
    }
    w.str(&ir.net);
    w.u8(ir.from_ff as u8);
}

fn dec_iob_route(r: &mut Reader) -> Result<IobRoute, String> {
    let iob = (r.u32()?, r.u32()?);
    let clb = (r.u32()?, r.u32()?);
    let ble = r.u8()?;
    let hops = r.u32()?;
    let delay_ps = r.i64()?;
    let n = r.u32()? as usize;
    let mut path = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        path.push((r.u32()?, r.u32()?));
    }
    let net = r.str()?;
    let from_ff = match r.u8()? {
        0 => false,
        1 => true,
        v => return Err(format!("checkpoint: bad from_ff {v}")),
    };
    Ok(IobRoute {
        iob,
        clb,
        ble,
        hops,
        delay_ps,
        path,
        net,
        from_ff,
    })
}

fn enc_routed(w: &mut Writer, rt: &Routed) {
    let mut pw = Writer::new();
    enc_placed(&mut pw, &rt.placed);
    w.bytes(&pw.buf);
    w.u32(rt.iob_src.len() as u32);
    for ir in &rt.iob_src {
        enc_iob_route(w, ir);
    }
    w.u32(rt.imux.len() as u32);
    for im in &rt.imux {
        w.u32(im.x);
        w.u32(im.y);
        w.u32(im.mux);
        w.u8(im.sel);
    }
    w.u32(rt.pathfinder_iters);
    w.u32(rt.overused);
    w.u32(rt.imux_skip);
}

fn dec_routed(r: &mut Reader) -> Result<Routed, String> {
    let placed_bytes = r.raw_bytes()?;
    let mut pr = Reader::new(placed_bytes);
    let placed = dec_placed(&mut pr)?;
    pr.end()?;
    let n = r.u32()? as usize;
    let mut iob_src = Vec::with_capacity(n.min(1 << 14));
    for _ in 0..n {
        iob_src.push(dec_iob_route(r)?);
    }
    let n = r.u32()? as usize;
    let mut imux = Vec::with_capacity(n.min(1 << 16));
    for _ in 0..n {
        imux.push(ImuxRoute {
            x: r.u32()?,
            y: r.u32()?,
            mux: r.u32()?,
            sel: r.u8()?,
        });
    }
    let pathfinder_iters = r.u32()?;
    let overused = r.u32()?;
    let imux_skip = r.u32()?;
    Ok(Routed {
        placed,
        iob_src,
        imux,
        pathfinder_iters,
        overused,
        imux_skip,
        // The checkpoint format predates the congestion export and is
        // version-locked; the heat map stays empty until a fresh route run.
        congestion: Vec::new(),
        congestion_x0: 0,
        congestion_y0: 0,
        congestion_cols: 0,
        congestion_rows: 0,
    })
}

fn enc_bitstream(w: &mut Writer, b: &Bitstream) {
    w.u32(b.idcode);
    w.u32(b.frames.len() as u32);
    for ((bl, maj, min), word) in &b.frames {
        w.u8(*bl);
        w.u32(*maj as u32);
        w.u8(*min);
        w.u64((*word & 0xFFFF_FFFF_FFFF_FFFF) as u64);
        w.u64((word >> 64) as u64);
    }
    w.bytes(&b.packets);
}

fn dec_bitstream(r: &mut Reader) -> Result<Bitstream, String> {
    let idcode = r.u32()?;
    let n = r.u32()? as usize;
    let mut frames = BTreeMap::new();
    for _ in 0..n {
        let bl = r.u8()?;
        let maj = r.u32()? as u16;
        let min = r.u8()?;
        let lo = r.u64()? as u128;
        let hi = r.u64()? as u128;
        frames.insert((bl, maj, min), lo | (hi << 64));
    }
    let packets = r.raw_bytes()?.to_vec();
    Ok(Bitstream {
        idcode,
        frames,
        packets,
    })
}

// ---------------------------------------------------------------------------
// stage / counts
// ---------------------------------------------------------------------------

/// Highest completed stage in the session.
pub fn stage_of(session: &Session) -> Option<CheckpointStage> {
    if session.routed.is_some() {
        Some(CheckpointStage::PostRoute)
    } else if session.placed.is_some() {
        Some(CheckpointStage::PostPlace)
    } else if session.packed.is_some() {
        Some(CheckpointStage::PostPack)
    } else if session.design.is_some() {
        Some(CheckpointStage::PostSynth)
    } else {
        None
    }
}

fn counts_of(session: &Session) -> (u32, u32, u32, u32, u32) {
    if let Some(p) = &session.packed {
        let ffs = p.lutffs.iter().filter(|lf| !lf.ff_cell.is_empty()).count() as u32;
        return (
            p.lutffs.len() as u32,
            ffs,
            p.iobs.len() as u32,
            p.macs.len() as u32,
            p.brams.len() as u32,
        );
    }
    if let Some(d) = &session.design {
        let mut luts = 0u32;
        let mut ffs = 0u32;
        let mut iobs = 0u32;
        let mut dsps = 0u32;
        let mut brams = 0u32;
        for c in &d.cells {
            match c.kind {
                helion_ir::CellKind::Lut6 { .. } => luts += 1,
                helion_ir::CellKind::Hff => ffs += 1,
                helion_ir::CellKind::IobOut => iobs += 1,
                helion_ir::CellKind::Mac27 => dsps += 1,
                helion_ir::CellKind::Bram18 => brams += 1,
                _ => {}
            }
        }
        return (luts, ffs, iobs, dsps, brams);
    }
    (0, 0, 0, 0, 0)
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn sanitize_name(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    if s.trim().is_empty() {
        s = "checkpoint".into();
    }
    s
}

// ---------------------------------------------------------------------------
// save / load / list / delete
// ---------------------------------------------------------------------------

/// Serialize the session at its current stage into `dir/{name}.hck2`.
pub fn save_checkpoint(
    session: &Session,
    constraints: &Constraints,
    name: &str,
    dir: &Path,
    summary: &CheckpointSummary,
) -> Result<PathBuf, String> {
    let stage = stage_of(session).ok_or("checkpoint: nothing to save (no design)")?;
    let design = session.design.as_ref().ok_or("checkpoint: no design")?;
    let created = now_unix();
    let stem = sanitize_name(name);
    std::fs::create_dir_all(dir).map_err(|e| format!("checkpoint mkdir {}: {e}", dir.display()))?;
    let mut path = dir.join(format!("{stem}.hck2"));
    if path.exists() {
        path = dir.join(format!("{stem}-{created}.hck2"));
    }
    let (luts, ffs, iobs, dsps, brams) = counts_of(session);

    let mut w = Writer::new();
    w.buf.extend_from_slice(MAGIC);
    w.u8(VER);
    let mut h = Writer::new();
    h.str(&stem);
    h.u64(created);
    h.u8(stage as u8);
    h.str(&session.part);
    h.u8(match session.strategy {
        ImplStrategy::Default => 0,
        ImplStrategy::TimingExplore => 1,
        ImplStrategy::RuntimeOpt => 2,
        ImplStrategy::PhysOpt => 3,
    });
    h.i64(summary.wns_ps);
    h.i64(summary.tns_ps);
    h.u32(luts);
    h.u32(ffs);
    h.u32(iobs);
    h.u32(dsps);
    h.u32(brams);
    w.u32(h.buf.len() as u32);
    w.buf.extend_from_slice(&h.buf);

    w.section(TAG_HNF, design.to_hnf().as_bytes());
    w.section(TAG_CONSTRAINTS, &constraints.encode());
    match stage {
        CheckpointStage::PostSynth => {}
        CheckpointStage::PostPack => {
            let p = session.packed.as_ref().ok_or("checkpoint: no packed")?;
            let mut pw = Writer::new();
            enc_packed(&mut pw, p);
            w.section(TAG_PACKED, &pw.buf);
        }
        CheckpointStage::PostPlace => {
            let p = session.placed.as_ref().ok_or("checkpoint: no placed")?;
            let mut pw = Writer::new();
            enc_placed(&mut pw, p);
            w.section(TAG_PLACED, &pw.buf);
        }
        CheckpointStage::PostRoute => {
            let rt = session.routed.as_ref().ok_or("checkpoint: no routed")?;
            let mut rw = Writer::new();
            enc_routed(&mut rw, rt);
            w.section(TAG_ROUTED, &rw.buf);
            if let Some(b) = &session.bitstream {
                let mut bw = Writer::new();
                enc_bitstream(&mut bw, b);
                w.section(TAG_BITSTREAM, &bw.buf);
            }
        }
    }
    std::fs::write(&path, &w.buf).map_err(|e| format!("checkpoint write {}: {e}", path.display()))?;
    Ok(path)
}

struct ParsedHeader {
    name: String,
    created_unix: u64,
    stage: CheckpointStage,
    part: String,
    strategy: ImplStrategy,
    wns_ps: i64,
    tns_ps: i64,
    luts: u32,
    ffs: u32,
    iobs: u32,
    dsps: u32,
    brams: u32,
}

fn parse_header(bytes: &[u8]) -> Result<(ParsedHeader, Vec<(u8, Vec<u8>)>), String> {
    let mut r = Reader::new(bytes);
    if r.take(4)? != MAGIC {
        return Err("checkpoint: bad magic (not .hck2)".into());
    }
    let ver = r.u8()?;
    if ver != VER {
        return Err(format!("checkpoint: unsupported version {ver}"));
    }
    let hlen = r.u32()? as usize;
    let hbytes = r.take(hlen)?;
    let mut h = Reader::new(hbytes);
    let hdr = ParsedHeader {
        name: h.str()?,
        created_unix: h.u64()?,
        stage: CheckpointStage::from_u8(h.u8()?)?,
        part: h.str()?,
        strategy: match h.u8()? {
            1 => ImplStrategy::TimingExplore,
            2 => ImplStrategy::RuntimeOpt,
            3 => ImplStrategy::PhysOpt,
            _ => ImplStrategy::Default,
        },
        wns_ps: h.i64()?,
        tns_ps: h.i64()?,
        luts: h.u32()?,
        ffs: h.u32()?,
        iobs: h.u32()?,
        dsps: h.u32()?,
        brams: h.u32()?,
    };
    h.end()?;
    let mut sections = Vec::new();
    while r.pos < r.buf.len() {
        let tag = r.u8()?;
        let payload = r.raw_bytes()?.to_vec();
        sections.push((tag, payload));
    }
    Ok((hdr, sections))
}

/// Read only the header: cheap listing without parsing the design.
pub fn read_meta(path: &Path) -> Result<CheckpointMeta, String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("checkpoint read {}: {e}", path.display()))?;
    let (hdr, _) = parse_header(&bytes)?;
    Ok(CheckpointMeta {
        name: hdr.name,
        path: path.to_path_buf(),
        stage: hdr.stage,
        created_unix: hdr.created_unix,
        part: hdr.part,
        strategy: hdr.strategy,
        luts: hdr.luts,
        ffs: hdr.ffs,
        iobs: hdr.iobs,
        dsps: hdr.dsps,
        brams: hdr.brams,
        wns_ps: hdr.wns_ps,
        tns_ps: hdr.tns_ps,
        bytes: bytes.len() as u64,
    })
}

/// List checkpoints in `dir`, newest first. Unreadable files are skipped.
pub fn list_checkpoints(dir: &Path) -> Vec<CheckpointMeta> {
    let mut out = Vec::new();
    let entries = std::fs::read_dir(dir).map(|rd| rd.filter_map(|e| e.ok()).collect::<Vec<_>>());
    let mut entries = match entries {
        Ok(e) => e,
        Err(_) => return out,
    };
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("hck2") {
            continue;
        }
        if let Ok(m) = read_meta(&p) {
            out.push(m);
        }
    }
    out.sort_by(|a, b| b.created_unix.cmp(&a.created_unix));
    out
}

pub fn delete_checkpoint(path: &Path) -> Result<(), String> {
    std::fs::remove_file(path).map_err(|e| format!("checkpoint delete {}: {e}", path.display()))
}

/// Restore a checkpoint into a live `Session`. The part must match `dev`.
/// Re-running reports on the result yields identical numbers to the original.
pub fn load_checkpoint(path: &Path, dev: &Device) -> Result<RestoredCheckpoint, String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("checkpoint read {}: {e}", path.display()))?;
    let (hdr, sections) = parse_header(&bytes)?;
    if hdr.part != dev.part {
        return Err(format!(
            "checkpoint part {} != device part {}",
            hdr.part, dev.part
        ));
    }
    let mut hnf: Option<String> = None;
    let mut constraints: Option<Constraints> = None;
    let mut packed: Option<Packed> = None;
    let mut placed: Option<Placed> = None;
    let mut routed: Option<Routed> = None;
    let mut bitstream: Option<Bitstream> = None;
    for (tag, payload) in &sections {
        match tag {
            &TAG_HNF => {
                hnf = Some(
                    std::str::from_utf8(payload)
                        .map_err(|e| format!("checkpoint: bad HNF utf8: {e}"))?
                        .to_string(),
                );
            }
            &TAG_CONSTRAINTS => constraints = Some(Constraints::decode(payload)?),
            &TAG_PACKED => {
                let mut r = Reader::new(payload);
                let p = dec_packed(&mut r)?;
                r.end()?;
                packed = Some(p);
            }
            &TAG_PLACED => {
                let mut r = Reader::new(payload);
                let p = dec_placed(&mut r)?;
                r.end()?;
                placed = Some(p);
            }
            &TAG_ROUTED => {
                let mut r = Reader::new(payload);
                let rt = dec_routed(&mut r)?;
                r.end()?;
                routed = Some(rt);
            }
            &TAG_BITSTREAM => {
                let mut r = Reader::new(payload);
                let b = dec_bitstream(&mut r)?;
                r.end()?;
                bitstream = Some(b);
            }
            _ => {}
        }
    }
    let hnf = hnf.ok_or("checkpoint: missing HNF section")?;
    let design = Design::from_hnf(&hnf)?;
    let constraints = constraints.unwrap_or_default();

    // Derive lower stages from the highest present section.
    if let Some(rt) = &routed {
        placed = Some(rt.placed.clone());
    }
    if let Some(p) = &placed {
        packed = Some(p.packed.clone());
    }

    let mut session = Session::new(Mode::Project);
    session.design = Some(design);
    session.packed = packed;
    session.placed = placed.clone();
    session.routed = routed;
    session.bitstream = bitstream;
    session.impl_checkpoint = placed;
    session.part = hdr.part.clone();
    session.strategy = hdr.strategy;
    let _ = pblock_unused();
    Ok(RestoredCheckpoint {
        meta: CheckpointMeta {
            name: hdr.name,
            path: path.to_path_buf(),
            stage: hdr.stage,
            created_unix: hdr.created_unix,
            part: hdr.part,
            strategy: hdr.strategy,
            luts: hdr.luts,
            ffs: hdr.ffs,
            iobs: hdr.iobs,
            dsps: hdr.dsps,
            brams: hdr.brams,
            wns_ps: hdr.wns_ps,
            tns_ps: hdr.tns_ps,
            bytes: bytes.len() as u64,
        },
        session,
        constraints,
    })
}

/// The v2 format intentionally does not persist `pblock` bounds: floorplan
/// rectangles are project state, not implementation state. Kept as a named
/// hook so a future version can add a section without touching call sites.
fn pblock_unused() {}

#[allow(dead_code)]
fn _pblock_ref(pb: &PrjPblock) -> String {
    pb.name.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_sta::report_timing_routed_xdc;

    fn test_dev() -> Device {
        Device::load_part("HL10T-C32-1").expect("test part")
    }

    fn counter_session() -> (Session, Device) {
        let dev = test_dev();
        let mut s = Session::new(Mode::Project);
        let sv = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/counter.sv");
        let d = helion_sv::synth_sv_path(&sv).expect("synth counter.sv");
        s.synth_design(d);
        (s, dev)
    }

    fn test_clocks() -> Vec<helion_sta::Clock> {
        let mut clks = Vec::new();
        helion_sta::create_clock(&mut clks, "clk", 10_000, "clk");
        clks
    }

    #[test]
    fn stage_detection() {
        let (mut s, dev) = counter_session();
        assert_eq!(stage_of(&s), Some(CheckpointStage::PostSynth));
        s.place_design(&dev).expect("place");
        assert_eq!(stage_of(&s), Some(CheckpointStage::PostPlace));
        s.route_design_with(&dev, Default::default()).expect("route");
        assert_eq!(stage_of(&s), Some(CheckpointStage::PostRoute));
    }

    #[test]
    fn round_trip_post_synth() {
        let (s, dev) = counter_session();
        let dir = std::env::temp_dir().join("hck2-test-synth");
        let _ = std::fs::remove_dir_all(&dir);
        let c = Constraints::default();
        let path = save_checkpoint(
            &s,
            &c,
            "synth-a",
            &dir,
            &CheckpointSummary::default(),
        )
        .expect("save");
        let meta = read_meta(&path).expect("meta");
        assert_eq!(meta.stage, CheckpointStage::PostSynth);
        assert_eq!(meta.name, "synth-a");
        assert!(meta.luts > 0);
        let rc = load_checkpoint(&path, &dev).expect("load");
        assert_eq!(rc.session.design.as_ref().unwrap().cells.len(), s.design.as_ref().unwrap().cells.len());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn round_trip_post_pack() {
        let (mut s, dev) = counter_session();
        // Pack and place are fused in the Session API; build a pure post-pack
        // state by hand to exercise the TAG_PACKED section.
        let d = s.design.clone().unwrap();
        let packed = helion_pack::pack(&d, &dev).expect("pack");
        let n_lutff = packed.lutffs.len();
        let n_iob = packed.iobs.len();
        s.packed = Some(packed);
        assert_eq!(stage_of(&s), Some(CheckpointStage::PostPack));

        let dir = std::env::temp_dir().join("hck2-test-pack");
        let _ = std::fs::remove_dir_all(&dir);
        let path = save_checkpoint(
            &s,
            &Constraints::default(),
            "pack-a",
            &dir,
            &CheckpointSummary::default(),
        )
        .expect("save");
        let meta = read_meta(&path).expect("meta");
        assert_eq!(meta.stage, CheckpointStage::PostPack);
        assert_eq!(meta.luts as usize, n_lutff);

        let rc = load_checkpoint(&path, &dev).expect("load");
        assert_eq!(stage_of(&rc.session), Some(CheckpointStage::PostPack));
        let rp = rc.session.packed.as_ref().expect("packed restored");
        assert_eq!(rp.lutffs.len(), n_lutff);
        assert_eq!(rp.iobs.len(), n_iob);
        // Placed/routed must NOT be present at this stage.
        assert!(rc.session.placed.is_none());
        assert!(rc.session.routed.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn round_trip_post_place() {
        let (mut s, dev) = counter_session();
        s.place_design(&dev).expect("place");
        assert_eq!(stage_of(&s), Some(CheckpointStage::PostPlace));
        let n_sites = s.placed.as_ref().unwrap().lutff_sites.len();

        let dir = std::env::temp_dir().join("hck2-test-place");
        let _ = std::fs::remove_dir_all(&dir);
        let path = save_checkpoint(
            &s,
            &Constraints::default(),
            "place-a",
            &dir,
            &CheckpointSummary::default(),
        )
        .expect("save");
        let meta = read_meta(&path).expect("meta");
        assert_eq!(meta.stage, CheckpointStage::PostPlace);

        let rc = load_checkpoint(&path, &dev).expect("load");
        assert_eq!(stage_of(&rc.session), Some(CheckpointStage::PostPlace));
        let rp = rc.session.placed.as_ref().expect("placed restored");
        assert_eq!(rp.lutff_sites.len(), n_sites);
        assert_eq!(
            rp.lutff_sites,
            s.placed.as_ref().unwrap().lutff_sites,
            "every LUTFF site must match"
        );
        assert_eq!(
            rp.iob_sites,
            s.placed.as_ref().unwrap().iob_sites,
            "every IOB site must match"
        );
        // The packed section is derived from placed; routed must be absent.
        assert!(rc.session.packed.is_some());
        assert!(rc.session.routed.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn round_trip_post_route_reports_identical() {
        let (mut s, dev) = counter_session();
        s.place_design(&dev).expect("place");
        s.route_design_with(&dev, Default::default()).expect("route");
        s.write_bitstream(&dev).expect("bitgen");
        let xdc = Constraints::default();
        let dir = std::env::temp_dir().join("hck2-test-route");
        let _ = std::fs::remove_dir_all(&dir);

        // Timing numbers before the checkpoint.
        let d0 = s.design.clone().unwrap();
        let r0 = s.routed.clone().unwrap();
        let clks = test_clocks();
        let t0 = report_timing_routed_xdc(&d0, &r0, &clks, &xdc).expect("timing0");
        let summary = CheckpointSummary {
            wns_ps: t0.wns_ps,
            tns_ps: t0.tns_ps,
        };
        let path = save_checkpoint(&s, &xdc, "route-gold", &dir, &summary).expect("save");
        let metas = list_checkpoints(&dir);
        assert_eq!(metas.len(), 1);
        assert_eq!(metas[0].stage, CheckpointStage::PostRoute);
        assert_eq!(metas[0].wns_ps, 9640, "gold WNS must be captured");

        // Restore into a fresh session and re-run the report.
        let rc = load_checkpoint(&path, &dev).expect("load");
        let d1 = rc.session.design.clone().unwrap();
        let r1 = rc.session.routed.clone().unwrap();
        let clks1 = test_clocks();
        let t1 = report_timing_routed_xdc(&d1, &r1, &clks1, &rc.constraints).expect("timing1");
        assert_eq!(t0.wns_ps, t1.wns_ps, "WNS must be identical after restore");
        assert_eq!(t0.tns_ps, t1.tns_ps);
        assert_eq!(t0.endpoints, t1.endpoints);
        assert_eq!(t0.setup_ps, t1.setup_ps);
        // Placement identical: every LUTFF site matches.
        let p0 = &r0.placed;
        let p1 = &r1.placed;
        assert_eq!(p0.lutff_sites, p1.lutff_sites);
        assert_eq!(p0.iob_sites, p1.iob_sites);
        assert_eq!(r0.iob_src.len(), r1.iob_src.len());
        assert_eq!(
            r0.iob_src.iter().map(|x| &x.path).collect::<Vec<_>>(),
            r1.iob_src.iter().map(|x| &x.path).collect::<Vec<_>>()
        );
        // Bitstream identical.
        let b0 = s.bitstream.as_ref().unwrap();
        let b1 = rc.session.bitstream.as_ref().unwrap();
        assert_eq!(b0.frames, b1.frames);
        assert_eq!(b0.idcode, b1.idcode);

        // Diff of identical metas is zero.
        let d = diff_metas(&metas[0], &rc.meta);
        assert_eq!(d.d_wns_ps, 0);
        assert_eq!(d.d_luts, 0);

        delete_checkpoint(&path).expect("delete");
        assert!(list_checkpoints(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_rejects_wrong_part_and_garbage() {
        let dev = test_dev();
        let dir = std::env::temp_dir().join("hck2-test-bad");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let bad = dir.join("bad.hck2");
        std::fs::write(&bad, b"nope").unwrap();
        assert!(read_meta(&bad).is_err());
        assert!(load_checkpoint(&bad, &dev).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

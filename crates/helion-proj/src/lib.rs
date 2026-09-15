//! Dual-mode Session, disk checkpoints `.hckp`, object query, opt, ECO.

use helion_bits::{bitgen, bitgen_pblock, eco_lut, Bitstream};
use helion_device::Device;
use helion_ir::{CellKind, Design, PortDir};
use helion_pack::{apply_iob_electrical, pack, Packed};
use helion_place::{place_in_region, place_incremental, place_with, PlaceOpts, Placed};
use helion_route::{route_with, RouteOpts, Routed, HOP_DELAY_PS};
use helion_sta::{create_clock, load_xdc, report_timing_routed, Constraints};
use helion_hw::{program_hbits_with_cable, prog_sim, resolve_cable, CableBackend};
use helion_debug::insert_ila;

/// UG986 Lab 1 Helion equivalents of implementation strategies.
/// Not Vivado strategy trademarks: same *kind* of lever (timing vs runtime vs phys).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImplStrategy {
    /// Timing-driven place + full PathFinder. Gold WNS path (`impl_1`).
    Default,
    /// Same engine as Default (timing-driven family).
    TimingExplore,
    /// Wirelength place + 1 PathFinder iter (faster, worse WNS).
    RuntimeOpt,
    /// Timing-driven place + directed extra hops (phys-opt detours).
    PhysOpt,
}

impl ImplStrategy {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "default" | "impl_1" => Ok(Self::Default),
            "timingexplore" | "timing_explore" | "explore" => Ok(Self::TimingExplore),
            "runtimeopt" | "runtime_opt" | "runtime" => Ok(Self::RuntimeOpt),
            "physopt" | "phys_opt" | "phys" => Ok(Self::PhysOpt),
            other => Err(format!("unknown impl strategy {other}")),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::TimingExplore => "TimingExplore",
            Self::RuntimeOpt => "RuntimeOpt",
            Self::PhysOpt => "PhysOpt",
        }
    }

    pub fn place_opts(self) -> PlaceOpts {
        match self {
            Self::RuntimeOpt => PlaceOpts { timing_weight: 0.0 },
            _ => PlaceOpts { timing_weight: 0.75 },
        }
    }

    pub fn route_opts(self) -> RouteOpts {
        match self {
            Self::RuntimeOpt => RouteOpts {
                max_iters: 1,
                extra_hops: 0,
            },
            Self::PhysOpt => RouteOpts {
                max_iters: 8,
                extra_hops: 8,
            },
            _ => RouteOpts::default(),
        }
    }
}

/// UG986 Lab 2 Incremental Reuse Report (cells/nets/ports from HNF names).
#[derive(Clone, Debug, Default)]
pub struct ReuseReport {
    pub cells: usize,
    pub reused_cells: usize,
    pub nets: usize,
    pub reused_nets: usize,
    pub ports: usize,
    pub reused_ports: usize,
}

impl ReuseReport {
    pub fn cell_pct(&self) -> u32 {
        if self.cells == 0 {
            0
        } else {
            (self.reused_cells * 100 / self.cells) as u32
        }
    }

    pub fn text(&self) -> String {
        format!(
            "reuse cells={}/{} ({pct}%) nets={}/{} ports={}/{}",
            self.reused_cells,
            self.cells,
            self.reused_nets,
            self.nets,
            self.reused_ports,
            self.ports,
            pct = self.cell_pct()
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    Project,
    NonProject,
}

#[derive(Clone, Debug)]
pub struct Session {
    pub mode: Mode,
    pub design: Option<Design>,
    pub packed: Option<Packed>,
    pub placed: Option<Placed>,
    pub bitstream: Option<Bitstream>,
    pub routed: Option<Routed>,
    /// Last complete placement, used by incremental_impl (UG986 Lab 2).
    pub impl_checkpoint: Option<Placed>,
    pub hw_open: bool,
    pub programmed: bool,
    pub part: String,
}

impl Session {
    pub fn new(mode: Mode) -> Self {
        Self {
            mode,
            design: None,
            packed: None,
            placed: None,
            bitstream: None,
            routed: None,
            impl_checkpoint: None,
            hw_open: false,
            programmed: false,
            part: "HL10T-C32-1".into(),
        }
    }

    pub fn synth_design(&mut self, d: Design) {
        self.design = Some(d);
        self.reset_impl();
    }

    /// Drop place/route/bitstream (Vivado `reset_run impl_1`). Keeps the synth netlist.
    pub fn reset_impl(&mut self) {
        self.packed = None;
        self.placed = None;
        self.routed = None;
        self.bitstream = None;
        self.impl_checkpoint = None;
        self.programmed = false;
    }

    pub fn write_checkpoint(&mut self) -> Result<String, String> {
        let p = self.placed.as_ref().ok_or("write_checkpoint: not placed")?;
        self.impl_checkpoint = Some(p.clone());
        Ok(format!(
            "write_checkpoint lutff={}",
            p.lutff_sites.len()
        ))
    }

    /// Persist a reopenable `.hckp` (HNF + bitstream hash) so a new process can
    /// `open_checkpoint` yesterday's run. Refuses an empty/fake bitstream.
    pub fn write_checkpoint_to(
        &mut self,
        path: impl AsRef<std::path::Path>,
    ) -> Result<String, String> {
        let _ = self.placed.as_ref().ok_or("write_checkpoint: not placed")?;
        let frames = {
            let bits = self.bitstream.as_ref().ok_or(
                "write_checkpoint: empty bitstream refused (write_bitstream first)",
            )?;
            if bits.frames.is_empty() {
                return Err(
                    "write_checkpoint: empty bitstream refused (no configured frames)".into(),
                );
            }
            bits.frames.len()
        };
        let path = path.as_ref();
        let bytes = self.checkpoint();
        let hash = self.blinky_hash().unwrap_or(0);
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("write_checkpoint mkdir: {e}"))?;
            }
        }
        std::fs::write(path, &bytes)
            .map_err(|e| format!("write_checkpoint {}: {e}", path.display()))?;
        let placed_msg = self.write_checkpoint()?;
        Ok(format!(
            "{placed_msg} path={} bytes={} frames={} hash={:#x}",
            path.display(),
            bytes.len(),
            frames,
            hash
        ))
    }

    /// Reopen a `.hckp` written by [`write_checkpoint_to`]. Re-impls from the
    /// embedded HNF so ECO / `write_bitstream` / incremental place work.
    pub fn open_checkpoint(
        path: impl AsRef<std::path::Path>,
        dev: &Device,
    ) -> Result<Self, String> {
        let path = path.as_ref();
        let bytes = std::fs::read(path)
            .map_err(|e| format!("open_checkpoint {}: {e}", path.display()))?;
        let mut s = Self::restore_session(&bytes, dev)?;
        s.impl_checkpoint = s.placed.clone();
        Ok(s)
    }

    /// Place/route/bitgen using `.prj` pblock + impl-run strategy (Default = gold).
    pub fn impl_project(&mut self, dev: &Device, prj: &ProjectFile) -> Result<(), String> {
        if self.design.is_none() {
            return Err("impl_project: no design".into());
        }
        if let Some(pb) = prj.pblocks.iter().find(|p| p.ranged) {
            self.place_pblock(dev, pb.x0, pb.y0, pb.x1, pb.y1)?;
            self.route_design(dev)?;
            self.write_bitstream(dev)?;
            return Ok(());
        }
        let strategy = prj
            .impl_runs
            .iter()
            .find(|r| r.name.to_ascii_lowercase().starts_with("impl"))
            .or_else(|| prj.impl_runs.first())
            .map(|r| ImplStrategy::parse(&r.strategy))
            .transpose()?
            .unwrap_or(ImplStrategy::Default);
        self.impl_with_strategy(dev, strategy)
    }

    /// Drop the synth netlist and every impl artifact (Vivado `reset_run synth_1`).
    pub fn reset_synth(&mut self) {
        self.design = None;
        self.reset_impl();
    }

    pub fn opt_design_step(&mut self) -> Result<usize, String> {
        let d = self.design.as_mut().ok_or("opt_design: no design")?;
        Ok(opt_design(d))
    }

    pub fn place_design(&mut self, dev: &Device) -> Result<(), String> {
        self.place_design_with(dev, ImplStrategy::Default.place_opts())
    }

    pub fn place_design_with(&mut self, dev: &Device, opts: PlaceOpts) -> Result<(), String> {
        let d = self.design.as_ref().ok_or("place_design: no design")?;
        let packed = pack(d, dev)?;
        // Default timing_weight 0.75 matches `helion run` / QoR gold (9640 ps).
        let placed = place_with(&packed, dev, opts)?;
        self.packed = Some(packed);
        self.placed = Some(placed);
        self.routed = None;
        self.bitstream = None;
        Ok(())
    }

    /// UG893 `create_pblock`/`resize_pblock`: place into a HAD rectangle.
    pub fn place_pblock(
        &mut self,
        dev: &Device,
        x0: u32,
        y0: u32,
        x1: u32,
        y1: u32,
    ) -> Result<(), String> {
        let d = self.design.as_ref().ok_or("place_pblock: no design")?;
        let packed = pack(d, dev)?;
        let placed = place_in_region(
            &packed,
            dev,
            ImplStrategy::Default.place_opts(),
            x0,
            y0,
            x1,
            y1,
        )?;
        self.packed = Some(packed);
        self.placed = Some(placed);
        self.routed = None;
        self.bitstream = None;
        Ok(())
    }

    /// Partial bitstream for Pblock sites (`helion-bits::bitgen_pblock`).
    pub fn write_pblock_bitstream(
        &self,
        dev: &Device,
        sites: &[(u32, u32)],
    ) -> Result<Bitstream, String> {
        let routed = self.routed.as_ref().ok_or("write_pblock: not routed")?;
        bitgen_pblock(dev, routed, sites)
    }

    pub fn route_design(&mut self, dev: &Device) -> Result<(), String> {
        self.route_design_with(dev, RouteOpts::default())
    }

    pub fn route_design_with(&mut self, dev: &Device, opts: RouteOpts) -> Result<(), String> {
        let placed = self.placed.as_ref().ok_or("route_design: not placed")?;
        let routed = route_with(placed, dev, opts)?;
        self.routed = Some(routed);
        self.bitstream = None;
        Ok(())
    }

    /// Full impl with a Lab 1 strategy. Assumes `design` is already synthesized.
    pub fn impl_with_strategy(&mut self, dev: &Device, strategy: ImplStrategy) -> Result<(), String> {
        if strategy == ImplStrategy::PhysOpt {
            let _ = self.opt_design_step()?;
        }
        self.place_design_with(dev, strategy.place_opts())?;
        self.route_design_with(dev, strategy.route_opts())?;
        self.write_bitstream(dev)?;
        Ok(())
    }

    /// UG986 Lab 2: place the current netlist reusing `prev` sites for named cells.
    pub fn incremental_place(
        &mut self,
        dev: &Device,
        prev: &Placed,
    ) -> Result<ReuseReport, String> {
        let d = self.design.as_ref().ok_or("incremental_place: no design")?;
        let packed = pack(d, dev)?;
        let (placed, reused_lutff) = place_incremental(
            &packed,
            dev,
            prev,
            PlaceOpts { timing_weight: 0.75 },
        )?;
        let prev_cells: std::collections::HashSet<&str> = prev
            .packed
            .lutffs
            .iter()
            .flat_map(|l| [l.lut_cell.as_str(), l.ff_cell.as_str()])
            .chain(prev.packed.iobs.iter().map(|i| i.cell.as_str()))
            .collect();
        let reused_cells = d
            .cells
            .iter()
            .filter(|c| prev_cells.contains(c.name.as_str()))
            .count();
        let prev_nets: std::collections::HashSet<&str> =
            prev.packed.lutffs.iter().map(|l| l.q_net.as_str()).collect();
        let reused_nets = d
            .nets
            .iter()
            .filter(|n| prev_nets.contains(n.name.as_str()))
            .count();
        let report = ReuseReport {
            cells: d.cells.len(),
            reused_cells,
            nets: d.nets.len(),
            reused_nets,
            ports: d.ports.len(),
            reused_ports: d.ports.len(),
        };
        let _ = reused_lutff;
        self.packed = Some(packed);
        self.placed = Some(placed);
        self.routed = None;
        self.bitstream = None;
        Ok(report)
    }

    /// UG986 Lab 3: drop an IOB net's route (delay 0).
    pub fn unroute_net(&mut self, net: &str) -> Result<String, String> {
        let r = self.routed.as_mut().ok_or("unroute_net: not routed")?;
        let idx = r
            .placed
            .packed
            .iobs
            .iter()
            .position(|i| i.from_net == net || i.cell == net)
            .ok_or_else(|| format!("unroute_net: no IOB net {net}"))?;
        if let Some(io) = r.iob_src.get_mut(idx) {
            io.hops = 0;
            io.delay_ps = 0;
        }
        self.bitstream = None;
        Ok(format!("unroute_net {net}"))
    }

    /// UG986 Lab 3: add FIXED_ROUTE extra hops (delay) on an IOB net.
    pub fn fix_route(&mut self, net: &str, extra_hops: u32) -> Result<String, String> {
        let r = self.routed.as_mut().ok_or("fix_route: not routed")?;
        let idx = r
            .placed
            .packed
            .iobs
            .iter()
            .position(|i| i.from_net == net || i.cell == net)
            .ok_or_else(|| format!("fix_route: no IOB net {net}"))?;
        if let Some(io) = r.iob_src.get_mut(idx) {
            io.hops += extra_hops;
            io.delay_ps += extra_hops as i64 * HOP_DELAY_PS;
        }
        self.bitstream = None;
        Ok(format!(
            "fix_route {net} extra_hops={extra_hops} delay_ps={}",
            r.iob_src.get(idx).map(|i| i.delay_ps).unwrap_or(0)
        ))
    }

    /// UG986 Lab 4 Check ECO: cells in the netlist that are not in the last placement.
    pub fn check_eco(&self) -> Result<String, String> {
        let d = self.design.as_ref().ok_or("check_eco: no design")?;
        let placed = self.placed.as_ref().ok_or("check_eco: not placed")?;
        let have: std::collections::HashSet<&str> = placed
            .packed
            .lutffs
            .iter()
            .map(|l| l.lut_cell.as_str())
            .chain(placed.packed.lutffs.iter().map(|l| l.ff_cell.as_str()))
            .chain(placed.packed.iobs.iter().map(|i| i.cell.as_str()))
            .collect();
        let missing: Vec<&str> = d
            .cells
            .iter()
            .map(|c| c.name.as_str())
            .filter(|n| !have.contains(n))
            .collect();
        Ok(format!(
            "check_eco missing={} {}",
            missing.len(),
            missing.join(",")
        ))
    }

    /// UG986 Lab 4: insert a LUT+FF pair named like ECO_LUT3 so pack can place it.
    pub fn insert_eco_lut(&mut self, name: &str, init: u64) -> Result<String, String> {
        let d = self.design.as_mut().ok_or("insert_eco_lut: no design")?;
        if d.cells.iter().any(|c| c.name == name) {
            return Err(format!("insert_eco_lut: {name} exists"));
        }
        let ff = format!("{name}_ff");
        d.add_cell(name, CellKind::Lut6 { init });
        d.add_cell(&ff, CellKind::Hff);
        if !d.ports.iter().any(|p| p.name == "clk") {
            d.add_port("clk", PortDir::In);
        }
        d.connect("clk", &ff, "CLK");
        d.connect(format!("{name}_d"), name, "O");
        d.connect(format!("{name}_d"), &ff, "D");
        d.connect(format!("{name}_q"), &ff, "Q");
        Ok(format!("insert_eco_lut {name} init={init:#x}"))
    }

    pub fn write_bitstream(&mut self, dev: &Device) -> Result<&Bitstream, String> {
        if self.design.is_none() {
            return Err("write_bitstream: no design".into());
        }
        self.sync_iob_electrical();
        let routed = self.routed.as_ref().ok_or("write_bitstream: not routed")?;
        let bits = bitgen(dev, routed)?;
        if bits.frames.is_empty() {
            return Err(
                "write_bitstream: empty/fake bitstream refused (no configured frames)".into(),
            );
        }
        self.bitstream = Some(bits);
        Ok(self.bitstream.as_ref().unwrap())
    }

    /// Push HNF port DRIVE / SLEW / PULLTYPE / DIFF_TERM / IN_TERM onto packed
    /// IOBs so bitgen sees post-pack `set_property` without a re-pack.
    fn sync_iob_electrical(&mut self) {
        let Some(d) = self.design.clone() else {
            return;
        };
        if let Some(p) = self.packed.as_mut() {
            apply_iob_electrical(&d, &mut p.iobs);
        }
        if let Some(p) = self.placed.as_mut() {
            apply_iob_electrical(&d, &mut p.packed.iobs);
        }
        if let Some(r) = self.routed.as_mut() {
            apply_iob_electrical(&d, &mut r.placed.packed.iobs);
        }
    }

    pub fn write_hnf(&self) -> Result<String, String> {
        Ok(self.design.as_ref().ok_or("write_hnf: no design")?.to_hnf())
    }

    pub fn report_timing(&self, dev: &Device) -> Result<String, String> {
        let _ = dev;
        let d = self.design.as_ref().ok_or("report_timing: no design")?;
        let r = self.routed.as_ref().ok_or("report_timing: not routed")?;
        let mut clks = Vec::new();
        create_clock(&mut clks, "clk", 10_000, "clk");
        let t = report_timing_routed(d, r, &clks)?;
        Ok(format!(
            "report_timing {} WNS_PS={} TNS_PS={} SETUP_PS={} HOLD_PS={} HOLD_SLACK_PS={} endpoints={} r2r_ps={} iob_ps={} route_ps={}",
            d.name, t.wns_ps, t.tns_ps, t.setup_ps, t.hold_ps, t.hold_slack_ps, t.endpoints, t.r2r_ps, t.iob_ps, t.route_ps
        ))
    }

    pub fn report_utilization(&self, dev: &Device) -> Result<String, String> {
        let p = self
            .placed
            .as_ref()
            .map(|pl| &pl.packed)
            .or(self.packed.as_ref())
            .ok_or("report_utilization: not packed")?;
        Ok(format!(
            "report_utilization LUTFF={}/{} IOB={}/{} BRAM={}/{} DSP={}/{}",
            p.lutffs.len(),
            dev.lut6_count(),
            p.iobs.len(),
            dev.iob_sites().count(),
            p.brams.len(),
            dev.n_bram,
            p.macs.len(),
            dev.n_dsp
        ))
    }

    pub fn open_hw_manager(&mut self) {
        self.hw_open = true;
    }

    /// Program last bitstream. `cable` is `auto|sim|usb|ofl` (default `auto`).
    pub fn program_hw(&mut self, dev: &Device) -> Result<String, String> {
        // Product default = auto → OFL board path. USB=0 → honest Err (never
        // soft-hold / never invent sim DONE=1). Explicit cable=sim for fabric.
        self.program_hw_cable(dev, "auto")
    }

    pub fn program_hw_cable(&mut self, dev: &Device, cable: &str) -> Result<String, String> {
        if !self.hw_open {
            return Err(
                "program_hw: no cable — open_hw_manager first (sim or openFPGALoader USB)".into(),
            );
        }
        let bits = self.bitstream.as_ref().ok_or_else(|| {
            String::from(
                "program_hw: no bitstream — run write_bitstream / Implement, or `helion bitstream -o out.hbits`",
            )
        })?;
        let info = resolve_cable(cable)?;
        match info.backend {
            CableBackend::Sim => {
                let frames = bits.frames.len();
                let bytes = bits.packets.len();
                let st = prog_sim(dev, bits)?;
                self.programmed = true;
                Ok(format!(
                    "program_hw cable={} backend=sim part={} frames={} bytes={} DONE={} GWE={} CRC_ERR={} (sim fabric; not board DONE)",
                    info.id,
                    dev.part,
                    frames,
                    bytes,
                    st.done as u8,
                    st.gwe as u8,
                    st.crc_err as u8
                ))
            }
            CableBackend::MpsseSim => {
                let frames = bits.frames.len();
                let bytes = bits.packets.len();
                let st = helion_hw::prog_mpsse_sim(dev, bits)?;
                self.programmed = true;
                Ok(format!(
                    "program_hw cable={} backend=mpsse-sim part={} frames={} bytes={} DONE={} GWE={} CRC_ERR={} (sim fabric bitbang; not board DONE)",
                    info.id,
                    dev.part,
                    frames,
                    bytes,
                    st.done as u8,
                    st.gwe as u8,
                    st.crc_err as u8
                ))
            }
            CableBackend::OpenFpgaLoader | CableBackend::NativeUsb => {
                // Board path: write packets and invoke helion-hw. DONE only if
                // OFL/native confirms — never soft-hold Ok, never invent DONE.
                let dir = std::env::temp_dir().join("helion-program-hw");
                std::fs::create_dir_all(&dir).map_err(|e| format!("program_hw: temp dir: {e}"))?;
                let path = dir.join(format!("session-{}.hbits", std::process::id()));
                std::fs::write(&path, &bits.packets)
                    .map_err(|e| format!("program_hw: write {}: {e}", path.display()))?;
                match program_hbits_with_cable(dev, &path, &info, false) {
                    Ok(outcome) => {
                        self.programmed = true;
                        let line = outcome.summary_line("program", &dev.part);
                        Ok(format!(
                            "program_hw cable={} {}",
                            info.id,
                            line
                        ))
                    }
                    Err(e) => {
                        self.programmed = false;
                        Err(format!(
                            "program_hw cable={} backend={} refused DONE (USB/programmer): {e}",
                            info.id,
                            info.backend.as_str()
                        ))
                    }
                }
            }
        }
    }

    pub fn mark_debug(&mut self, net: &str) -> Result<(), String> {
        let d = self.design.as_mut().ok_or("mark_debug: no design")?;
        d.mark_debug(net)?;
        insert_ila(d, net)?;
        Ok(())
    }

    pub fn set_property(&mut self, key: &str, val: &str, obj: &str) -> Result<(), String> {
        let d = self.design.as_mut().ok_or("set_property: no design")?;
        if key.eq_ignore_ascii_case("DONT_TOUCH") || key.eq_ignore_ascii_case("keep") {
            d.set_cell_attr(obj, "DONT_TOUCH", val)?;
        } else if key.eq_ignore_ascii_case("mark_debug") {
            d.set_net_attr(obj, "mark_debug", val)?;
        } else if key.eq_ignore_ascii_case("LOC") || key.eq_ignore_ascii_case("PACKAGE_PIN") {
            d.set_loc(obj, val)?;
        } else if key.eq_ignore_ascii_case("IOSTANDARD") {
            d.set_iostandard(obj, val)?;
        } else if key.eq_ignore_ascii_case("DRIVE") {
            d.set_drive(obj, val)?;
        } else if key.eq_ignore_ascii_case("SLEW") {
            d.set_slew(obj, val)?;
        } else if key.eq_ignore_ascii_case("PULLTYPE") {
            d.set_pulltype(obj, val)?;
        } else if key.eq_ignore_ascii_case("DIFF_TERM") {
            d.set_diff_term(obj, val)?;
        } else if key.eq_ignore_ascii_case("IN_TERM") {
            d.set_in_term(obj, val)?;
        } else {
            d.set_cell_attr(obj, key, val)?;
        }
        Ok(())
    }

    pub fn impl_design(&mut self, d: Design, dev: &Device) -> Result<(), String> {
        self.synth_design(d);
        self.place_design(dev)?;
        self.route_design(dev)?;
        self.write_bitstream(dev)?;
        Ok(())
    }

    pub fn restore_session(bytes: &[u8], dev: &Device) -> Result<Self, String> {
        let (mode, hash, design) = Self::restore_with_ir(bytes)?;
        let mut s = Self::new(mode);
        s.part = dev.part.clone();
        if let Some(d) = design {
            s.impl_design(d, dev)?;
        }
        let h2 = s.blinky_hash().unwrap_or(0);
        if h2 != hash {
            return Err(format!(
                "hckp restore hash mismatch stored {hash:#x} got {h2:#x}"
            ));
        }
        Ok(s)
    }

    pub fn eco(&mut self, dev: &Device, cell: &str, new_init: u64) -> Result<(), String> {
        let r = self.routed.as_ref().ok_or("eco: not implemented")?;
        let bits = eco_lut(dev, r, cell, new_init)?;
        if let Some(rt) = self.routed.as_mut() {
            if let Some(lf) = rt
                .placed
                .packed
                .lutffs
                .iter_mut()
                .find(|l| l.lut_cell == cell || l.ff_cell == cell)
            {
                lf.init = new_init;
            }
        }
        self.bitstream = Some(bits);
        Ok(())
    }

    pub fn blinky_hash(&self) -> Option<u32> {
        self.bitstream.as_ref().map(|b| {
            let mut h = b.idcode;
            for ((bl, maj, min), w) in &b.frames {
                h ^= helion_bits::crc32c(&[*bl, *min]);
                h ^= *maj as u32;
                h = h.wrapping_mul(0x9E37_79B9) ^ helion_bits::crc32c(&w.to_le_bytes());
            }
            h
        })
    }

    pub fn checkpoint(&self) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"HCKP");
        v.push(match self.mode {
            Mode::Project => 1,
            Mode::NonProject => 2,
        });
        let h = self.blinky_hash().unwrap_or(0);
        v.extend_from_slice(&h.to_le_bytes());
        if let Some(d) = &self.design {
            let hnf = d.to_hnf();
            let b = hnf.as_bytes();
            v.extend_from_slice(&(b.len() as u32).to_le_bytes());
            v.extend_from_slice(b);
        }
        v
    }

    pub fn restore(bytes: &[u8]) -> Result<(Mode, u32), String> {
        let (m, h, _) = Self::restore_with_ir(bytes)?;
        Ok((m, h))
    }

    pub fn restore_with_ir(bytes: &[u8]) -> Result<(Mode, u32, Option<Design>), String> {
        if bytes.len() < 9 || &bytes[0..4] != b"HCKP" {
            return Err("bad hckp".into());
        }
        let mode = match bytes[4] {
            1 => Mode::Project,
            2 => Mode::NonProject,
            _ => return Err("bad mode".into()),
        };
        let hash = u32::from_le_bytes(bytes[5..9].try_into().unwrap());
        let design = if bytes.len() >= 13 {
            let n = u32::from_le_bytes(bytes[9..13].try_into().unwrap()) as usize;
            if bytes.len() >= 13 + n {
                let text = std::str::from_utf8(&bytes[13..13 + n]).map_err(|e| e.to_string())?;
                Some(Design::from_hnf(text)?)
            } else {
                None
            }
        } else {
            None
        };
        Ok((mode, hash, design))
    }
}

pub fn get_cells(d: &Design, filter: Option<&str>) -> Vec<String> {
    d.cells
        .iter()
        .filter(|c| filter.map(|f| c.name.contains(f)).unwrap_or(true))
        .map(|c| c.name.clone())
        .collect()
}

pub fn get_nets(d: &Design, filter: Option<&str>) -> Vec<String> {
    d.nets
        .iter()
        .filter(|n| filter.map(|f| n.name.contains(f)).unwrap_or(true))
        .map(|n| n.name.clone())
        .collect()
}

pub fn get_pins(d: &Design, cell: &str) -> Vec<String> {
    let idx = d.pin_index();
    ["I0", "I1", "I2", "I3", "I4", "I5", "O", "D", "Q", "CLK", "I", "PAD"]
        .into_iter()
        .filter_map(|pin| {
            idx.net_on(cell, pin)
                .map(|_| format!("{cell}/{pin}"))
        })
        .collect()
}

/// UG893 Floorplanning rectangle persisted in `.prj` (`create_pblock` / `resize_pblock`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrjPblock {
    pub name: String,
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
    pub ranged: bool,
    pub cells: Vec<String>,
}

impl PrjPblock {
    pub fn range_text(&self) -> String {
        if !self.ranged {
            return "-".into();
        }
        format!("CLB_X{}Y{}:CLB_X{}Y{}", self.x0, self.y0, self.x1, self.y1)
    }
}

/// UG986 Design Run persisted in `.prj` (`create_run` / `launch_runs`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PrjImplRun {
    pub name: String,
    pub strategy: String,
}

/// Drop const-0 LUT+FF pairs that do not drive an IOB.
/// Vivado-like project file: `part`, `read_sv` (multi), `read_xdc`/`read_sdc`,
/// `create_clock`, `set_property PACKAGE_PIN` / `TOP`, pblock, impl run, `.hckp`.
#[derive(Clone, Debug, Default)]
pub struct ProjectFile {
    pub part: String,
    pub sources: Vec<String>,
    /// External XDC/SDC paths from `read_xdc` / `read_sdc`.
    pub constraint_files: Vec<String>,
    /// `.helion` IP package paths from `read_ip` (expanded by `expand_ip_packages`).
    pub ip_packages: Vec<String>,
    /// Optional elaborator top (`top <mod>` / `set_property TOP <mod>`).
    pub top: Option<String>,
    pub sdc: Vec<String>,
    pub package_pins: Vec<(String, String)>,
    pub iostandards: Vec<(String, String)>,
    pub drives: Vec<(String, String)>,
    pub slews: Vec<(String, String)>,
    pub pulltypes: Vec<(String, String)>,
    pub diff_terms: Vec<(String, String)>,
    pub in_terms: Vec<(String, String)>,
    /// Floorplan pblocks (`create_pblock` / `resize_pblock` / `add_cells_to_pblock`).
    pub pblocks: Vec<PrjPblock>,
    /// Implementation runs (`create_run impl_1 -strategy Default`).
    pub impl_runs: Vec<PrjImplRun>,
    /// Disk checkpoint path (`write_checkpoint path.hckp`).
    pub checkpoint_path: Option<String>,
}

fn parse_clb_xy(spec: &str) -> Option<(u32, u32)> {
    let s = spec.trim().trim_matches(|c: char| "{}[]".contains(c));
    let rest = s
        .strip_prefix("CLB_X")
        .or_else(|| s.strip_prefix("SLICE_X"))?;
    let (xs, ys) = rest.split_once('Y')?;
    Some((xs.parse().ok()?, ys.parse().ok()?))
}

fn parse_clb_range(spec: &str) -> Option<(u32, u32, u32, u32)> {
    let s = spec.trim().trim_matches(|c: char| "{}[]".contains(c));
    let (a, b) = s.split_once(':')?;
    let (x0, y0) = parse_clb_xy(a)?;
    let (x1, y1) = parse_clb_xy(b)?;
    Some((x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)))
}

fn prj_token_is_helper(s: &str) -> bool {
    matches!(
        s,
        "get_cells" | "get_pblocks" | "get_runs" | "-cells" | "-add" | "-force"
    )
}

fn upsert_impl_run(p: &mut ProjectFile, name: &str, strategy: Option<&str>) {
    if let Some(r) = p.impl_runs.iter_mut().find(|r| r.name == name) {
        if let Some(s) = strategy {
            r.strategy = s.to_string();
        }
    } else {
        p.impl_runs.push(PrjImplRun {
            name: name.to_string(),
            strategy: strategy.unwrap_or("Default").to_string(),
        });
    }
}

fn pblock_named_mut<'a>(p: &'a mut ProjectFile, name: &str) -> &'a mut PrjPblock {
    if let Some(i) = p.pblocks.iter().position(|b| b.name == name) {
        return &mut p.pblocks[i];
    }
    p.pblocks.push(PrjPblock {
        name: name.to_string(),
        ..Default::default()
    });
    p.pblocks.last_mut().unwrap()
}

pub fn load_prj(text: &str) -> Result<ProjectFile, String> {
    let mut p = ProjectFile {
        part: "HL10T-C32-1".into(),
        ..Default::default()
    };
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut toks = line.split_whitespace();
        let Some(cmd) = toks.next() else { continue };
        match cmd {
            "part" => {
                if let Some(v) = toks.next() {
                    p.part = v.to_string();
                }
            }
            "read_sv" | "read_vhdl" | "read_c" | "read_verilog" | "sv" | "vhdl" | "c" => {
                if let Some(v) = toks.next() {
                    p.sources.push(v.to_string());
                }
            }
            "read_xdc" | "read_sdc" | "xdc" | "sdc" => {
                if let Some(v) = toks.next() {
                    p.constraint_files.push(v.to_string());
                }
            }
            "read_ip" | "ip" => {
                if let Some(v) = toks.next() {
                    p.ip_packages.push(v.to_string());
                }
            }
            "top" => {
                if let Some(v) = toks.next() {
                    p.top = Some(v.to_string());
                }
            }
            "create_clock" => p.sdc.push(line.to_string()),
            "set_property" => {
                let rest: Vec<&str> = toks.collect();
                if rest.first().copied() == Some("TOP") && rest.len() >= 2 {
                    let name = rest[1]
                        .trim_matches(|c: char| c == '[' || c == ']')
                        .to_string();
                    if !name.is_empty()
                        && !name.eq_ignore_ascii_case("current_fileset")
                        && !name.starts_with("get_")
                    {
                        p.top = Some(name);
                    }
                } else if rest.first().copied() == Some("PACKAGE_PIN") && rest.len() >= 2 {
                    let site = rest[1].to_string();
                    let joined = rest[2..].join(" ");
                    let port = joined
                        .split_once("get_ports")
                        .and_then(|(_, r)| {
                            r.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                                .find(|s| !s.is_empty())
                        })
                        .unwrap_or("")
                        .to_string();
                    if !port.is_empty() {
                        p.package_pins.push((port, site));
                    }
                } else if rest.first().copied() == Some("IOSTANDARD") && rest.len() >= 2 {
                    let std = rest[1].to_string();
                    let joined = rest[2..].join(" ");
                    let port = joined
                        .split_once("get_ports")
                        .and_then(|(_, r)| {
                            r.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                                .find(|s| !s.is_empty())
                        })
                        .unwrap_or("")
                        .to_string();
                    if !port.is_empty() {
                        p.iostandards.push((port, std));
                    }
                } else if rest.first().copied() == Some("DRIVE") && rest.len() >= 2 {
                    let val = rest[1].to_string();
                    let joined = rest[2..].join(" ");
                    let port = joined
                        .split_once("get_ports")
                        .and_then(|(_, r)| {
                            r.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                                .find(|s| !s.is_empty())
                        })
                        .unwrap_or("")
                        .to_string();
                    if !port.is_empty() {
                        p.drives.push((port, val));
                    }
                } else if rest.first().copied() == Some("SLEW") && rest.len() >= 2 {
                    let val = rest[1].to_string();
                    let joined = rest[2..].join(" ");
                    let port = joined
                        .split_once("get_ports")
                        .and_then(|(_, r)| {
                            r.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                                .find(|s| !s.is_empty())
                        })
                        .unwrap_or("")
                        .to_string();
                    if !port.is_empty() {
                        p.slews.push((port, val));
                    }
                } else if rest.first().copied() == Some("PULLTYPE") && rest.len() >= 2 {
                    let val = rest[1].to_string();
                    let joined = rest[2..].join(" ");
                    let port = joined
                        .split_once("get_ports")
                        .and_then(|(_, r)| {
                            r.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                                .find(|s| !s.is_empty())
                        })
                        .unwrap_or("")
                        .to_string();
                    if !port.is_empty() {
                        p.pulltypes.push((port, val));
                    }
                } else if rest.first().copied() == Some("DIFF_TERM") && rest.len() >= 2 {
                    let val = rest[1].to_string();
                    let joined = rest[2..].join(" ");
                    let port = joined
                        .split_once("get_ports")
                        .and_then(|(_, r)| {
                            r.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                                .find(|s| !s.is_empty())
                        })
                        .unwrap_or("")
                        .to_string();
                    if !port.is_empty() {
                        p.diff_terms.push((port, val));
                    }
                } else if rest.first().copied() == Some("IN_TERM") && rest.len() >= 2 {
                    let val = rest[1].to_string();
                    let joined = rest[2..].join(" ");
                    let port = joined
                        .split_once("get_ports")
                        .and_then(|(_, r)| {
                            r.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                                .find(|s| !s.is_empty())
                        })
                        .unwrap_or("")
                        .to_string();
                    if !port.is_empty() {
                        p.in_terms.push((port, val));
                    }
                }
            }
            "create_pblock" => {
                let rest: Vec<&str> = toks.collect();
                let mut name = String::new();
                let mut add: Option<String> = None;
                let mut i = 0;
                while i < rest.len() {
                    if rest[i] == "-add" {
                        i += 1;
                        if i < rest.len() {
                            add = Some(rest[i].to_string());
                        }
                    } else if name.is_empty() && !rest[i].starts_with('-') {
                        name = rest[i]
                            .trim_matches(|c: char| "{}[]".contains(c))
                            .to_string();
                    }
                    i += 1;
                }
                if name.is_empty() {
                    name = format!("pblock_{}", p.pblocks.len());
                }
                let _ = pblock_named_mut(&mut p, &name);
                if let Some(spec) = add {
                    let Some((x0, y0, x1, y1)) = parse_clb_range(&spec) else {
                        return Err(format!(
                            "create_pblock -add: cannot parse CLB range {spec}"
                        ));
                    };
                    let pb = pblock_named_mut(&mut p, &name);
                    pb.x0 = x0;
                    pb.y0 = y0;
                    pb.x1 = x1;
                    pb.y1 = y1;
                    pb.ranged = true;
                }
            }
            "resize_pblock" => {
                let rest: Vec<&str> = toks.collect();
                let mut name = String::new();
                let mut spec = String::new();
                for tok in rest {
                    if tok == "-add" {
                        continue;
                    }
                    let t = tok.trim_matches(|c: char| "{}[]".contains(c));
                    if t.is_empty() || prj_token_is_helper(t) {
                        continue;
                    }
                    if name.is_empty() && !t.contains(':') {
                        name = t.to_string();
                    } else if !spec.is_empty() {
                        spec.push(':');
                        spec.push_str(t);
                    } else {
                        spec = t.to_string();
                    }
                }
                if !name.is_empty() && !spec.is_empty() {
                    let Some((x0, y0, x1, y1)) = parse_clb_range(&spec) else {
                        return Err(format!(
                            "resize_pblock: cannot parse CLB range {spec}"
                        ));
                    };
                    let pb = pblock_named_mut(&mut p, &name);
                    pb.x0 = x0;
                    pb.y0 = y0;
                    pb.x1 = x1;
                    pb.y1 = y1;
                    pb.ranged = true;
                }
            }
            "add_cells_to_pblock" => {
                let rest: Vec<String> = toks
                    .map(|t| t.trim_matches(|c: char| "{}[]".contains(c)).to_string())
                    .filter(|t| !t.is_empty() && !prj_token_is_helper(t))
                    .collect();
                if let Some(name) = rest.first() {
                    let pb = pblock_named_mut(&mut p, name);
                    for c in rest.iter().skip(1) {
                        if !pb.cells.contains(c) {
                            pb.cells.push(c.clone());
                        }
                    }
                }
            }
            "create_run" => {
                let rest: Vec<&str> = toks.collect();
                let mut name = String::new();
                let mut strategy = String::from("Default");
                let mut i = 0;
                while i < rest.len() {
                    if rest[i] == "-strategy" || rest[i] == "-strat" {
                        i += 1;
                        if i < rest.len() {
                            strategy = rest[i].to_string();
                        }
                    } else if name.is_empty() && !rest[i].starts_with('-') {
                        name = rest[i]
                            .trim_matches(|c: char| "{}[]".contains(c))
                            .to_string();
                    }
                    i += 1;
                }
                if !name.is_empty() {
                    upsert_impl_run(&mut p, &name, Some(&strategy));
                }
            }
            "launch_runs" => {
                if let Some(name) = toks.next() {
                    let name = name.trim_matches(|c: char| "{}[]".contains(c));
                    if !name.is_empty() {
                        upsert_impl_run(&mut p, name, None);
                    }
                }
            }
            "write_checkpoint" | "open_checkpoint" | "read_checkpoint" | "checkpoint" => {
                let rest: Vec<&str> = toks.collect();
                if let Some(v) = rest.iter().rev().find(|t| !t.starts_with('-') && !t.is_empty()) {
                    p.checkpoint_path = Some((*v).to_string());
                }
            }
            _ => {}
        }
    }
    if p.sources.is_empty() && p.ip_packages.is_empty() {
        return Err("project has no sources".into());
    }
    Ok(p)
}

/// Resolve a project-relative path against the `.prj` location, then CWD.
pub fn resolve_prj_path(prj_path: &std::path::Path, given: &str) -> std::path::PathBuf {
    let given = std::path::Path::new(given);
    if given.exists() {
        return given.to_path_buf();
    }
    for anc in prj_path.ancestors() {
        let cand = anc.join(given);
        if cand.exists() {
            return cand;
        }
        if let Some(name) = given.file_name() {
            let cand = anc.join(name);
            if cand.exists() {
                return cand;
            }
        }
    }
    given.to_path_buf()
}

/// Expand `read_ip` `.helion` packages into `sources` / `constraint_files`.
/// Paths are resolved against the `.prj`, then against each package manifest.
/// Does not override an explicit project `top`; if unset, takes the first package top.
pub fn expand_ip_packages(
    prj: &mut ProjectFile,
    prj_path: &std::path::Path,
) -> Result<Vec<helion_ipxact::HelionPackage>, String> {
    let mut loaded = Vec::new();
    let ips = prj.ip_packages.clone();
    for ip_ref in ips {
        let path = resolve_prj_path(prj_path, &ip_ref);
        let pkg = helion_ipxact::load_helion(&path).map_err(|e| {
            format!("read_ip {ip_ref}: {e}")
        })?;
        for f in pkg.resolve_files()? {
            let s = f.display().to_string();
            if !prj.sources.iter().any(|x| x == &s) {
                prj.sources.push(s);
            }
        }
        for c in pkg.resolve_constraints()? {
            let s = c.display().to_string();
            if !prj.constraint_files.iter().any(|x| x == &s) {
                prj.constraint_files.push(s);
            }
        }
        if prj.top.is_none() {
            if let Some(t) = &pkg.top {
                prj.top = Some(t.clone());
            }
        }
        loaded.push(pkg);
    }
    if prj.sources.is_empty() {
        return Err("project has no sources after read_ip expand".into());
    }
    Ok(loaded)
}

/// Serialize a [`ProjectFile`] to Vivado-shaped `.prj` text (`part` / `read_sv` / `read_xdc`).
pub fn format_prj(prj: &ProjectFile) -> String {
    let mut out = String::new();
    if !prj.part.is_empty() {
        out.push_str(&format!("part {}\n", prj.part));
    } else {
        out.push_str("part HL10T-C32-1\n");
    }
    if let Some(top) = &prj.top {
        out.push_str(&format!("top {top}\n"));
    }
    for s in &prj.sources {
        let ext = std::path::Path::new(s)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let cmd = match ext.as_str() {
            "vhd" | "vhdl" => "read_vhdl",
            "c" | "cc" | "cpp" => "read_c",
            _ => "read_sv",
        };
        out.push_str(&format!("{cmd} {s}\n"));
    }
    for ip in &prj.ip_packages {
        out.push_str(&format!("read_ip {ip}\n"));
    }
    for c in &prj.constraint_files {
        out.push_str(&format!("read_xdc {c}\n"));
    }
    for line in &prj.sdc {
        out.push_str(line);
        if !line.ends_with('\n') {
            out.push('\n');
        }
    }
    for pb in &prj.pblocks {
        out.push_str(&format!("create_pblock {}\n", pb.name));
        if pb.ranged {
            out.push_str(&format!(
                "resize_pblock {} -add {{{}}}\n",
                pb.name,
                pb.range_text()
            ));
        }
        for c in &pb.cells {
            out.push_str(&format!("add_cells_to_pblock {} {c}\n", pb.name));
        }
    }
    for r in &prj.impl_runs {
        let strat = if r.strategy.is_empty() {
            "Default"
        } else {
            r.strategy.as_str()
        };
        out.push_str(&format!("create_run {} -strategy {strat}\n", r.name));
    }
    if let Some(ck) = &prj.checkpoint_path {
        out.push_str(&format!("write_checkpoint {ck}\n"));
    }
    out
}

/// Write `prj` to `path` (parent directories must exist or be creatable by the caller).
pub fn write_prj(path: &std::path::Path, prj: &ProjectFile) -> Result<(), String> {
    let body = format_prj(prj);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| format!("write_prj mkdir: {e}"))?;
        }
    }
    std::fs::write(path, body).map_err(|e| format!("write_prj {}: {e}", path.display()))
}

/// Flatten inline SDC + `read_xdc` files + `set_property` IO into one XDC blob, then `load_xdc`.
/// Empty constraints keep the CLI gold WNS path (default 10 ns clk applied by the runner).
pub fn constraints_from_project(
    prj: &ProjectFile,
    prj_path: &std::path::Path,
) -> Result<Constraints, String> {
    let mut blob = String::new();
    for line in &prj.sdc {
        blob.push_str(line);
        blob.push('\n');
    }
    for cf in &prj.constraint_files {
        let path = resolve_prj_path(prj_path, cf);
        let body = std::fs::read_to_string(&path)
            .map_err(|e| format!("read_xdc {}: {e}", path.display()))?;
        blob.push_str(&body);
        if !body.ends_with('\n') {
            blob.push('\n');
        }
    }
    for (port, site) in &prj.package_pins {
        blob.push_str(&format!(
            "set_property PACKAGE_PIN {site} [get_ports {port}]\n"
        ));
    }
    for (port, std) in &prj.iostandards {
        blob.push_str(&format!(
            "set_property IOSTANDARD {std} [get_ports {port}]\n"
        ));
    }
    for (port, val) in &prj.drives {
        blob.push_str(&format!("set_property DRIVE {val} [get_ports {port}]\n"));
    }
    for (port, val) in &prj.slews {
        blob.push_str(&format!("set_property SLEW {val} [get_ports {port}]\n"));
    }
    for (port, val) in &prj.pulltypes {
        blob.push_str(&format!(
            "set_property PULLTYPE {val} [get_ports {port}]\n"
        ));
    }
    for (port, val) in &prj.diff_terms {
        blob.push_str(&format!(
            "set_property DIFF_TERM {val} [get_ports {port}]\n"
        ));
    }
    for (port, val) in &prj.in_terms {
        blob.push_str(&format!(
            "set_property IN_TERM {val} [get_ports {port}]\n"
        ));
    }
    if blob.trim().is_empty() {
        return Ok(Constraints::default());
    }
    load_xdc(&blob)
}

pub fn opt_design(d: &mut Design) -> usize {
    let pins = d.pin_index();
    let iob_nets: std::collections::HashSet<&str> = d
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::IobOut))
        .filter_map(|c| pins.net_on(&c.name, "I"))
        .collect();
    let mut d_to_ff: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for f in &d.cells {
        if matches!(f.kind, CellKind::Hff) {
            if let Some(dnet) = pins.net_on(&f.name, "D") {
                d_to_ff.entry(dnet).or_insert(f.name.as_str());
            }
        }
    }
    let mut drop: std::collections::HashSet<String> = std::collections::HashSet::new();
    for c in &d.cells {
        let CellKind::Lut6 { init: 0 } = c.kind else {
            continue;
        };
        let Some(o) = pins.net_on(&c.name, "O") else {
            continue;
        };
        let Some(ff_name) = d_to_ff.get(o).copied() else {
            continue;
        };
        let Some(ff) = d.cell(ff_name) else {
            continue;
        };
        let q = pins.net_on(ff_name, "Q").unwrap_or("");
        if iob_nets.contains(q) {
            continue;
        }
        if c.attrs.flag("DONT_TOUCH") || c.attrs.flag("keep") || ff.attrs.flag("DONT_TOUCH") {
            continue;
        }
        drop.insert(c.name.clone());
        drop.insert(ff.name.clone());
    }
    let n = drop.len() / 2;
    d.cells.retain(|c| !drop.contains(&c.name));
    d.rebuild_indexes();
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_device::Device;
    use helion_ir::{CellKind, Design, PortDir};

    #[test]
    fn dual_mode_same_hash_and_ckpt() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut proj = Session::new(Mode::Project);
        let mut np = Session::new(Mode::NonProject);
        proj.impl_design(Design::structural_blinky(), &dev).unwrap();
        np.impl_design(Design::structural_blinky(), &dev).unwrap();
        assert_eq!(proj.blinky_hash(), np.blinky_hash());
        let ck = proj.checkpoint();
        let (mode, h) = Session::restore(&ck).unwrap();
        assert_eq!(mode, Mode::Project);
        assert_eq!(Some(h), proj.blinky_hash());
        let cells = get_cells(proj.design.as_ref().unwrap(), Some("lut"));
        assert_eq!(cells, vec!["u_lut"]);
        let (_, _, ir) = Session::restore_with_ir(&ck).unwrap();
        let ir = ir.expect("checkpoint must embed HNF");
        assert_eq!(ir.name, "blinky");
        assert_eq!(ir.lut_inits(), Design::structural_blinky().lut_inits());
        assert!(get_nets(proj.design.as_ref().unwrap(), Some("q")).contains(&"q".into()));
        assert!(get_pins(proj.design.as_ref().unwrap(), "u_lut").iter().any(|p| p.ends_with("/I0")));
    }

    #[test]
    fn opt_drops_dead_const0_and_eco_changes_hash() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut d = Design::structural_blinky();
        d.add_cell("dead_lut", CellKind::Lut6 { init: 0 });
        d.add_cell("dead_ff", CellKind::Hff);
        d.connect("clk", "dead_ff", "CLK");
        d.connect("dead_d", "dead_lut", "O");
        d.connect("dead_d", "dead_ff", "D");
        d.connect("dead_q", "dead_ff", "Q");
        d.connect("dead_q", "dead_lut", "I0");
        let before = d.cells.len();
        let n = opt_design(&mut d);
        assert_eq!(n, 1);
        assert!(d.cells.len() < before);
        assert!(d.cell("u_lut").is_some());
        assert!(d.cell("dead_lut").is_none());

        let mut kept = Design::structural_blinky();
        kept.add_cell("dead_lut", CellKind::Lut6 { init: 0 });
        kept.add_cell("dead_ff", CellKind::Hff);
        kept.connect("clk", "dead_ff", "CLK");
        kept.connect("dead_d", "dead_lut", "O");
        kept.connect("dead_d", "dead_ff", "D");
        kept.connect("dead_q", "dead_ff", "Q");
        kept.connect("dead_q", "dead_lut", "I0");
        kept.dont_touch("dead_lut").unwrap();
        let before = kept.cells.len();
        assert_eq!(opt_design(&mut kept), 0);
        assert_eq!(kept.cells.len(), before, "DONT_TOUCH must survive opt");

        let mut s = Session::new(Mode::NonProject);
        s.impl_design(Design::structural_blinky(), &dev).unwrap();
        let h0 = s.blinky_hash();
        s.eco(&dev, "u_lut", 0xAAAA_AAAA_AAAA_AAAA).unwrap();
        assert_ne!(s.blinky_hash(), h0, "ECO must change bitstream hash");
        let _ = PortDir::In;
    }

    #[test]
    fn write_bitstream_refuses_missing_design_and_unrouted() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut s = Session::new(Mode::NonProject);
        let e = s.write_bitstream(&dev).unwrap_err();
        assert!(e.contains("no design"), "{e}");
        s.synth_design(Design::structural_counter());
        let e = s.write_bitstream(&dev).unwrap_err();
        assert!(e.contains("not routed"), "{e}");
        s.place_design(&dev).unwrap();
        let e = s.write_bitstream(&dev).unwrap_err();
        assert!(e.contains("not routed"), "{e}");
        s.route_design(&dev).unwrap();
        let bits = s.write_bitstream(&dev).unwrap();
        assert_eq!(bits.packets.len(), 185, "counter golden size");
        assert!(!bits.frames.is_empty());
    }

    #[test]
    fn tcl_session_steps_hit_engines_and_hckp_restores_hash() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut s = Session::new(Mode::NonProject);
        s.synth_design(Design::structural_counter());
        assert!(s.design.is_some());
        assert!(s.placed.is_none(), "synth must not place");
        let n = s.opt_design_step().unwrap();
        let _ = n;
        s.place_design(&dev).unwrap();
        assert!(s.placed.is_some());
        assert!(s.routed.is_none(), "place_design must not route");
        s.route_design(&dev).unwrap();
        assert!(s.routed.is_some());
        assert!(s.bitstream.is_none(), "route_design must not bitgen");
        let bits = s.write_bitstream(&dev).unwrap();
        assert!(!bits.frames.is_empty());
        let hnf = s.write_hnf().unwrap();
        assert!(hnf.starts_with("HNF 1"));
        let t = s.report_timing(&dev).unwrap();
        assert!(t.contains("WNS_PS="), "{t}");
        assert!(t.contains("HOLD_PS="), "{t}");
        assert!(!t.contains("report_timing ok"), "must hit STA engine: {t}");
        let u = s.report_utilization(&dev).unwrap();
        assert!(u.contains("LUTFF=4/8192"), "{u}");
        s.set_property("DONT_TOUCH", "true", "u_lut0").unwrap();
        assert!(s.design.as_ref().unwrap().cell("u_lut0").unwrap().attrs.flag("DONT_TOUCH"));
        s.open_hw_manager();
        // USB=0: product program_hw (auto) must refuse DONE — no soft-hold / no sim invent.
        let board_err = s.program_hw(&dev).unwrap_err();
        assert!(
            board_err.contains("refused DONE") || board_err.contains("no USB") || board_err.contains("programmer"),
            "{board_err}"
        );
        assert!(!board_err.contains("soft-hold"), "{board_err}");
        let hw = s.program_hw_cable(&dev, "sim").unwrap();
        assert!(hw.contains("DONE=1"), "{hw}");
        assert!(hw.contains("not board DONE"), "{hw}");
        let h0 = s.blinky_hash().unwrap();
        let ck = s.checkpoint();
        let s2 = Session::restore_session(&ck, &dev).unwrap();
        assert_eq!(s2.blinky_hash(), Some(h0), ".hckp restore must match bitstream hash");
        let die = dev.report_die();
        assert!(die.contains("HL10T-C32-1"));
        s.mark_debug("q3").unwrap();
        assert!(s.design.as_ref().unwrap().net("q3").unwrap().attrs.flag("mark_debug"));
        assert!(get_cells(s.design.as_ref().unwrap(), None).iter().any(|c| c.contains("lut")));
        assert!(!get_pins(s.design.as_ref().unwrap(), "u_lut0").is_empty());
    }

    #[test]
    fn format_prj_round_trips_load() {
        let prj = load_prj(
            "part HL10T-C32-1\nread_sv examples/counter.sv\nread_xdc examples/counter.sdc\n",
        )
        .unwrap();
        let text = format_prj(&prj);
        let again = load_prj(&text).unwrap();
        assert_eq!(again.part, "HL10T-C32-1");
        assert_eq!(again.sources, vec!["examples/counter.sv"]);
        assert_eq!(again.constraint_files, vec!["examples/counter.sdc"]);
    }

    #[test]
    fn project_file_parses_vivado_shaped_commands() {
        let prj = load_prj(
            r#"
part HL10T-C32-1
read_sv examples/blinky.sv
create_clock -period 10.000 [get_ports clk]
set_property PACKAGE_PIN IOB_X2Y0 [get_ports led]
set_property IOSTANDARD LVCMOS18 [get_ports led]
set_property DRIVE 12 [get_ports led]
set_property SLEW SLOW [get_ports led]
set_property PULLTYPE NONE [get_ports led]
set_property DIFF_TERM FALSE [get_ports led]
set_property IN_TERM NONE [get_ports led]
"#,
        )
        .unwrap();
        assert_eq!(prj.part, "HL10T-C32-1");
        assert_eq!(prj.sources, vec!["examples/blinky.sv"]);
        assert!(prj.constraint_files.is_empty());
        assert!(prj.top.is_none());
        assert_eq!(prj.sdc.len(), 1);
        assert_eq!(prj.package_pins, vec![("led".into(), "IOB_X2Y0".into())]);
        assert_eq!(prj.iostandards, vec![("led".into(), "LVCMOS18".into())]);
        assert_eq!(prj.drives, vec![("led".into(), "12".into())]);
        assert_eq!(prj.slews, vec![("led".into(), "SLOW".into())]);
        assert_eq!(prj.pulltypes, vec![("led".into(), "NONE".into())]);
        assert_eq!(prj.diff_terms, vec![("led".into(), "FALSE".into())]);
        assert_eq!(prj.in_terms, vec![("led".into(), "NONE".into())]);
        assert!(load_prj("part X\n").is_err());
    }

    #[test]
    fn project_file_read_xdc_multi_source_and_top() {
        let prj = load_prj(
            r#"
part HL10T-C32-1
read_sv examples/multi/tog.sv
read_sv examples/multi/top.sv
read_xdc examples/multi/multi.sdc
top top
set_property TOP top
create_clock -period 10.000 [get_ports clk]
"#,
        )
        .unwrap();
        assert_eq!(prj.sources.len(), 2);
        assert_eq!(prj.constraint_files, vec!["examples/multi/multi.sdc"]);
        assert_eq!(prj.top.as_deref(), Some("top"));
        assert_eq!(prj.sdc.len(), 1);
    }

    #[test]
    fn project_file_read_ip_parses() {
        let prj = load_prj(
            r#"
part HL10T-C32-1
read_ip ip/h_gpio/h_gpio.helion
top h_gpio
"#,
        )
        .unwrap();
        assert!(prj.sources.is_empty());
        assert_eq!(prj.ip_packages, vec!["ip/h_gpio/h_gpio.helion"]);
        assert_eq!(prj.top.as_deref(), Some("h_gpio"));
    }

    #[test]
    fn strategies_move_wns_and_incremental_reuses_cells() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut def = Session::new(Mode::NonProject);
        def.synth_design(Design::structural_counter());
        def.impl_with_strategy(&dev, ImplStrategy::Default).unwrap();
        let t_def = def.report_timing(&dev).unwrap();
        let mut rt = Session::new(Mode::NonProject);
        rt.synth_design(Design::structural_counter());
        rt.impl_with_strategy(&dev, ImplStrategy::RuntimeOpt).unwrap();
        let t_rt = rt.report_timing(&dev).unwrap();
        assert_ne!(t_def, t_rt, "RuntimeOpt WNS must differ from Default: {t_def} vs {t_rt}");
        let mut phys = Session::new(Mode::NonProject);
        phys.synth_design(Design::structural_counter());
        phys.impl_with_strategy(&dev, ImplStrategy::PhysOpt).unwrap();
        let t_phys = phys.report_timing(&dev).unwrap();
        assert_ne!(t_phys, t_def, "PhysOpt extra hops must move WNS: {t_phys} vs {t_def}");

        let prev = def.placed.clone().unwrap();
        let reuse = def.incremental_place(&dev, &prev).unwrap();
        assert_eq!(reuse.cell_pct(), 100, "{}", reuse.text());
        def.insert_eco_lut("ECO_LUT3", 0x8).unwrap();
        let chk = def.check_eco().unwrap();
        assert!(chk.contains("ECO_LUT3"), "{chk}");
        let reuse2 = def.incremental_place(&dev, &prev).unwrap();
        assert!(reuse2.reused_cells < reuse2.cells, "{}", reuse2.text());
        assert!(reuse2.reused_cells > 0, "{}", reuse2.text());
        def.route_design(&dev).unwrap();
        let led = def
            .placed
            .as_ref()
            .unwrap()
            .packed
            .iobs
            .first()
            .unwrap()
            .from_net
            .clone();
        let before = def.routed.as_ref().unwrap().iob_src[0].delay_ps;
        def.fix_route(&led, 3).unwrap();
        assert_eq!(
            def.routed.as_ref().unwrap().iob_src[0].delay_ps,
            before + 3 * helion_route::HOP_DELAY_PS
        );
        def.unroute_net(&led).unwrap();
        assert_eq!(def.routed.as_ref().unwrap().iob_src[0].delay_ps, 0);
    }

    #[test]
    fn format_prj_round_trips_pblock_impl_run_checkpoint() {
        let prj = load_prj(
            r#"
part HL10T-C32-1
read_sv examples/counter.sv
read_xdc examples/counter.sdc
create_pblock pblock_0
resize_pblock pblock_0 -add {CLB_X5Y1:CLB_X8Y8}
add_cells_to_pblock pblock_0 u_lut0
create_run impl_1 -strategy Default
create_run impl_runtime -strategy RuntimeOpt
write_checkpoint counter.hckp
"#,
        )
        .unwrap();
        assert_eq!(prj.pblocks.len(), 1);
        assert_eq!(prj.pblocks[0].name, "pblock_0");
        assert!(prj.pblocks[0].ranged);
        assert_eq!(prj.pblocks[0].x0, 5);
        assert_eq!(prj.pblocks[0].y0, 1);
        assert_eq!(prj.pblocks[0].x1, 8);
        assert_eq!(prj.pblocks[0].y1, 8);
        assert_eq!(prj.pblocks[0].cells, vec!["u_lut0"]);
        assert_eq!(
            prj.impl_runs,
            vec![
                PrjImplRun {
                    name: "impl_1".into(),
                    strategy: "Default".into(),
                },
                PrjImplRun {
                    name: "impl_runtime".into(),
                    strategy: "RuntimeOpt".into(),
                },
            ]
        );
        assert_eq!(prj.checkpoint_path.as_deref(), Some("counter.hckp"));

        let text = format_prj(&prj);
        assert!(text.contains("create_pblock pblock_0"), "{text}");
        assert!(
            text.contains("resize_pblock pblock_0 -add {CLB_X5Y1:CLB_X8Y8}"),
            "{text}"
        );
        assert!(
            text.contains("add_cells_to_pblock pblock_0 u_lut0"),
            "{text}"
        );
        assert!(
            text.contains("create_run impl_1 -strategy Default"),
            "{text}"
        );
        assert!(
            text.contains("create_run impl_runtime -strategy RuntimeOpt"),
            "{text}"
        );
        assert!(text.contains("write_checkpoint counter.hckp"), "{text}");

        let again = load_prj(&text).unwrap();
        assert_eq!(again.pblocks, prj.pblocks);
        assert_eq!(again.impl_runs, prj.impl_runs);
        assert_eq!(again.checkpoint_path, prj.checkpoint_path);
        assert_eq!(again.sources, prj.sources);
        assert_eq!(again.constraint_files, prj.constraint_files);
    }

    #[test]
    fn load_prj_rejects_unparseable_pblock_range() {
        let err = load_prj(
            r#"
part HL10T-C32-1
read_sv examples/counter.sv
create_pblock pblock_0
resize_pblock pblock_0 -add {NOT_A_CLB_RANGE}
"#,
        )
        .unwrap_err();
        assert!(
            err.contains("resize_pblock") && err.contains("NOT_A_CLB_RANGE"),
            "{err}"
        );

        let err = load_prj(
            r#"
part HL10T-C32-1
read_sv examples/counter.sv
create_pblock pblock_0
resize_pblock pblock_0 -add {FOO_X0Y0:BAR_X1Y1}
"#,
        )
        .unwrap_err();
        assert!(
            err.contains("resize_pblock") && err.contains("FOO_X0Y0:BAR_X1Y1"),
            "{err}"
        );

        let err = load_prj(
            r#"
part HL10T-C32-1
read_sv examples/counter.sv
create_pblock pblock_0 -add {GARBAGE}
"#,
        )
        .unwrap_err();
        assert!(
            err.contains("create_pblock") && err.contains("GARBAGE"),
            "{err}"
        );

        let ok = load_prj(
            r#"
part HL10T-C32-1
read_sv examples/counter.sv
create_pblock pblock_0
"#,
        )
        .unwrap();
        assert_eq!(ok.pblocks.len(), 1);
        assert!(!ok.pblocks[0].ranged, "create_pblock without -add stays unranged");
    }

    #[test]
    fn disk_hckp_restore_eco_changes_bitstream_hash() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut s = Session::new(Mode::Project);
        s.impl_design(Design::structural_counter(), &dev).unwrap();
        let t0 = s.report_timing(&dev).unwrap();
        assert!(
            t0.contains("WNS_PS=9640"),
            "empty-XDC counter gold must hold before checkpoint: {t0}"
        );
        let h0 = s.blinky_hash().expect("impl must bitgen");
        let frames0 = s.bitstream.as_ref().unwrap().frames.len();
        assert!(frames0 > 0, "impl frames must be non-empty");

        let dir = std::env::temp_dir().join(format!(
            "helion-hckp-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("counter.hckp");
        let wr = s.write_checkpoint_to(&path).unwrap();
        assert!(wr.contains("bytes="), "{wr}");
        assert!(path.is_file(), "write_checkpoint must create {}", path.display());
        drop(s);

        let mut s2 = Session::open_checkpoint(&path, &dev).unwrap();
        assert!(
            s2.impl_checkpoint.is_some(),
            "disk restore must seed impl_checkpoint for incremental place"
        );
        let t1 = s2.report_timing(&dev).unwrap();
        assert!(
            t1.contains("WNS_PS=9640"),
            "reopen must hold empty-XDC counter gold: {t1}"
        );
        assert_eq!(
            s2.blinky_hash(),
            Some(h0),
            "disk .hckp restore must match bitstream hash"
        );
        s2.eco(&dev, "u_lut0", 0xAAAA_AAAA_AAAA_AAAA).unwrap();
        s2.write_bitstream(&dev).unwrap();
        let frames1 = s2.bitstream.as_ref().map(|b| b.frames.len()).unwrap_or(0);
        assert!(
            frames1 > 0,
            "ECO write_bitstream must keep non-empty frames"
        );
        let h1 = s2.blinky_hash().expect("ECO bitstream");
        assert_ne!(h1, h0, "ECO LUT must change bitstream hash ({h0:#x} vs {h1:#x})");
        eprintln!(
            "disk_hckp restore WNS_PS=9640 hash={h0:#x} ECO hash={h1:#x} frames={frames1}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_checkpoint_to_refuses_empty_bitstream() {
        let mut s = Session::new(Mode::NonProject);
        let err = s
            .write_checkpoint_to("/tmp/helion-empty.hckp")
            .unwrap_err();
        assert!(err.contains("not placed") || err.contains("empty bitstream"), "{err}");
        assert!(s.impl_checkpoint.is_none());
        s.synth_design(Design::structural_counter());
        let err = s
            .write_checkpoint_to("/tmp/helion-empty.hckp")
            .unwrap_err();
        assert!(err.contains("not placed"), "{err}");
        assert!(s.impl_checkpoint.is_none());

        let dev = Device::load_part("HL10T-C32-1").unwrap();
        s.place_design(&dev).unwrap();
        assert!(s.placed.is_some());
        assert!(s.bitstream.is_none());
        let err = s
            .write_checkpoint_to("/tmp/helion-empty.hckp")
            .unwrap_err();
        assert!(err.contains("empty bitstream"), "{err}");
        assert!(
            s.impl_checkpoint.is_none(),
            "empty-bitstream error must not seed impl_checkpoint"
        );
    }

    #[test]
    fn write_checkpoint_to_does_not_mutate_on_disk_error() {
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let mut s = Session::new(Mode::NonProject);
        s.impl_design(Design::structural_counter(), &dev).unwrap();
        assert!(s.bitstream.as_ref().is_some_and(|b| !b.frames.is_empty()));
        assert!(s.impl_checkpoint.is_none());

        let dir = std::env::temp_dir().join(format!(
            "helion-hckp-isdir-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let err = s.write_checkpoint_to(&dir).unwrap_err();
        assert!(err.contains("write_checkpoint"), "{err}");
        assert!(
            s.impl_checkpoint.is_none(),
            "disk-write error must not seed impl_checkpoint"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

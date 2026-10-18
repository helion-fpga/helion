//! Automatic `mark_debug` Netlist Insertion (`insert_debug_cores` — UG908 parity).
//!
//! Scans a [`Design`] for `(* mark_debug = "true" *)` attributes (and/or explicit
//! probe lists), stitches synthesizable ILA sample-buffer LUT+FFs, trigger
//! comparator LUTs, capture BRAM18, and `BSCANE2` + `xsdbm_dbg_hub` BSCAN routing
//! into the netlist, and emits a [`DebugProbesLtx`] `.ltx` descriptor file.

use crate::bscan::{BscanDebugHub, BscanUserChain, DebugCoreKind, deterministic_core_uuid};
use crate::ltx::{
    DebugProbesLtx, LtxCoreDescriptor, LtxProbeChannel, LtxProbeDirection,
    group_nets_into_probe_channels,
};
use crate::{
    CAPTURE_BRAM, IlaTriggerKind, TRIG_FSM_CELL, insert_capture_bram, insert_ila,
    insert_match_unit, strip_capture_bram, strip_ila, strip_match_unit,
};
use helion_ir::{CellKind, Design, PortDir, intern, resolve};

/// Configuration for [`insert_debug_cores`].
#[derive(Clone, Debug)]
pub struct DebugInsertConfig {
    /// Additional explicit probe net names (merged with `(* mark_debug = "true" *)` nets).
    pub explicit_probes: Vec<String>,
    /// ILA core instance name in netlist and `.ltx` (default: `"u_ila_0"`).
    pub ila_core_name: String,
    /// BSCAN Debug Hub instance name (default: `"u_dbg_hub"`).
    pub hub_name: String,
    /// JTAG `BSCANE2` user chain (`1..=4`, default `1` = `USER1`).
    pub bscan_chain: u8,
    /// Sample buffer depth in words (default `64`, clamped `1..=255` for Bram18 INIT).
    pub sample_depth: usize,
    /// Number of multi-stage trigger sequencer states synthesized (`1..=16`).
    pub trigger_stages: usize,
    /// Initial hardware match unit mode (default `IlaTriggerKind::Rising`).
    pub default_trigger: IlaTriggerKind,
    /// Optional clock net name (auto-detected from design ports if `None`).
    pub clock_net: Option<String>,
    /// Optional VIO input probe nets (`PROBE_IN*`) to stitch into a `u_vio_0` core.
    pub vio_probes_in: Vec<String>,
    /// Optional VIO output stimulus nets (`PROBE_OUT*`) to stitch into a `u_vio_0` core.
    pub vio_probes_out: Vec<String>,
    /// Whether to also attach a JTAG-to-AXI4-Lite master core (`u_jtag_axi_0`) to the BSCAN hub.
    pub insert_jtag_axi: bool,
}

impl Default for DebugInsertConfig {
    fn default() -> Self {
        Self {
            explicit_probes: Vec::new(),
            ila_core_name: "u_ila_0".to_string(),
            hub_name: "u_dbg_hub".to_string(),
            bscan_chain: 1,
            sample_depth: 64,
            trigger_stages: 4,
            default_trigger: IlaTriggerKind::Rising,
            clock_net: None,
            vio_probes_in: Vec::new(),
            vio_probes_out: Vec::new(),
            insert_jtag_axi: false,
        }
    }
}

impl DebugInsertConfig {
    pub fn with_probes(probes: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            explicit_probes: probes.into_iter().map(Into::into).collect(),
            ..Self::default()
        }
    }
}

/// Summary and artifacts produced by [`insert_debug_cores`].
#[derive(Clone, Debug)]
pub struct DebugCoreInsertionReport {
    pub ila_core_name: String,
    pub hub_cell_name: String,
    pub bscan_cell_name: String,
    pub bscan_chain: u8,
    pub clock_net: String,
    /// Ordered physical net names probed by the ILA core.
    pub probed_nets: Vec<String>,
    /// Logical probe channels (`PROBE0`, `PROBE1`, ...) with bus widths.
    pub probe_channels: Vec<LtxProbeChannel>,
    /// Names of all cells inserted into the [`Design`].
    pub inserted_cells: Vec<String>,
    /// Configured BSCAN Debug Hub multiplexer with registered slots.
    pub hub: BscanDebugHub,
    /// Generated `.ltx` debug probes descriptor ready for serialization or file export.
    pub ltx: DebugProbesLtx,
}

const CMP_LUT_CELL: &str = "ila_trig_cmp_lut";
const CMP_HIT_NET: &str = "ila_trig_cmp_hit";

/// Collect all net names in `design` carrying `mark_debug` / `MARK_DEBUG` truthy attributes.
pub fn collect_marked_debug_nets(design: &Design) -> Vec<String> {
    let mut nets = Vec::new();
    for n in &design.nets {
        let marked = n.attrs.flag("mark_debug")
            || n.attrs.flag("MARK_DEBUG")
            || n.attrs
                .map
                .iter()
                .any(|(k, v)| {
                    k.eq_ignore_ascii_case("mark_debug")
                        && matches!(v.trim(), "1" | "true" | "TRUE" | "yes" | "YES")
                });
        if marked {
            let name = resolve(n.name).to_string();
            if !nets.contains(&name) {
                nets.push(name);
            }
        }
    }
    nets
}

fn detect_clock_net(design: &Design, explicit: Option<&str>) -> String {
    if let Some(c) = explicit {
        return c.to_string();
    }
    for p in &design.ports {
        if p.dir == PortDir::In {
            let n = resolve(p.name);
            if n.eq_ignore_ascii_case("clk") || n.to_ascii_lowercase().contains("clk") {
                return n.to_string();
            }
        }
    }
    "clk".to_string()
}

/// Remove any previously inserted BSCAN hub, comparator LUT, VIO, JTAG-AXI, and ILA debug cells.
pub fn strip_debug_cores(design: &mut Design) {
    let marked: Vec<String> = design
        .cells
        .iter()
        .filter_map(|c| match &c.kind {
            CellKind::Ila { net } => Some(resolve(*net).to_string()),
            _ => None,
        })
        .collect();
    for net in &marked {
        strip_ila(design, net);
    }
    strip_capture_bram(design);
    strip_match_unit(design);

    let drop_cells: Vec<String> = design
        .cells
        .iter()
        .map(|c| resolve(c.name).to_string())
        .filter(|name| {
            name == CMP_LUT_CELL
                || name.starts_with("bscan_usr")
                || name == "u_dbg_hub"
                || name.starts_with("dbg_hub_")
                || name.starts_with("u_vio_")
                || name.starts_with("vio_")
                || name.starts_with("u_jtag_axi_")
        })
        .collect();

    design
        .cells
        .retain(|c| !drop_cells.iter().any(|d| d == resolve(c.name)));
    design.nets.retain(|n| {
        let name = resolve(n.name);
        name != CMP_HIT_NET
            && !name.starts_with("dbg_hub_")
            && !name.starts_with("bscan_jtag_")
            && !name.starts_with("vio_stim_")
    });
    for n in &mut design.nets {
        n.endpoints
            .retain(|e| !drop_cells.iter().any(|d| d == resolve(e.cell)));
    }
    design.rebuild_indexes();
}

/// Scan `design` for `(* mark_debug = "true" *)` nets (plus `cfg.explicit_probes`),
/// automatically stitch synthesizable ILA sample-buffer + trigger comparator cells
/// and `BSCANE2` + `xsdbm_dbg_hub` BSCAN routing into `design`, and return a
/// [`DebugCoreInsertionReport`] containing the `.ltx` probe descriptor.
pub fn insert_debug_cores(
    design: &mut Design,
    cfg: &DebugInsertConfig,
) -> Result<DebugCoreInsertionReport, String> {
    let chain = BscanUserChain::from_index(cfg.bscan_chain)?;
    let stages = cfg.trigger_stages.clamp(1, 16);
    let depth = cfg.sample_depth.clamp(1, 255);

    // 1. Gather marked_debug nets + explicit_probes in deterministic order.
    let mut probed_nets = collect_marked_debug_nets(design);
    for p in &cfg.explicit_probes {
        if !probed_nets.iter().any(|n| n == p) {
            probed_nets.push(p.clone());
        }
    }
    if probed_nets.is_empty() {
        return Err(
            "insert_debug_cores: no nets have (* mark_debug = \"true\" *) and explicit_probes is empty"
                .into(),
        );
    }

    // Validate all probed nets exist in design before mutating.
    for net in &probed_nets {
        if design.net(net).is_none() {
            return Err(format!("insert_debug_cores: net '{net}' not found in design"));
        }
    }
    for net in cfg.vio_probes_in.iter().chain(cfg.vio_probes_out.iter()) {
        if design.net(net).is_none() {
            return Err(format!(
                "insert_debug_cores: VIO probe net '{net}' not found in design"
            ));
        }
    }

    // Clean any prior debug core insertion for idempotency.
    strip_debug_cores(design);

    let clock_net = detect_clock_net(design, cfg.clock_net.as_deref());
    let mut inserted_cells = Vec::new();

    // 2. Stitch synthesizable ILA sample-buffer LUT+FF cells for each probed net.
    for net in &probed_nets {
        insert_ila(design, net)?;
        inserted_cells.push(format!("ila_{net}"));
        inserted_cells.push(format!("ila_{net}_lut"));
        inserted_cells.push(format!("ila_{net}_ff"));
    }

    // 3. Stitch synthesizable multi-probe trigger comparator LUT6 (up to 6 inputs I0..I5)
    //    plus the match-unit delay LUT+FF + match LUT + `h_ila_trig` FSM IP cell.
    let first_probe = &probed_nets[0];
    insert_match_unit(design, first_probe, cfg.default_trigger)?;
    inserted_cells.push("ila_match_delay_lut".to_string());
    inserted_cells.push("ila_match_delay_ff".to_string());
    inserted_cells.push("ila_match_lut".to_string());
    inserted_cells.push(TRIG_FSM_CELL.to_string());

    // Multi-probe boolean AND comparator LUT across up to 6 probes (O = &I[0..k]).
    let cmp_inputs = probed_nets.len().min(6);
    let mut cmp_init = 0u64;
    let mask = (1usize << cmp_inputs) - 1;
    for addr in 0..64usize {
        if (addr & mask) == mask {
            cmp_init |= 1u64 << addr;
        }
    }
    design.add_cell(CMP_LUT_CELL, CellKind::Lut6 { init: cmp_init });
    for (pin_idx, net) in probed_nets.iter().take(6).enumerate() {
        design.connect(net, CMP_LUT_CELL, &format!("I{pin_idx}"));
    }
    design.connect(CMP_HIT_NET, CMP_LUT_CELL, "O");
    inserted_cells.push(CMP_LUT_CELL.to_string());

    // Annotate trigger FSM IP cell with core parameters.
    if let Some(fsm) = design.cell_mut(TRIG_FSM_CELL) {
        fsm.attrs.set("CORE_NAME", cfg.ila_core_name.clone());
        fsm.attrs.set("C_NUM_PROBES", probed_nets.len().to_string());
        fsm.attrs.set("C_DATA_DEPTH", depth.to_string());
        fsm.attrs.set("C_TRIG_STAGES", stages.to_string());
    }

    // 4. Stitch on-fabric Bram18 sample buffer (`ila_capture_ram`).
    insert_capture_bram(design, depth)?;
    if let Some(bram) = design.cell_mut(CAPTURE_BRAM) {
        bram.attrs.set("CORE_NAME", cfg.ila_core_name.clone());
        bram.attrs.set("C_DATA_DEPTH", depth.to_string());
        bram.attrs.set("C_NUM_PROBES", probed_nets.len().to_string());
    }
    inserted_cells.push(CAPTURE_BRAM.to_string());

    // 5. Stitch `BSCANE2` primitive + `xsdbm_dbg_hub` BSCAN Debug Hub multiplexer.
    let bscan_cell_name = format!("bscan_usr{}", chain.index());
    design.add_cell(
        &bscan_cell_name,
        CellKind::BlackBox {
            module: intern("BSCANE2"),
        },
    );
    if let Some(c) = design.cell_mut(&bscan_cell_name) {
        c.attrs.set("JTAG_CHAIN", chain.index().to_string());
        c.attrs.set("DONT_TOUCH", "true");
    }
    inserted_cells.push(bscan_cell_name.clone());

    let hub_cell_name = cfg.hub_name.clone();
    design.add_cell(
        &hub_cell_name,
        CellKind::BlackBox {
            module: intern("xsdbm_dbg_hub"),
        },
    );
    inserted_cells.push(hub_cell_name.clone());

    // Route JTAG BSCAN signals between BSCANE2 and xsdbm_dbg_hub.
    for sig in [
        "tck", "tdi", "tdo", "tms", "sel", "shift", "capture", "update",
    ] {
        let net_name = format!("bscan_jtag_{sig}");
        let pin_upper = sig.to_ascii_uppercase();
        design.connect(&net_name, &bscan_cell_name, &pin_upper);
        design.connect(&net_name, &hub_cell_name, &format!("BSCAN_{pin_upper}"));
    }
    if design.net(&clock_net).is_some() {
        design.connect(&clock_net, &hub_cell_name, "CLK");
        design.connect(&clock_net, TRIG_FSM_CELL, "CLK");
    }

    // 6. Build BscanDebugHub & register ILA core on slot 0.
    let mut hub = BscanDebugHub::new(&hub_cell_name, chain);
    let ila_channels =
        group_nets_into_probe_channels(&probed_nets, "PROBE", LtxProbeDirection::Probe);
    let ila_slot = hub.register_core(&cfg.ila_core_name, DebugCoreKind::Ila, ila_channels.len())?;

    // Connect hub slot 0 BSCAN mux nets to the ILA trigger FSM / capture controller.
    for port_sig in ["tdi", "tdo", "clk", "ctrl"] {
        let mux_net = format!("dbg_hub_slot{ila_slot}_{port_sig}");
        design.connect(
            &mux_net,
            &hub_cell_name,
            &format!("SLOT{ila_slot}_{}", port_sig.to_ascii_uppercase()),
        );
        design.connect(
            &mux_net,
            TRIG_FSM_CELL,
            &format!("BSCAN_{}", port_sig.to_ascii_uppercase()),
        );
    }

    let mut ltx = DebugProbesLtx::new(resolve(design.name), &hub_cell_name, chain.index());
    let ila_uuid = format!(
        "{:032X}",
        deterministic_core_uuid(&hub_cell_name, ila_slot, &cfg.ila_core_name)
    );
    ltx.cores.push(LtxCoreDescriptor {
        core_name: cfg.ila_core_name.clone(),
        core_kind: "ila".to_string(),
        slot_id: ila_slot,
        uuid: ila_uuid,
        clock_net: clock_net.clone(),
        sample_depth: depth,
        trigger_stages: stages,
        probes: ila_channels.clone(),
    });

    // 7. Optional VIO core insertion on next BSCAN slot.
    if !cfg.vio_probes_in.is_empty() || !cfg.vio_probes_out.is_empty() {
        let vio_core_name = "u_vio_0".to_string();
        design.add_cell(
            &vio_core_name,
            CellKind::BlackBox {
                module: intern("h_vio_core"),
            },
        );
        inserted_cells.push(vio_core_name.clone());
        let mut vio_channels = group_nets_into_probe_channels(
            &cfg.vio_probes_in,
            "PROBE_IN",
            LtxProbeDirection::ProbeIn,
        );
        let out_channels = group_nets_into_probe_channels(
            &cfg.vio_probes_out,
            "PROBE_OUT",
            LtxProbeDirection::ProbeOut,
        );
        vio_channels.extend(out_channels);

        let vio_slot =
            hub.register_core(&vio_core_name, DebugCoreKind::Vio, vio_channels.len())?;
        if let Some(vc) = design.cell_mut(&vio_core_name) {
            vc.attrs
                .set("C_NUM_PROBE_IN", cfg.vio_probes_in.len().to_string());
            vc.attrs
                .set("C_NUM_PROBE_OUT", cfg.vio_probes_out.len().to_string());
            vc.attrs.set("BSCAN_SLOT", vio_slot.to_string());
        }
        for (i, n) in cfg.vio_probes_in.iter().enumerate() {
            design.connect(n, &vio_core_name, &format!("PROBE_IN{i}"));
        }
        for (i, n) in cfg.vio_probes_out.iter().enumerate() {
            design.connect(n, &vio_core_name, &format!("PROBE_OUT{i}"));
        }
        for port_sig in ["tdi", "tdo", "ctrl"] {
            let mux_net = format!("dbg_hub_slot{vio_slot}_{port_sig}");
            design.connect(
                &mux_net,
                &hub_cell_name,
                &format!("SLOT{vio_slot}_{}", port_sig.to_ascii_uppercase()),
            );
            design.connect(
                &mux_net,
                &vio_core_name,
                &format!("BSCAN_{}", port_sig.to_ascii_uppercase()),
            );
        }
        let vio_uuid = format!(
            "{:032X}",
            deterministic_core_uuid(&hub_cell_name, vio_slot, &vio_core_name)
        );
        ltx.cores.push(LtxCoreDescriptor {
            core_name: vio_core_name,
            core_kind: "vio".to_string(),
            slot_id: vio_slot,
            uuid: vio_uuid,
            clock_net: clock_net.clone(),
            sample_depth: 1,
            trigger_stages: 0,
            probes: vio_channels,
        });
    }

    // 8. Optional JTAG-to-AXI4-Lite master core insertion on next BSCAN slot.
    if cfg.insert_jtag_axi {
        let axi_core_name = "u_jtag_axi_0".to_string();
        design.add_cell(
            &axi_core_name,
            CellKind::BlackBox {
                module: intern("h_jtag_axi"),
            },
        );
        inserted_cells.push(axi_core_name.clone());
        let axi_slot = hub.register_core(&axi_core_name, DebugCoreKind::JtagAxi, 1)?;
        if let Some(ac) = design.cell_mut(&axi_core_name) {
            ac.attrs.set("PROTOCOL", "AXI4LITE");
            ac.attrs.set("M_AXI_ADDR_WIDTH", "32");
            ac.attrs.set("M_AXI_DATA_WIDTH", "32");
            ac.attrs.set("BSCAN_SLOT", axi_slot.to_string());
        }
        for port_sig in ["tdi", "tdo", "ctrl"] {
            let mux_net = format!("dbg_hub_slot{axi_slot}_{port_sig}");
            design.connect(
                &mux_net,
                &hub_cell_name,
                &format!("SLOT{axi_slot}_{}", port_sig.to_ascii_uppercase()),
            );
            design.connect(
                &mux_net,
                &axi_core_name,
                &format!("BSCAN_{}", port_sig.to_ascii_uppercase()),
            );
        }
        let axi_uuid = format!(
            "{:032X}",
            deterministic_core_uuid(&hub_cell_name, axi_slot, &axi_core_name)
        );
        ltx.cores.push(LtxCoreDescriptor {
            core_name: axi_core_name,
            core_kind: "jtag_axi".to_string(),
            slot_id: axi_slot,
            uuid: axi_uuid,
            clock_net: clock_net.clone(),
            sample_depth: 1,
            trigger_stages: 0,
            probes: Vec::new(),
        });
    }

    if let Some(hc) = design.cell_mut(&hub_cell_name) {
        hc.attrs.set("C_BSCAN_CHAIN", chain.index().to_string());
        hc.attrs
            .set("C_NUM_DEBUG_CORES", hub.slots.len().to_string());
        hc.attrs
            .set("VLNV", "community:helion:xsdbm_dbg_hub:3.0".to_string());
        hc.attrs.set("DONT_TOUCH", "true");
    }

    design.rebuild_indexes();

    Ok(DebugCoreInsertionReport {
        ila_core_name: cfg.ila_core_name.clone(),
        hub_cell_name,
        bscan_cell_name,
        bscan_chain: chain.index(),
        clock_net,
        probed_nets,
        probe_channels: ila_channels,
        inserted_cells,
        hub,
        ltx,
    })
}

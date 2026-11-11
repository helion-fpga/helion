//! Helion-MM block-design system: packaged cells with per-instance
//! parameters and base addresses, Helion-MM connection automation (plan +
//! apply), address assignment, and SystemVerilog emission of the top module
//! plus a real single-master Helion-MM interconnect / address decoder.
//!
//! The picture an IDE draws for a block design must match the HDL. This
//! module is the single source for both: [`emit_system`] returns the emitted
//! text together with the structure it realized (interconnect slaves, master,
//! top-level ports), so a canvas renders exactly what was instantiated.
//!
//! Port lists come from each cell's packaged RTL ([`helion_ipxact::CoreModel`]),
//! never from canned tables.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use helion_ipxact::{CoreModel, MmRole, PortDirection, is_mm_signal};

/// Aperture of every Helion-MM slave (cores decode at most `mm_addr[7:0]`).
pub const MM_APERTURE: u64 = 0x1000;
/// First peripheral base (matches [`crate::build_memory_map`]).
pub const MM_PERIPH_BASE: u64 = 0x4000_0000;

/// One block-design cell: a packaged core instance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SysCell {
    /// HDL instance name (`h_uart_0`).
    pub inst: String,
    pub model: CoreModel,
    /// User parameter values (`CONFIG.*`), validated on set.
    pub params: BTreeMap<String, String>,
    /// Assigned Helion-MM base address (slaves only).
    pub base: Option<u64>,
    /// Wired by connection automation (clock, reset, bus, external pins).
    pub connected: bool,
}

impl SysCell {
    pub fn role(&self) -> Option<MmRole> {
        self.model.mm_role()
    }
}

/// A Helion-MM block design.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BdSystem {
    pub name: String,
    pub cells: Vec<SysCell>,
}

/// One step connection automation will take.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedConnection {
    /// Cell this step wires (`""` for design-level steps).
    pub cell: String,
    /// `clock`, `reset`, `bus`, `address`, `external`.
    pub kind: &'static str,
    pub from: String,
    pub to: String,
}

/// What "Run Connection Automation" would do, computed without mutating.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AutomationPlan {
    pub steps: Vec<PlannedConnection>,
    /// Cells automation will wire.
    pub cells: Vec<String>,
    /// Cells it cannot wire, with the reason.
    pub skipped: Vec<(String, String)>,
}

/// Top-level port of the emitted block design.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopPort {
    pub name: String,
    pub dir: PortDirection,
    pub width: u32,
    /// Cell (and its port) this top port exposes; `None` = design clock/reset
    /// or the external Helion-MM master port.
    pub cell: Option<(String, String)>,
}

/// One decoded interconnect slave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedSlave {
    pub inst: String,
    pub core: String,
    pub base: u64,
    pub range: u64,
}

/// The Helion-MM interconnect the emitter instantiated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterconnectInfo {
    pub module: String,
    pub inst: String,
    /// Master cell, or `None` when the master is the external `s_mm_*` port.
    pub master: Option<String>,
    pub slaves: Vec<DecodedSlave>,
}

/// Emitted block design: SystemVerilog plus the structure it realized.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemEmit {
    pub top: String,
    pub sv: String,
    pub ports: Vec<TopPort>,
    pub interconnect: Option<InterconnectInfo>,
    /// Every HDL file the design needs (the emitted top excluded), deduped.
    pub sources: Vec<PathBuf>,
}

/// One block-design DRC finding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SysDiagnostic {
    pub error: bool,
    pub cell: String,
    pub message: String,
}

fn ident_ok(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !s.starts_with(|c: char| c.is_ascii_digit())
}

impl BdSystem {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            cells: Vec::new(),
        }
    }

    pub fn cell(&self, inst: &str) -> Option<&SysCell> {
        self.cells.iter().find(|c| c.inst == inst)
    }

    pub fn cell_mut(&mut self, inst: &str) -> Option<&mut SysCell> {
        self.cells.iter_mut().find(|c| c.inst == inst)
    }

    /// Next free `<core>_<n>` instance name.
    pub fn next_inst(&self, core: &str) -> String {
        (0..)
            .map(|n| format!("{core}_{n}"))
            .find(|n| self.cell(n).is_none())
            .unwrap_or_else(|| format!("{core}_x"))
    }

    /// Add a packaged core as a new, unconnected cell. Returns its instance.
    pub fn add_cell(&mut self, model: CoreModel) -> Result<String, String> {
        if !matches!(model.core.bus.as_str(), "Helion-MM" | "Helion-ST") {
            return Err(format!(
                "{}: bus {} has no block-design interface (Helion-MM/ST only)",
                model.core.name, model.core.bus
            ));
        }
        let inst = self.next_inst(&model.core.name);
        self.cells.push(SysCell {
            inst: inst.clone(),
            model,
            params: BTreeMap::new(),
            base: None,
            connected: false,
        });
        Ok(inst)
    }

    pub fn remove_cell(&mut self, inst: &str) -> Result<(), String> {
        let before = self.cells.len();
        self.cells.retain(|c| c.inst != inst);
        if self.cells.len() == before {
            return Err(format!("no cell {inst}"));
        }
        Ok(())
    }

    /// Set one `CONFIG.<name>` parameter (validated against the core model).
    pub fn set_param(&mut self, inst: &str, name: &str, value: &str) -> Result<String, String> {
        let cell = self.cell_mut(inst).ok_or_else(|| format!("no cell {inst}"))?;
        let p = cell
            .model
            .param(name)
            .ok_or_else(|| format!("{}: no parameter {name}", cell.model.core.name))?;
        let lit = p.validate(value)?;
        cell.params.insert(name.to_string(), value.trim().to_string());
        Ok(lit)
    }

    /// Set a slave's base address (4 KiB aligned).
    pub fn set_base(&mut self, inst: &str, base: u64) -> Result<(), String> {
        if base % MM_APERTURE != 0 {
            return Err(format!("{inst}: base {base:#x} is not 4 KiB aligned"));
        }
        if base > u32::MAX as u64 - (MM_APERTURE - 1) {
            return Err(format!("{inst}: base {base:#x} is outside the 32-bit space"));
        }
        let cell = self.cell_mut(inst).ok_or_else(|| format!("no cell {inst}"))?;
        if cell.role() != Some(MmRole::Slave) {
            return Err(format!("{inst}: not a Helion-MM slave"));
        }
        cell.base = Some(base);
        Ok(())
    }

    /// Rename a cell's HDL instance.
    pub fn rename(&mut self, inst: &str, to: &str) -> Result<(), String> {
        if !ident_ok(to) {
            return Err(format!("{to}: not a valid HDL identifier"));
        }
        if inst != to && self.cell(to).is_some() {
            return Err(format!("{to}: instance name already used"));
        }
        let cell = self.cell_mut(inst).ok_or_else(|| format!("no cell {inst}"))?;
        cell.inst = to.to_string();
        Ok(())
    }

    /// Give every slave without a base the next free 4 KiB aperture from
    /// `0x4000_0000`. Returns the `(inst, base)` pairs assigned.
    pub fn assign_addresses(&mut self) -> Vec<(String, u64)> {
        let mut used: BTreeSet<u64> = self.cells.iter().filter_map(|c| c.base).collect();
        let mut out = Vec::new();
        let mut next = MM_PERIPH_BASE;
        for c in &mut self.cells {
            if c.role() != Some(MmRole::Slave) || c.base.is_some() {
                continue;
            }
            while used.contains(&next) {
                next += MM_APERTURE;
            }
            c.base = Some(next);
            used.insert(next);
            out.push((c.inst.clone(), next));
        }
        out
    }

    fn masters(&self) -> Vec<&SysCell> {
        self.cells
            .iter()
            .filter(|c| c.role() == Some(MmRole::Master))
            .collect()
    }

    /// Preview connection automation (what [`Self::apply_automation`] does).
    pub fn plan_automation(&self) -> AutomationPlan {
        let mut plan = AutomationPlan::default();
        let mut sim = self.clone();
        let assigned: BTreeMap<String, u64> = sim.assign_addresses().into_iter().collect();
        let masters = self.masters();
        let master = masters
            .iter()
            .find(|c| c.connected)
            .or_else(|| masters.first())
            .map(|c| c.inst.clone());
        let has_slave = self.cells.iter().any(|c| c.role() == Some(MmRole::Slave));
        let ic = format!("{}_mm_interconnect", self.name);
        let mut need_ext_master = false;
        for c in &self.cells {
            if c.connected {
                continue;
            }
            if c.role() == Some(MmRole::Master) && master.as_deref() != Some(c.inst.as_str()) {
                plan.skipped.push((
                    c.inst.clone(),
                    format!(
                        "second Helion-MM master; the interconnect is single-master ({} drives it)",
                        master.clone().unwrap_or_default()
                    ),
                ));
                continue;
            }
            plan.cells.push(c.inst.clone());
            for (port, kind, src) in [("clk", "clock", "clk"), ("resetn", "reset", "resetn")] {
                if c.model.module.port(port).is_some() {
                    plan.steps.push(PlannedConnection {
                        cell: c.inst.clone(),
                        kind,
                        from: format!("/{src}"),
                        to: format!("{}/{port}", c.inst),
                    });
                }
            }
            match c.role() {
                Some(MmRole::Slave) => {
                    let base = c.base.or_else(|| assigned.get(&c.inst).copied()).unwrap_or(0);
                    plan.steps.push(PlannedConnection {
                        cell: c.inst.clone(),
                        kind: "bus",
                        from: format!("{ic}/M"),
                        to: format!("{}/s_mm", c.inst),
                    });
                    plan.steps.push(PlannedConnection {
                        cell: c.inst.clone(),
                        kind: "address",
                        from: format!("{base:#010x}"),
                        to: format!("{:#010x}", base + MM_APERTURE - 1),
                    });
                    if master.is_none() {
                        need_ext_master = true;
                    }
                }
                Some(MmRole::Master) if has_slave => plan.steps.push(PlannedConnection {
                    cell: c.inst.clone(),
                    kind: "bus",
                    from: format!("{}/m_mm", c.inst),
                    to: format!("{ic}/S"),
                }),
                _ => {}
            }
            for p in &c.model.module.ports {
                if p.name == "clk" || p.name == "resetn" {
                    continue;
                }
                if c.role().is_some() && is_mm_signal(&p.name) {
                    continue;
                }
                plan.steps.push(PlannedConnection {
                    cell: c.inst.clone(),
                    kind: "external",
                    from: format!("{}/{}", c.inst, p.name),
                    to: format!("/{}_{}", c.inst, p.name),
                });
            }
        }
        let ext_exists = master.is_none()
            && self
                .cells
                .iter()
                .any(|c| c.connected && c.role() == Some(MmRole::Slave));
        if need_ext_master && !ext_exists {
            plan.steps.push(PlannedConnection {
                cell: String::new(),
                kind: "external",
                from: "/s_mm".into(),
                to: format!("{ic}/S"),
            });
        }
        plan
    }

    /// Run connection automation: assign addresses and wire every cell the
    /// plan covers. Returns the plan it executed.
    pub fn apply_automation(&mut self) -> AutomationPlan {
        let plan = self.plan_automation();
        self.assign_addresses();
        for inst in &plan.cells {
            if let Some(c) = self.cell_mut(inst) {
                c.connected = true;
            }
        }
        plan
    }

    /// Block-design DRC: errors block emission; warnings are advisory.
    pub fn validate(&self) -> Vec<SysDiagnostic> {
        let mut out = Vec::new();
        let err = |cell: &str, m: String| SysDiagnostic {
            error: true,
            cell: cell.to_string(),
            message: m,
        };
        let warn = |cell: &str, m: String| SysDiagnostic {
            error: false,
            cell: cell.to_string(),
            message: m,
        };
        if !ident_ok(&self.name) {
            out.push(err("", format!("design name {} is not an HDL identifier", self.name)));
        }
        let mut seen = BTreeSet::new();
        for c in &self.cells {
            if !seen.insert(c.inst.clone()) {
                out.push(err(&c.inst, format!("duplicate instance {}", c.inst)));
            }
            if let Err(e) = c.model.validate_overrides(&c.params) {
                out.push(err(&c.inst, e));
            }
            if !c.connected {
                out.push(warn(
                    &c.inst,
                    format!("{} is unconnected; its pins are exported as top-level ports", c.inst),
                ));
            }
        }
        let connected_masters: Vec<&SysCell> =
            self.masters().into_iter().filter(|c| c.connected).collect();
        if connected_masters.len() > 1 {
            for c in &connected_masters[1..] {
                out.push(err(&c.inst, "more than one Helion-MM master on the interconnect".into()));
            }
        }
        let slaves: Vec<&SysCell> = self
            .cells
            .iter()
            .filter(|c| c.connected && c.role() == Some(MmRole::Slave))
            .collect();
        for (i, a) in slaves.iter().enumerate() {
            let Some(ab) = a.base else {
                out.push(err(&a.inst, format!("{} has no base address", a.inst)));
                continue;
            };
            if ab % MM_APERTURE != 0 {
                out.push(err(&a.inst, format!("{} base {ab:#x} not 4 KiB aligned", a.inst)));
            }
            for b in &slaves[i + 1..] {
                if b.base == Some(ab) {
                    out.push(err(
                        &b.inst,
                        format!("{} overlaps {} at {ab:#010x}", b.inst, a.inst),
                    ));
                }
            }
        }
        if let Some(m) = connected_masters.first() {
            let reset = m
                .params
                .get("PROGADDR_RESET")
                .cloned()
                .or_else(|| m.model.param("PROGADDR_RESET").map(|p| p.default.clone()))
                .and_then(|v| helion_ipxact::parse_param_number(&v));
            if let Some(rv) = reset {
                let rv = rv as u64;
                let mapped = slaves
                    .iter()
                    .any(|s| s.base.is_some_and(|b| rv >= b && rv < b + MM_APERTURE));
                if !mapped {
                    out.push(warn(
                        &m.inst,
                        format!(
                            "{} fetches its reset vector {rv:#010x} from an unmapped address (reads return 0)",
                            m.inst
                        ),
                    ));
                }
            }
        }
        out
    }

    /// Memory map rows of the connected slaves (base order).
    pub fn address_map(&self) -> Vec<DecodedSlave> {
        let mut v: Vec<DecodedSlave> = self
            .cells
            .iter()
            .filter(|c| c.connected && c.role() == Some(MmRole::Slave))
            .filter_map(|c| {
                Some(DecodedSlave {
                    inst: c.inst.clone(),
                    core: c.model.core.name.clone(),
                    base: c.base?,
                    range: MM_APERTURE,
                })
            })
            .collect();
        v.sort_by_key(|s| s.base);
        v
    }
}

fn range_decl(width: u32) -> String {
    if width > 1 {
        format!("[{}:0] ", width - 1)
    } else {
        String::new()
    }
}

fn emit_interconnect(module: &str, slaves: &[DecodedSlave]) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "// Helion-MM single-master interconnect: {} slave(s), 4 KiB apertures.\n\
         // Generated by helion-bd. An access outside every aperture completes\n\
         // immediately and reads 32'h0 (decode error), so a master never hangs.\n",
        slaves.len()
    ));
    s.push_str(&format!("module {module} (\n"));
    let mut ports = vec![
        "    input  wire        s_valid".to_string(),
        "    input  wire        s_write".into(),
        "    input  wire [31:0] s_addr".into(),
        "    input  wire [31:0] s_wdata".into(),
        "    input  wire [3:0]  s_wstrb".into(),
        "    output wire [31:0] s_rdata".into(),
        "    output wire        s_ready".into(),
    ];
    for i in 0..slaves.len() {
        ports.push(format!("    output wire        m{i}_valid"));
        ports.push(format!("    output wire        m{i}_write"));
        ports.push(format!("    output wire [31:0] m{i}_addr"));
        ports.push(format!("    output wire [31:0] m{i}_wdata"));
        ports.push(format!("    output wire [3:0]  m{i}_wstrb"));
        ports.push(format!("    input  wire [31:0] m{i}_rdata"));
        ports.push(format!("    input  wire        m{i}_ready"));
    }
    s.push_str(&ports.join(",\n"));
    s.push_str("\n);\n");
    let shift = MM_APERTURE.trailing_zeros();
    for (i, sl) in slaves.iter().enumerate() {
        let w = 32 - shift;
        s.push_str(&format!(
            "    // {} @ {:#010x}..{:#010x}\n    wire sel{i} = (s_addr[31:{shift}] == {w}'h{:X});\n",
            sl.inst,
            sl.base,
            sl.base + sl.range - 1,
            sl.base >> shift
        ));
        s.push_str(&format!(
            "    assign m{i}_valid = s_valid & sel{i};\n    assign m{i}_write = s_write;\n    assign m{i}_addr  = s_addr;\n    assign m{i}_wdata = s_wdata;\n    assign m{i}_wstrb = s_wstrb;\n"
        ));
    }
    let mut rdata = "32'h0".to_string();
    let mut ready = "s_valid".to_string();
    for i in (0..slaves.len()).rev() {
        rdata = format!("sel{i} ? m{i}_rdata : {rdata}");
        ready = format!("sel{i} ? m{i}_ready : {ready}");
    }
    s.push_str(&format!("    assign s_rdata = {rdata};\n    assign s_ready = {ready};\nendmodule\n"));
    s
}

/// Emit the block design: top module `<name>`, plus `<name>_mm_interconnect`
/// when connected Helion-MM slaves exist. Fails on any DRC error.
pub fn emit_system(sys: &BdSystem) -> Result<SystemEmit, String> {
    let errors: Vec<String> = sys
        .validate()
        .into_iter()
        .filter(|d| d.error)
        .map(|d| d.message)
        .collect();
    if !errors.is_empty() {
        return Err(errors.join("; "));
    }
    let mut ports: Vec<TopPort> = Vec::new();
    let mut body = String::new();
    let mut wires = String::new();
    let any_connected = sys.cells.iter().any(|c| c.connected);
    if any_connected {
        for p in ["clk", "resetn"] {
            ports.push(TopPort {
                name: p.into(),
                dir: PortDirection::In,
                width: 1,
                cell: None,
            });
        }
    }
    let slaves = sys.address_map();
    let master = sys
        .cells
        .iter()
        .find(|c| c.connected && c.role() == Some(MmRole::Master));
    let ic_module = format!("{}_mm_interconnect", sys.name);
    let interconnect = if slaves.is_empty() {
        None
    } else {
        Some(InterconnectInfo {
            module: ic_module.clone(),
            inst: "mm_interconnect".into(),
            master: master.map(|m| m.inst.clone()),
            slaves: slaves.clone(),
        })
    };
    if interconnect.is_some() && master.is_none() {
        for (n, d, w) in [
            ("s_mm_valid", PortDirection::In, 1),
            ("s_mm_write", PortDirection::In, 1),
            ("s_mm_addr", PortDirection::In, 32),
            ("s_mm_wdata", PortDirection::In, 32),
            ("s_mm_wstrb", PortDirection::In, 4),
            ("s_mm_rdata", PortDirection::Out, 32),
            ("s_mm_ready", PortDirection::Out, 1),
        ] {
            ports.push(TopPort {
                name: n.into(),
                dir: d,
                width: w,
                cell: None,
            });
        }
    }
    let slave_index: BTreeMap<&str, usize> = slaves
        .iter()
        .enumerate()
        .map(|(i, s)| (s.inst.as_str(), i))
        .collect();
    for c in &sys.cells {
        let overrides = c.model.validate_overrides(&c.params)?;
        let params = if overrides.is_empty() {
            String::new()
        } else {
            format!(
                " #(\n{}\n    )",
                overrides
                    .iter()
                    .map(|(k, v)| format!("        .{k}({v})"))
                    .collect::<Vec<_>>()
                    .join(",\n")
            )
        };
        let mut conns = Vec::new();
        for p in &c.model.module.ports {
            let ext = |ports: &mut Vec<TopPort>| {
                let name = format!("{}_{}", c.inst, p.name);
                ports.push(TopPort {
                    name: name.clone(),
                    dir: p.dir,
                    width: p.width,
                    cell: Some((c.inst.clone(), p.name.clone())),
                });
                name
            };
            let net = if !c.connected {
                ext(&mut ports)
            } else if p.name == "clk" || p.name == "resetn" {
                p.name.clone()
            } else if is_mm_signal(&p.name) && c.role() == Some(MmRole::Slave) {
                let i = slave_index[c.inst.as_str()];
                let sig = p.name.trim_start_matches("mm_");
                let full = if sig == "wstrb" { 4 } else if matches!(sig, "valid" | "write" | "ready") { 1 } else { 32 };
                if p.width < full {
                    format!("ic_m{i}_{sig}[{}:0]", p.width - 1)
                } else {
                    format!("ic_m{i}_{sig}")
                }
            } else if is_mm_signal(&p.name) && c.role() == Some(MmRole::Master) {
                format!("ic_s_{}", p.name.trim_start_matches("mm_"))
            } else {
                ext(&mut ports)
            };
            conns.push(format!("        .{}({net})", p.name));
        }
        body.push_str(&format!(
            "    {}{params} {} (\n{}\n    );\n\n",
            c.model.module.name,
            c.inst,
            conns.join(",\n")
        ));
    }
    if let Some(ic) = &interconnect {
        let sigs: [(&str, u32); 7] = [
            ("valid", 1),
            ("write", 1),
            ("addr", 32),
            ("wdata", 32),
            ("wstrb", 4),
            ("rdata", 32),
            ("ready", 1),
        ];
        if master.is_some() {
            for (sig, w) in sigs {
                wires.push_str(&format!("    wire {}ic_s_{sig};\n", range_decl(w)));
            }
            // A master without mm_write/mm_wstrb still needs driven inputs.
            let m = master.expect("master");
            for (sig, tie) in [("write", "1'b0"), ("wstrb", "4'hF")] {
                if m.model.module.port(&format!("mm_{sig}")).is_none() {
                    wires.push_str(&format!("    assign ic_s_{sig} = {tie};\n"));
                }
            }
        } else {
            for (sig, _) in sigs {
                wires.push_str(&format!("    wire ic_s_{sig}_unused;\n"));
            }
        }
        for i in 0..ic.slaves.len() {
            for (sig, w) in sigs {
                wires.push_str(&format!("    wire {}ic_m{i}_{sig};\n", range_decl(w)));
            }
        }
        let src = |sig: &str| {
            if master.is_some() {
                format!("ic_s_{sig}")
            } else {
                format!("s_mm_{sig}")
            }
        };
        let mut conns = Vec::new();
        for (sig, _) in sigs {
            conns.push(format!("        .s_{sig}({})", src(sig)));
        }
        for i in 0..ic.slaves.len() {
            for (sig, _) in sigs {
                conns.push(format!("        .m{i}_{sig}(ic_m{i}_{sig})"));
            }
        }
        // Slaves without mm_ready / mm_write must still drive the decoder.
        for (i, sl) in ic.slaves.iter().enumerate() {
            let cell = sys.cell(&sl.inst).expect("slave cell");
            if cell.model.module.port("mm_ready").is_none() {
                wires.push_str(&format!("    assign ic_m{i}_ready = ic_m{i}_valid;\n"));
            }
        }
        body.push_str(&format!(
            "    {} {} (\n{}\n    );\n",
            ic.module,
            ic.inst,
            conns.join(",\n")
        ));
    }
    let mut sv = String::new();
    sv.push_str(&format!(
        "// Block design {} — generated by helion-bd from packaged Helion IP.\n",
        sys.name
    ));
    let mut decl = Vec::new();
    for p in &ports {
        decl.push(format!(
            "    {} wire {}{}",
            p.dir.sv_keyword(),
            range_decl(p.width),
            p.name
        ));
    }
    sv.push_str(&format!("module {} (\n{}\n);\n", sys.name, decl.join(",\n")));
    sv.push_str(&wires);
    if !wires.is_empty() {
        sv.push('\n');
    }
    sv.push_str(&body);
    sv.push_str("endmodule\n");
    if let Some(ic) = &interconnect {
        sv.push('\n');
        sv.push_str(&emit_interconnect(&ic.module, &ic.slaves));
    }
    let mut sources = Vec::new();
    for c in &sys.cells {
        for f in &c.model.files {
            if !sources.contains(f) {
                sources.push(f.clone());
            }
        }
    }
    Ok(SystemEmit {
        top: sys.name.clone(),
        sv,
        ports,
        interconnect,
        sources,
    })
}

impl BdSystem {
    /// Build a fully automated system from a legacy core list, naming cells
    /// `u_<core>_<idx>` exactly like [`crate::build_memory_map`] and
    /// assigning the same sequential apertures. `None` when a core has no
    /// RTL package (generator-only cores).
    pub fn from_block_design(bd: &crate::BlockDesign) -> Option<Self> {
        let mut sys = BdSystem::new(&bd.name);
        for (idx, c) in bd.cores.iter().enumerate() {
            let model = helion_ipxact::load_core_model(&c.name).ok()?;
            sys.cells.push(SysCell {
                inst: format!("u_{}_{idx}", c.name),
                model,
                params: BTreeMap::new(),
                base: None,
                connected: true,
            });
        }
        sys.assign_addresses();
        Some(sys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(name: &str) -> CoreModel {
        helion_ipxact::load_core_model(name).expect(name)
    }

    #[test]
    fn plan_previews_and_apply_wires_a_processor_system() {
        let mut sys = BdSystem::new("soc");
        let cpu = sys.add_cell(model("h_rv32_hb1")).unwrap();
        let uart = sys.add_cell(model("h_uart")).unwrap();
        let gpio = sys.add_cell(model("h_gpio")).unwrap();
        assert_eq!((cpu.as_str(), uart.as_str()), ("h_rv32_hb1_0", "h_uart_0"));
        let before = sys.clone();
        let plan = sys.plan_automation();
        assert_eq!(sys, before, "planning never mutates");
        assert_eq!(plan.cells, vec![cpu.clone(), uart.clone(), gpio.clone()]);
        assert!(plan.steps.iter().any(|s| s.kind == "bus" && s.from == "h_rv32_hb1_0/m_mm"));
        assert!(plan.steps.iter().any(|s| s.kind == "address" && s.cell == gpio && s.from == "0x40001000"));
        assert!(plan.steps.iter().any(|s| s.kind == "external" && s.to == "/h_uart_0_tx"));
        let applied = sys.apply_automation();
        assert_eq!(applied, plan, "apply executes the previewed plan");
        assert!(sys.cells.iter().all(|c| c.connected));
        assert!(sys.plan_automation().cells.is_empty(), "nothing left to wire");

        sys.set_param(&cpu, "ENABLE_MUL", "1").unwrap();
        assert!(sys.set_param(&cpu, "ENABLE_MUL", "7").is_err());
        let out = emit_system(&sys).unwrap();
        let ic = out.interconnect.clone().expect("interconnect");
        assert_eq!(ic.master.as_deref(), Some("h_rv32_hb1_0"));
        assert_eq!(ic.slaves.len(), 2);
        assert!(out.sv.contains("module soc_mm_interconnect"));
        assert!(out.sv.contains("soc_mm_interconnect mm_interconnect ("));
        assert!(out.sv.contains(".ENABLE_MUL(1)"));
        assert!(out.sv.contains("s_addr[31:12] == 20'h40000"));
        assert!(out.sv.contains(".mm_addr(ic_m0_addr[7:0])"));
        assert!(out.ports.iter().any(|p| p.name == "h_gpio_0_gpio_out" && p.width == 8));
        assert!(out.sources.iter().any(|p| p.ends_with("picorv32.v")));
        let warns = sys.validate();
        assert!(
            warns.iter().any(|d| !d.error && d.message.contains("reset vector")),
            "{warns:?}"
        );
    }

    #[test]
    fn emitted_system_synthesizes_with_the_core_rtl() {
        let mut sys = BdSystem::new("bd_synth");
        sys.add_cell(model("h_uart")).unwrap();
        sys.add_cell(model("h_gpio")).unwrap();
        sys.add_cell(model("h_sync_fifo")).unwrap();
        sys.apply_automation();
        let out = emit_system(&sys).unwrap();
        assert!(out.interconnect.as_ref().unwrap().master.is_none());
        assert!(out.ports.iter().any(|p| p.name == "s_mm_addr"), "external master port");
        let mut files: Vec<(String, String)> = out
            .sources
            .iter()
            .map(|p| (p.display().to_string(), std::fs::read_to_string(p).unwrap()))
            .collect();
        files.push(("bd_synth.sv".into(), out.sv.clone()));
        let refs: Vec<(&str, &str)> = files.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let d = helion_sv::synth_sv_sources(&refs).unwrap_or_else(|e| panic!("{e}\n{}", out.sv));
        assert!(!d.cells.is_empty());
        assert!(d.ports.len() >= out.ports.len(), "every top port elaborates");
    }

    #[test]
    fn unconnected_cells_export_pins_and_drc_catches_overlap() {
        let mut sys = BdSystem::new("bd");
        let a = sys.add_cell(model("h_uart")).unwrap();
        let b = sys.add_cell(model("h_timer")).unwrap();
        let out = emit_system(&sys).unwrap();
        assert!(out.interconnect.is_none(), "no interconnect before automation");
        assert!(out.ports.iter().any(|p| p.name == "h_uart_0_mm_addr"));
        sys.apply_automation();
        sys.set_base(&a, 0x4000_0000).unwrap();
        sys.set_base(&b, 0x4000_0000).unwrap();
        assert!(emit_system(&sys).unwrap_err().contains("overlaps"));
        assert!(sys.set_base(&b, 0x4000_0800).is_err(), "alignment");
        sys.set_base(&b, 0x4001_0000).unwrap();
        assert!(emit_system(&sys).is_ok());
        assert!(sys.rename(&a, "uart").is_ok());
        assert!(sys.rename("uart", "9bad").is_err());
    }

    #[test]
    fn second_master_is_skipped_with_a_reason() {
        let mut sys = BdSystem::new("mm2");
        sys.add_cell(model("h_rv32_hb1")).unwrap();
        sys.add_cell(model("h_rv32_hb1")).unwrap();
        sys.add_cell(model("h_uart")).unwrap();
        let plan = sys.plan_automation();
        assert_eq!(plan.skipped.len(), 1);
        assert!(plan.skipped[0].1.contains("single-master"));
    }
}

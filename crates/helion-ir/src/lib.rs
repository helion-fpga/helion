//! Helion netlist IR (HNF). Structural cells, attributes, hierarchy, round-trip.

use std::collections::{BTreeMap, HashMap};

#[derive(Clone, Debug, Default)]
pub struct Attrs {
    pub map: BTreeMap<String, String>,
}

impl Attrs {
    pub fn set(&mut self, k: impl Into<String>, v: impl Into<String>) {
        self.map.insert(k.into(), v.into());
    }
    pub fn get(&self, k: &str) -> Option<&str> {
        self.map.get(k).map(|s| s.as_str())
    }
    pub fn flag(&self, k: &str) -> bool {
        match self.get(k) {
            Some("1" | "true" | "TRUE" | "yes") => true,
            _ => false,
        }
    }
}


/// Source location for a SOFT diagnostic (FM-HEL-L3). Prefer file+line; columns optional.
/// Elaborators (SV/VHDL) fill what they know; unknown fields stay `None`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SoftSpan {
    pub file: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub end_line: Option<u32>,
}

impl SoftSpan {
    pub fn file_line(file: impl Into<String>, line: u32) -> Self {
        Self {
            file: Some(file.into()),
            line: Some(line),
            column: None,
            end_line: None,
        }
    }
}

/// First-class incomplete-map entry. **SOFT ≠ PASS** — never invent Helion cells for these.
///
/// `name` is the stable diagnostic id (same strings SV already prints), e.g.
/// `assign_not_lowered`, `generate_not_lowered`, `child_soft_incomplete`, `wide_cone`,
/// `function_not_called`, `sequential_not_lowered`, …
///
/// `module` is hierarchical context. `detail` is optional signal / function / child /
/// primitive / path (whatever the diagnostic already carries).
///
/// `children` nests child softs (e.g. `child_soft_incomplete` under a parent wrap).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SoftDiag {
    pub name: String,
    pub module: String,
    pub detail: Option<String>,
    pub span: SoftSpan,
    pub children: Vec<SoftDiag>,
}

impl SoftDiag {
    pub fn new(name: impl Into<String>, module: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            module: module.into(),
            detail: None,
            span: SoftSpan::default(),
            children: Vec::new(),
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn with_span(mut self, span: SoftSpan) -> Self {
        self.span = span;
        self
    }

    pub fn push_child(&mut self, child: SoftDiag) {
        self.children.push(child);
    }

    /// One-line table row (headless / Reports). Does not claim PASS.
    pub fn table_line(&self) -> String {
        let mut s = format!("soft name={} module={}", self.name, self.module);
        if let Some(d) = &self.detail {
            s.push_str(&format!(" detail={d}"));
        }
        if let (Some(f), Some(l)) = (&self.span.file, self.span.line) {
            s.push_str(&format!(" span={f}:{l}"));
            if let Some(c) = self.span.column {
                s.push_str(&format!(":{c}"));
            }
        } else if let Some(l) = self.span.line {
            s.push_str(&format!(" span=:{l}"));
        }
        if !self.children.is_empty() {
            s.push_str(&format!(" children={}", self.children.len()));
        }
        s
    }
}

/// Post-map elaborator result: Helion `Design` cells + first-class SOFT table.
/// PASS / closed-WNS paths must use `design` alone only when `softs` is empty
/// for the cones under test — soft cones never count as PASS.
#[derive(Clone, Debug)]
pub struct MapResult {
    pub design: Design,
    pub softs: Vec<SoftDiag>,
}

impl MapResult {
    pub fn from_design(design: Design) -> Self {
        Self {
            design,
            softs: Vec::new(),
        }
    }

    pub fn soft_table_lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        fn walk(s: &SoftDiag, out: &mut Vec<String>) {
            out.push(s.table_line());
            for c in &s.children {
                walk(c, out);
            }
        }
        for s in &self.softs {
            walk(s, &mut out);
        }
        out
    }

    /// True when any soft is present (including nested children).
    pub fn has_softs(&self) -> bool {
        !self.softs.is_empty()
    }
}

#[derive(Clone, Debug)]
pub struct Design {
    pub name: String,
    pub ports: Vec<Port>,
    pub cells: Vec<Cell>,
    pub nets: Vec<Net>,
    pub instances: Vec<Instance>,
    pub attrs: Attrs,
    /// Name → index in `nets`. `connect` / `merge_net` keep this in sync so
    /// Ibex-scale stitch is O(1) per net, not a linear scan of every net.
    net_ix: HashMap<String, usize>,
    /// cell → pin → net index. Makes `net_on` O(1) instead of scanning every net.
    pin_ix: HashMap<String, HashMap<String, usize>>,
    /// Name → index in `cells`.
    cell_ix: HashMap<String, usize>,
}

#[derive(Clone, Debug)]
pub struct Port {
    pub name: String,
    pub dir: PortDir,
    pub attrs: Attrs,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PortDir {
    In,
    Out,
    Inout,
}

#[derive(Clone, Debug)]
pub struct Cell {
    pub name: String,
    pub kind: CellKind,
    pub attrs: Attrs,
}

#[derive(Clone, Debug)]
pub enum CellKind {
    /// Logical LUT6. `init[addr]` is the output for inputs `{I5..I0}` as addr.
    Lut6 { init: u64 },
    /// Logical FF (HELIONLIB HFF).
    Hff,
    /// Output IOB driving a top port.
    IobOut,
    /// DSP MAC27 (pre-add * mul + acc). Site primitive.
    Mac27,
    /// Inserted ILA core capturing `net`.
    Ila { net: String },
    /// Block RAM 18Kb (true dual-port primitive).
    Bram18,
    /// Hierarchical black box (unelaborated IP / DFX partition).
    BlackBox { module: String },
}

#[derive(Clone, Debug)]
pub struct Net {
    pub name: String,
    pub endpoints: Vec<Endpoint>,
    pub attrs: Attrs,
}

#[derive(Clone, Debug)]
pub struct Endpoint {
    pub cell: String,
    pub pin: String,
}

#[derive(Clone, Debug)]
pub struct Instance {
    pub name: String,
    pub module: String,
    pub conns: Vec<(String, String)>,
    pub attrs: Attrs,
}

/// Gold INIT words for a 4-bit incrementer with I0=q0 .. Ii=qi (I0 = LSB of addr).
/// bit0: ~I0; bit1: I1^I0; bit2: I2^(I1&I0); bit3: I3^(I2&I1&I0).
pub const INC4_INIT: [u64; 4] = [
    0x5555_5555_5555_5555,
    0x6666_6666_6666_6666,
    0x7878_7878_7878_7878,
    0x7F80_7F80_7F80_7F80,
];


/// Fast (cell, pin) → net lookup built by [`Design::pin_index`].
pub struct PinIndex<'a> {
    pin_to_net: HashMap<(&'a str, &'a str), &'a str>,
}

impl<'a> PinIndex<'a> {
    pub fn net_on(&self, cell: &str, pin: &str) -> Option<&'a str> {
        self.pin_to_net.get(&(cell, pin)).copied()
    }
}

impl Design {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ports: Vec::new(),
            cells: Vec::new(),
            nets: Vec::new(),
            instances: Vec::new(),
            attrs: Attrs::default(),
            net_ix: HashMap::new(),
            pin_ix: HashMap::new(),
            cell_ix: HashMap::new(),
        }
    }

    fn rebuild_net_ix(&mut self) {
        self.net_ix.clear();
        self.pin_ix.clear();
        self.net_ix.reserve(self.nets.len());
        for (i, n) in self.nets.iter().enumerate() {
            self.net_ix.insert(n.name.clone(), i);
            for e in &n.endpoints {
                self.pin_ix
                    .entry(e.cell.clone())
                    .or_default()
                    .insert(e.pin.clone(), i);
            }
        }
    }

    fn rebuild_cell_ix(&mut self) {
        self.cell_ix.clear();
        self.cell_ix.reserve(self.cells.len());
        for (i, c) in self.cells.iter().enumerate() {
            self.cell_ix.insert(c.name.clone(), i);
        }
    }

    /// Rebuild name maps after bulk cell retain/rename. `connect` keeps them in sync.
    pub fn rebuild_indexes(&mut self) {
        self.rebuild_net_ix();
        self.rebuild_cell_ix();
    }

    /// Clone ports/cells/nets without the O(n) name indexes.
    /// `push_cell` / `merge_net` / `connect` rebuild indexes on the next mutate.
    /// Used by helion-sv OwnCache so a cache hit does not copy `net_ix`/`pin_ix`/`cell_ix`.
    pub fn clone_data(&self) -> Self {
        Self {
            name: self.name.clone(),
            ports: self.ports.clone(),
            cells: self.cells.clone(),
            nets: self.nets.clone(),
            instances: self.instances.clone(),
            attrs: self.attrs.clone(),
            net_ix: HashMap::new(),
            pin_ix: HashMap::new(),
            cell_ix: HashMap::new(),
        }
    }

    fn ensure_net_ix(&mut self) {
        if self.net_ix.len() != self.nets.len() {
            self.rebuild_net_ix();
        }
    }

    fn ensure_cell_ix(&mut self) {
        if self.cell_ix.len() != self.cells.len() {
            self.rebuild_cell_ix();
        }
    }

    fn index_pin(&mut self, cell: &str, pin: &str, net_i: usize) {
        self.pin_ix
            .entry(cell.to_string())
            .or_default()
            .insert(pin.to_string(), net_i);
    }

    /// Merge `net` into this design by name (O(1) index). Used by hierarchy stitch.
    pub fn merge_net(&mut self, net: Net) {
        self.ensure_net_ix();
        if let Some(&i) = self.net_ix.get(&net.name) {
            for e in &net.endpoints {
                self.index_pin(&e.cell, &e.pin, i);
            }
            self.nets[i].endpoints.extend(net.endpoints);
            return;
        }
        let i = self.nets.len();
        self.net_ix.insert(net.name.clone(), i);
        for e in &net.endpoints {
            self.index_pin(&e.cell, &e.pin, i);
        }
        self.nets.push(net);
    }

    pub fn add_port(&mut self, name: impl Into<String>, dir: PortDir) {
        self.ports.push(Port {
            name: name.into(),
            dir,
            attrs: Attrs::default(),
        });
    }

    pub fn add_cell(&mut self, name: impl Into<String>, kind: CellKind) {
        self.push_cell(Cell {
            name: name.into(),
            kind,
            attrs: Attrs::default(),
        });
    }

    /// Append a fully-built cell and keep `cell_ix` in sync (hierarchy stitch).
    pub fn push_cell(&mut self, cell: Cell) {
        self.ensure_cell_ix();
        self.cell_ix.insert(cell.name.clone(), self.cells.len());
        self.cells.push(cell);
    }

    /// Move `cells` into this design, updating `cell_ix` in O(new) not O(all).
    pub fn append_cells(&mut self, cells: &mut Vec<Cell>) {
        self.ensure_cell_ix();
        self.cell_ix.reserve(cells.len());
        self.cells.reserve(cells.len());
        for c in cells.drain(..) {
            self.cell_ix.insert(c.name.clone(), self.cells.len());
            self.cells.push(c);
        }
    }

    pub fn add_instance(&mut self, name: impl Into<String>, module: impl Into<String>) {
        self.instances.push(Instance {
            name: name.into(),
            module: module.into(),
            conns: Vec::new(),
            attrs: Attrs::default(),
        });
    }

    pub fn connect(&mut self, net: impl Into<String>, cell: impl Into<String>, pin: impl Into<String>) {
        let net_name = net.into();
        self.ensure_net_ix();
        let cell = cell.into();
        let pin = pin.into();
        if let Some(&i) = self.net_ix.get(&net_name) {
            self.index_pin(&cell, &pin, i);
            self.nets[i].endpoints.push(Endpoint { cell, pin });
            return;
        }
        let i = self.nets.len();
        self.net_ix.insert(net_name.clone(), i);
        self.index_pin(&cell, &pin, i);
        self.nets.push(Net {
            name: net_name,
            endpoints: vec![Endpoint { cell, pin }],
            attrs: Attrs::default(),
        });
    }

    pub fn net_on(&self, cell: &str, pin: &str) -> Option<&str> {
        if let Some(&i) = self.pin_ix.get(cell).and_then(|m| m.get(pin)) {
            return self.nets.get(i).map(|n| n.name.as_str());
        }
        self.nets.iter().find_map(|n| {
            n.endpoints
                .iter()
                .any(|e| e.cell == cell && e.pin == pin)
                .then_some(n.name.as_str())
        })
    }

    /// O(|endpoints|) index for repeated `net_on` (Ibex-scale pack).
    pub fn pin_index(&self) -> PinIndex<'_> {
        let mut pin_to_net = HashMap::with_capacity(self.nets.iter().map(|n| n.endpoints.len()).sum());
        for n in &self.nets {
            for e in &n.endpoints {
                pin_to_net.insert((e.cell.as_str(), e.pin.as_str()), n.name.as_str());
            }
        }
        PinIndex { pin_to_net }
    }

    pub fn cell(&self, name: &str) -> Option<&Cell> {
        if let Some(&i) = self.cell_ix.get(name) {
            return self.cells.get(i).filter(|c| c.name == name);
        }
        self.cells.iter().find(|c| c.name == name)
    }

    pub fn cell_mut(&mut self, name: &str) -> Option<&mut Cell> {
        self.ensure_cell_ix();
        let i = *self.cell_ix.get(name)?;
        self.cells.get_mut(i)
    }

    pub fn net(&self, name: &str) -> Option<&Net> {
        if let Some(&i) = self.net_ix.get(name) {
            return self.nets.get(i).filter(|n| n.name == name);
        }
        self.nets.iter().find(|n| n.name == name)
    }

    pub fn net_mut(&mut self, name: &str) -> Option<&mut Net> {
        self.ensure_net_ix();
        let i = *self.net_ix.get(name)?;
        self.nets.get_mut(i)
    }

    pub fn port_mut(&mut self, name: &str) -> Option<&mut Port> {
        self.ports.iter_mut().find(|p| p.name == name)
    }

    pub fn set_cell_attr(&mut self, cell: &str, k: &str, v: impl Into<String>) -> Result<(), String> {
        self.cell_mut(cell)
            .ok_or_else(|| format!("no cell {cell}"))?
            .attrs
            .set(k, v);
        Ok(())
    }

    pub fn set_net_attr(&mut self, net: &str, k: &str, v: impl Into<String>) -> Result<(), String> {
        self.net_mut(net)
            .ok_or_else(|| format!("no net {net}"))?
            .attrs
            .set(k, v);
        Ok(())
    }

    pub fn set_port_attr(&mut self, port: &str, k: &str, v: impl Into<String>) -> Result<(), String> {
        self.port_mut(port)
            .ok_or_else(|| format!("no port {port}"))?
            .attrs
            .set(k, v);
        Ok(())
    }

    pub fn mark_debug(&mut self, net: &str) -> Result<(), String> {
        self.set_net_attr(net, "mark_debug", "true")
    }

    pub fn dont_touch(&mut self, cell: &str) -> Result<(), String> {
        self.set_cell_attr(cell, "DONT_TOUCH", "true")
    }

    pub fn set_loc(&mut self, port: &str, site: &str) -> Result<(), String> {
        self.set_port_attr(port, "LOC", site)
    }

    pub fn set_iostandard(&mut self, port: &str, std: &str) -> Result<(), String> {
        self.set_port_attr(port, "IOSTANDARD", std)
    }

    pub fn set_drive(&mut self, port: &str, ma: &str) -> Result<(), String> {
        self.set_port_attr(port, "DRIVE", ma)
    }

    pub fn set_slew(&mut self, port: &str, slew: &str) -> Result<(), String> {
        self.set_port_attr(port, "SLEW", slew)
    }

    pub fn set_pulltype(&mut self, port: &str, pull: &str) -> Result<(), String> {
        self.set_port_attr(port, "PULLTYPE", pull)
    }

    pub fn set_diff_term(&mut self, port: &str, term: &str) -> Result<(), String> {
        self.set_port_attr(port, "DIFF_TERM", term)
    }

    pub fn set_in_term(&mut self, port: &str, term: &str) -> Result<(), String> {
        self.set_port_attr(port, "IN_TERM", term)
    }

    pub fn lut_inits(&self) -> Vec<u64> {
        self.cells
            .iter()
            .filter_map(|c| match c.kind {
                CellKind::Lut6 { init } => Some(init),
                _ => None,
            })
            .collect()
    }

    pub fn marked_debug_nets(&self) -> Vec<String> {
        self.nets
            .iter()
            .filter(|n| n.attrs.flag("mark_debug"))
            .map(|n| n.name.clone())
            .collect()
    }

    /// Prefix all cell/net names (hierarchy flatten).
    pub fn prefix(&mut self, pfx: &str) {
        for c in &mut self.cells {
            c.name = format!("{pfx}{}", c.name);
        }
        for n in &mut self.nets {
            n.name = format!("{pfx}{}", n.name);
            for e in &mut n.endpoints {
                e.cell = format!("{pfx}{}", e.cell);
            }
        }
        for i in &mut self.instances {
            i.name = format!("{pfx}{}", i.name);
        }
        self.rebuild_indexes();
    }

    /// Inline `child` as instance `inst`. Port `conns` map child-port → parent-net.
    pub fn instantiate(&mut self, inst: &str, mut child: Design, conns: &[(String, String)]) {
        let pfx = format!("{inst}_");
        child.prefix(&pfx);
        for (cport, pnet) in conns {
            let cname = format!("{pfx}{cport}");
            for n in &mut child.nets {
                if n.name == cname {
                    n.name = pnet.clone();
                }
            }
        }
        self.cells.append(&mut child.cells);
        for n in child.nets {
            self.merge_net(n);
        }
        self.rebuild_cell_ix();
        self.instances.push(Instance {
            name: inst.into(),
            module: child.name,
            conns: conns.to_vec(),
            attrs: Attrs::default(),
        });
    }

    /// Structural inverter-FF blinky: LED toggles each user clock.
    /// LUT INIT = 0x5555… so O = ~I0; I0 driven by Q.
    pub fn structural_blinky() -> Self {
        let mut d = Design::new("blinky");
        d.add_port("clk", PortDir::In);
        d.add_port("led", PortDir::Out);
        d.add_cell(
            "u_lut",
            CellKind::Lut6 {
                init: 0x5555_5555_5555_5555,
            },
        );
        d.add_cell("u_ff", CellKind::Hff);
        d.add_cell("u_iob", CellKind::IobOut);
        d.connect("clk", "u_ff", "CLK");
        d.connect("d", "u_lut", "O");
        d.connect("d", "u_ff", "D");
        d.connect("q", "u_ff", "Q");
        d.connect("q", "u_lut", "I0");
        d.connect("q", "u_iob", "I");
        d.connect("led", "u_iob", "PAD");
        d
    }

    /// 4-bit incrementer, LED = cnt[3]. Gold for synth + fabric.
    pub fn structural_counter() -> Self {
        let mut d = Design::new("counter");
        d.add_port("clk", PortDir::In);
        d.add_port("led", PortDir::Out);
        for i in 0..4 {
            d.add_cell(
                format!("u_lut{i}"),
                CellKind::Lut6 {
                    init: INC4_INIT[i],
                },
            );
            d.add_cell(format!("u_ff{i}"), CellKind::Hff);
            d.connect("clk", format!("u_ff{i}"), "CLK");
            d.connect(format!("d{i}"), format!("u_lut{i}"), "O");
            d.connect(format!("d{i}"), format!("u_ff{i}"), "D");
            d.connect(format!("q{i}"), format!("u_ff{i}"), "Q");
            for pin in 0..=i {
                d.connect(format!("q{pin}"), format!("u_lut{i}"), format!("I{pin}"));
            }
        }
        d.add_cell("u_iob", CellKind::IobOut);
        d.connect("q3", "u_iob", "I");
        d.connect("led", "u_iob", "PAD");
        d
    }

    /// Helion Netlist Format text (round-trip with [`from_hnf`]).
    pub fn to_hnf(&self) -> String {
        let mut s = format!("HNF 1\ndesign {}\n", self.name);
        for (k, v) in &self.attrs.map {
            s.push_str(&format!("dattr {k} {v}\n"));
        }
        for p in &self.ports {
            let d = match p.dir {
                PortDir::In => "in",
                PortDir::Out => "out",
                PortDir::Inout => "inout",
            };
            s.push_str(&format!("port {} {d}\n", p.name));
            for (k, v) in &p.attrs.map {
                s.push_str(&format!("pattr {} {k} {v}\n", p.name));
            }
        }
        for c in &self.cells {
            match &c.kind {
                CellKind::Lut6 { init } => s.push_str(&format!("cell {} Lut6 {init:#x}\n", c.name)),
                CellKind::Hff => s.push_str(&format!("cell {} Hff\n", c.name)),
                CellKind::IobOut => s.push_str(&format!("cell {} IobOut\n", c.name)),
                CellKind::Mac27 => s.push_str(&format!("cell {} Mac27\n", c.name)),
                CellKind::Ila { net } => s.push_str(&format!("cell {} Ila {net}\n", c.name)),
                CellKind::Bram18 => s.push_str(&format!("cell {} Bram18\n", c.name)),
                CellKind::BlackBox { module } => {
                    s.push_str(&format!("cell {} BlackBox {module}\n", c.name))
                }
            }
            for (k, v) in &c.attrs.map {
                s.push_str(&format!("cattr {} {k} {v}\n", c.name));
            }
        }
        for n in &self.nets {
            let eps: String = n
                .endpoints
                .iter()
                .map(|e| format!("{}/{}", e.cell, e.pin))
                .collect::<Vec<_>>()
                .join(" ");
            s.push_str(&format!("net {} {eps}\n", n.name));
            for (k, v) in &n.attrs.map {
                s.push_str(&format!("nattr {} {k} {v}\n", n.name));
            }
        }
        for i in &self.instances {
            let c: String = i
                .conns
                .iter()
                .map(|(p, n)| format!("{p}:{n}"))
                .collect::<Vec<_>>()
                .join(",");
            s.push_str(&format!("inst {} {} {c}\n", i.name, i.module));
        }
        s
    }

    pub fn from_hnf(text: &str) -> Result<Self, String> {
        let mut d: Option<Design> = None;
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with("HNF") {
                continue;
            }
            let mut it = line.split_whitespace();
            let cmd = it.next().ok_or("empty")?;
            match cmd {
                "design" => {
                    d = Some(Design::new(it.next().unwrap_or("top")));
                }
                "dattr" => {
                    let k = it.next().ok_or("dattr k")?;
                    let v = it.collect::<Vec<_>>().join(" ");
                    d.as_mut().ok_or("no design")?.attrs.set(k, v);
                }
                "port" => {
                    let n = it.next().ok_or("port")?.to_string();
                    let dir = match it.next().unwrap_or("in") {
                        "out" => PortDir::Out,
                        "inout" => PortDir::Inout,
                        _ => PortDir::In,
                    };
                    d.as_mut().ok_or("no design")?.add_port(n, dir);
                }
                "pattr" => {
                    let n = it.next().ok_or("pattr")?;
                    let k = it.next().ok_or("pattr k")?;
                    let v = it.collect::<Vec<_>>().join(" ");
                    d.as_mut().ok_or("no design")?.set_port_attr(n, k, v)?;
                }
                "cell" => {
                    let n = it.next().ok_or("cell")?.to_string();
                    let kind = it.next().ok_or("kind")?;
                    let rest = it.collect::<Vec<_>>().join(" ");
                    let ck = match kind {
                        "Lut6" => {
                            let init = rest
                                .trim()
                                .trim_start_matches("0x")
                                .trim_start_matches("0X");
                            CellKind::Lut6 {
                                init: u64::from_str_radix(init, 16).unwrap_or(0),
                            }
                        }
                        "Hff" => CellKind::Hff,
                        "IobOut" => CellKind::IobOut,
                        "Mac27" => CellKind::Mac27,
                        "Ila" => CellKind::Ila { net: rest },
                        "Bram18" => CellKind::Bram18,
                        "BlackBox" => CellKind::BlackBox { module: rest },
                        other => return Err(format!("unknown kind {other}")),
                    };
                    d.as_mut().ok_or("no design")?.add_cell(n, ck);
                }
                "cattr" => {
                    let n = it.next().ok_or("cattr")?;
                    let k = it.next().ok_or("cattr k")?;
                    let v = it.collect::<Vec<_>>().join(" ");
                    d.as_mut().ok_or("no design")?.set_cell_attr(n, k, v)?;
                }
                "net" => {
                    let n = it.next().ok_or("net")?.to_string();
                    let des = d.as_mut().ok_or("no design")?;
                    for ep in it {
                        let (cell, pin) = ep.split_once('/').ok_or("ep")?;
                        des.connect(&n, cell, pin);
                    }
                    if des.net(&n).is_none() {
                        des.merge_net(Net {
                            name: n,
                            endpoints: vec![],
                            attrs: Attrs::default(),
                        });
                    }
                }
                "nattr" => {
                    let n = it.next().ok_or("nattr")?;
                    let k = it.next().ok_or("nattr k")?;
                    let v = it.collect::<Vec<_>>().join(" ");
                    d.as_mut().ok_or("no design")?.set_net_attr(n, k, v)?;
                }
                "inst" => {
                    let n = it.next().ok_or("inst")?.to_string();
                    let m = it.next().ok_or("imod")?.to_string();
                    let mut inst = Instance {
                        name: n,
                        module: m,
                        conns: vec![],
                        attrs: Attrs::default(),
                    };
                    if let Some(rest) = it.next() {
                        for pair in rest.split(',') {
                            if let Some((a, b)) = pair.split_once(':') {
                                inst.conns.push((a.into(), b.into()));
                            }
                        }
                    }
                    d.as_mut().ok_or("no design")?.instances.push(inst);
                }
                other => return Err(format!("hnf cmd {other}")),
            }
        }
        d.ok_or_else(|| "no design".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clone_data_omits_indexes_until_rebuild() {
        let mut d = Design::new("idx");
        d.add_cell("ff", CellKind::Hff);
        d.connect("q", "ff", "Q");
        let mut c = d.clone_data();
        assert!(c.cell("ff").is_some());
        assert_eq!(c.net_on("ff", "Q"), Some("q"));
        c.rebuild_indexes();
        assert_eq!(c.cell("ff").map(|x| x.name.as_str()), Some("ff"));
        assert_eq!(c.net_on("ff", "Q"), Some("q"));
    }

    #[test]
    fn connect_is_subquadratic_on_many_nets() {
        let mut d = Design::new("wide");
        let t0 = std::time::Instant::now();
        for i in 0..20_000u32 {
            d.add_cell(format!("c{i}"), CellKind::Hff);
            d.connect(format!("n{i}"), format!("c{i}"), "Q");
        }
        let ms = t0.elapsed().as_millis();
        assert_eq!(d.nets.len(), 20_000);
        assert_eq!(d.net("n19999").map(|n| n.name.as_str()), Some("n19999"));
        assert!(
            ms < 800,
            "connect must stay O(1) per net (HashMap index), took {ms}ms for 20k"
        );
        let t1 = std::time::Instant::now();
        let mut hits = 0usize;
        for i in 0..20_000u32 {
            let cell = format!("c{i}");
            if d.net_on(&cell, "Q").is_some() {
                hits += 1;
            }
        }
        let lookup_ms = t1.elapsed().as_millis();
        assert_eq!(hits, 20_000);
        assert_eq!(d.net_on("c0", "Q"), Some("n0"));
        assert_eq!(d.net_on("c19999", "Q"), Some("n19999"));
        assert!(
            lookup_ms < 800,
            "20k net_on lookups must stay O(1), took {lookup_ms}ms"
        );
    }

    #[test]
    fn merge_net_and_instantiate_are_subquadratic() {
        let mut d = Design::new("merge");
        let t0 = std::time::Instant::now();
        for i in 0..20_000u32 {
            d.add_cell(format!("c{i}"), CellKind::Hff);
            d.merge_net(Net {
                name: format!("n{i}"),
                endpoints: vec![Endpoint {
                    cell: format!("c{i}"),
                    pin: "Q".into(),
                }],
                attrs: Attrs::default(),
            });
        }
        let merge_ms = t0.elapsed().as_millis();
        assert_eq!(d.nets.len(), 20_000);
        assert_eq!(d.net_on("c0", "Q"), Some("n0"));
        assert_eq!(d.net_on("c19999", "Q"), Some("n19999"));
        assert_eq!(d.cell("c19999").map(|c| c.name.as_str()), Some("c19999"));
        assert!(
            merge_ms < 800,
            "merge_net must stay O(1) per net, took {merge_ms}ms for 20k"
        );

        let mut top = Design::new("top");
        let t1 = std::time::Instant::now();
        for i in 0..2_000u32 {
            let mut child = Design::new(format!("m{i}"));
            child.add_cell("ff", CellKind::Hff);
            child.connect("q", "ff", "Q");
            top.instantiate(&format!("u{i}"), child, &[]);
        }
        let inst_ms = t1.elapsed().as_millis();
        assert_eq!(top.cells.len(), 2_000);
        assert!(top.cell("u1999_ff").is_some());
        assert_eq!(top.net_on("u1999_ff", "Q"), Some("u1999_q"));
        assert!(
            inst_ms < 1500,
            "instantiate of 2k children must stay subquadratic, took {inst_ms}ms"
        );
    }

    #[test]
    fn blinky_has_lut_ff_iob() {
        let d = Design::structural_blinky();
        assert_eq!(d.cells.len(), 3);
        assert!(matches!(d.cell("u_lut").unwrap().kind, CellKind::Lut6 { .. }));
        assert!(matches!(d.cell("u_ff").unwrap().kind, CellKind::Hff));
    }

    #[test]
    fn counter_has_four_distinct_luts() {
        let d = Design::structural_counter();
        let inits = d.lut_inits();
        assert_eq!(inits, INC4_INIT.to_vec());
        assert_eq!(inits.len(), 4);
        for i in 0..4 {
            for j in i + 1..4 {
                assert_ne!(inits[i], inits[j], "bit {i} INIT must differ from bit {j}");
            }
        }
        assert_eq!(d.net_on("u_lut3", "I3"), Some("q3"));
        assert_eq!(d.net_on("u_iob", "I"), Some("q3"));
    }

    #[test]
    fn hnf_round_trip_preserves_attrs_and_init() {
        let mut d = Design::structural_blinky();
        d.mark_debug("q").unwrap();
        d.dont_touch("u_lut").unwrap();
        d.set_loc("led", "IOB_X2Y0").unwrap();
        d.set_iostandard("led", "LVCMOS18").unwrap();
        d.set_drive("led", "12").unwrap();
        d.set_slew("led", "SLOW").unwrap();
        d.set_pulltype("led", "NONE").unwrap();
        d.set_diff_term("led", "FALSE").unwrap();
        d.set_in_term("led", "NONE").unwrap();
        d.attrs.set("top", "blinky");
        let text = d.to_hnf();
        assert!(text.starts_with("HNF 1"), "{text}");
        let back = Design::from_hnf(&text).unwrap();
        assert_eq!(back.name, "blinky");
        assert_eq!(back.lut_inits(), d.lut_inits());
        assert!(back.net("q").unwrap().attrs.flag("mark_debug"));
        assert!(back.cell("u_lut").unwrap().attrs.flag("DONT_TOUCH"));
        assert_eq!(back.ports.iter().find(|p| p.name == "led").unwrap().attrs.get("LOC"), Some("IOB_X2Y0"));
        assert_eq!(
            back.ports.iter().find(|p| p.name == "led").unwrap().attrs.get("IOSTANDARD"),
            Some("LVCMOS18")
        );
        assert_eq!(
            back.ports.iter().find(|p| p.name == "led").unwrap().attrs.get("DRIVE"),
            Some("12")
        );
        assert_eq!(
            back.ports.iter().find(|p| p.name == "led").unwrap().attrs.get("SLEW"),
            Some("SLOW")
        );
        assert_eq!(
            back.ports.iter().find(|p| p.name == "led").unwrap().attrs.get("PULLTYPE"),
            Some("NONE")
        );
        assert_eq!(
            back.ports.iter().find(|p| p.name == "led").unwrap().attrs.get("DIFF_TERM"),
            Some("FALSE")
        );
        assert_eq!(
            back.ports.iter().find(|p| p.name == "led").unwrap().attrs.get("IN_TERM"),
            Some("NONE")
        );
        assert_eq!(back.net_on("u_lut", "I0"), Some("q"));
    }

    #[test]
    fn instantiate_prefixes_and_rewires_ports() {
        let child = Design::structural_blinky();
        let mut top = Design::new("top");
        top.add_port("clk", PortDir::In);
        top.add_port("led", PortDir::Out);
        top.instantiate("u0", child, &[("clk".into(), "clk".into()), ("led".into(), "led".into())]);
        assert!(top.cell("u0_u_lut").is_some());
        assert!(top.cell("u_lut").is_none());
        assert_eq!(top.instances[0].module, "blinky");
        assert!(top.cells.len() >= 3);
    }
}


#[cfg(test)]
mod soft_diag_tests {
    use super::*;

    #[test]
    fn soft_diag_table_line_and_children() {
        let mut parent = SoftDiag::new("child_soft_incomplete", "ibex_core")
            .with_detail("ibex_if_stage")
            .with_span(SoftSpan::file_line("ibex_core.sv", 120));
        parent.push_child(
            SoftDiag::new("generate_not_lowered", "ibex_if_stage")
                .with_span(SoftSpan::file_line("ibex_if_stage.sv", 40)),
        );
        let mr = MapResult {
            design: Design::new("t"),
            softs: vec![parent],
        };
        let lines = mr.soft_table_lines();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("soft name=child_soft_incomplete"));
        assert!(lines[0].contains("span=ibex_core.sv:120"));
        assert!(lines[1].contains("soft name=generate_not_lowered"));
        assert!(mr.has_softs());
    }
}

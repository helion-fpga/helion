//! Helion netlist IR (HNF). Structural cells, attributes, hierarchy, round-trip.

use std::collections::BTreeMap;

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

#[derive(Clone, Debug)]
pub struct Design {
    pub name: String,
    pub ports: Vec<Port>,
    pub cells: Vec<Cell>,
    pub nets: Vec<Net>,
    pub instances: Vec<Instance>,
    pub attrs: Attrs,
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

impl Design {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ports: Vec::new(),
            cells: Vec::new(),
            nets: Vec::new(),
            instances: Vec::new(),
            attrs: Attrs::default(),
        }
    }

    pub fn add_port(&mut self, name: impl Into<String>, dir: PortDir) {
        self.ports.push(Port {
            name: name.into(),
            dir,
            attrs: Attrs::default(),
        });
    }

    pub fn add_cell(&mut self, name: impl Into<String>, kind: CellKind) {
        self.cells.push(Cell {
            name: name.into(),
            kind,
            attrs: Attrs::default(),
        });
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
        if let Some(existing) = self.nets.iter_mut().find(|n| n.name == net_name) {
            existing.endpoints.push(Endpoint {
                cell: cell.into(),
                pin: pin.into(),
            });
        } else {
            self.nets.push(Net {
                name: net_name,
                endpoints: vec![Endpoint {
                    cell: cell.into(),
                    pin: pin.into(),
                }],
                attrs: Attrs::default(),
            });
        }
    }

    pub fn net_on(&self, cell: &str, pin: &str) -> Option<&str> {
        self.nets.iter().find_map(|n| {
            n.endpoints
                .iter()
                .any(|e| e.cell == cell && e.pin == pin)
                .then_some(n.name.as_str())
        })
    }

    pub fn cell(&self, name: &str) -> Option<&Cell> {
        self.cells.iter().find(|c| c.name == name)
    }

    pub fn cell_mut(&mut self, name: &str) -> Option<&mut Cell> {
        self.cells.iter_mut().find(|c| c.name == name)
    }

    pub fn net(&self, name: &str) -> Option<&Net> {
        self.nets.iter().find(|n| n.name == name)
    }

    pub fn net_mut(&mut self, name: &str) -> Option<&mut Net> {
        self.nets.iter_mut().find(|n| n.name == name)
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
            if let Some(ex) = self.nets.iter_mut().find(|x| x.name == n.name) {
                ex.endpoints.extend(n.endpoints);
            } else {
                self.nets.push(n);
            }
        }
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
                    if des.net_mut(&n).is_none() {
                        des.nets.push(Net {
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
        d.attrs.set("top", "blinky");
        let text = d.to_hnf();
        assert!(text.starts_with("HNF 1"), "{text}");
        let back = Design::from_hnf(&text).unwrap();
        assert_eq!(back.name, "blinky");
        assert_eq!(back.lut_inits(), d.lut_inits());
        assert!(back.net("q").unwrap().attrs.flag("mark_debug"));
        assert!(back.cell("u_lut").unwrap().attrs.flag("DONT_TOUCH"));
        assert_eq!(back.ports.iter().find(|p| p.name == "led").unwrap().attrs.get("LOC"), Some("IOB_X2Y0"));
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

//! Helion netlist IR (HNF-in-memory). Structural cells + multi-bit connectivity.

#[derive(Clone, Debug)]
pub struct Design {
    pub name: String,
    pub ports: Vec<Port>,
    pub cells: Vec<Cell>,
    pub nets: Vec<Net>,
}

#[derive(Clone, Debug)]
pub struct Port {
    pub name: String,
    pub dir: PortDir,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PortDir {
    In,
    Out,
}

#[derive(Clone, Debug)]
pub struct Cell {
    pub name: String,
    pub kind: CellKind,
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
}

#[derive(Clone, Debug)]
pub struct Net {
    pub name: String,
    pub endpoints: Vec<Endpoint>,
}

#[derive(Clone, Debug)]
pub struct Endpoint {
    pub cell: String,
    pub pin: String,
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
        }
    }

    pub fn add_port(&mut self, name: impl Into<String>, dir: PortDir) {
        self.ports.push(Port {
            name: name.into(),
            dir,
        });
    }

    pub fn add_cell(&mut self, name: impl Into<String>, kind: CellKind) {
        self.cells.push(Cell {
            name: name.into(),
            kind,
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

    pub fn lut_inits(&self) -> Vec<u64> {
        self.cells
            .iter()
            .filter_map(|c| match c.kind {
                CellKind::Lut6 { init } => Some(init),
                _ => None,
            })
            .collect()
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
}

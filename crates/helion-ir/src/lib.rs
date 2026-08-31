//! Helion netlist IR (HNF-in-memory). Structural cells only in 0.1.

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

    pub fn cell(&self, name: &str) -> Option<&Cell> {
        self.cells.iter().find(|c| c.name == name)
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
}

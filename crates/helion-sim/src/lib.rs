//! Event-driven kernel driven by LUT INIT + FF in the netlist (not a hardcoded toggle).

use helion_ir::{CellKind, Design};
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct Sim {
    pub time: u64,
    lut_init: HashMap<String, u64>,
    lut_i0_ff: HashMap<String, String>,
    ff_d_lut: HashMap<String, String>,
    iob_from_ff: Option<String>,
    ff_q: HashMap<String, bool>,
    pub led: bool,
}

impl Sim {
    pub fn new(d: &Design) -> Self {
        let mut lut_init = HashMap::new();
        for c in &d.cells {
            if let CellKind::Lut6 { init } = c.kind {
                lut_init.insert(c.name.clone(), init);
            }
        }
        let mut lut_i0_ff = HashMap::new();
        let mut ff_d_lut = HashMap::new();
        for n in &d.nets {
            let lut_i0 = n.endpoints.iter().find(|e| e.pin == "I0");
            let ff_q = n.endpoints.iter().find(|e| e.pin == "Q");
            if let (Some(l), Some(f)) = (lut_i0, ff_q) {
                lut_i0_ff.insert(l.cell.clone(), f.cell.clone());
            }
            let lut_o = n.endpoints.iter().find(|e| e.pin == "O");
            let ff_d = n.endpoints.iter().find(|e| e.pin == "D");
            if let (Some(l), Some(f)) = (lut_o, ff_d) {
                ff_d_lut.insert(f.cell.clone(), l.cell.clone());
            }
        }
        let iob_from_ff = d.cells.iter().find_map(|c| {
            if !matches!(c.kind, CellKind::IobOut) {
                return None;
            }
            let net = d.nets.iter().find(|n| {
                n.endpoints
                    .iter()
                    .any(|e| e.cell == c.name && e.pin == "I")
            })?;
            net.endpoints
                .iter()
                .find(|e| e.pin == "Q")
                .map(|e| e.cell.clone())
        });
        let mut ff_q = HashMap::new();
        for c in &d.cells {
            if matches!(c.kind, CellKind::Hff) {
                ff_q.insert(c.name.clone(), false);
            }
        }
        Self {
            time: 0,
            lut_init,
            lut_i0_ff,
            ff_d_lut,
            iob_from_ff,
            ff_q,
            led: false,
        }
    }

    pub fn step_posedge(&mut self, delay: u64) {
        self.time += delay;
        let mut next_q = HashMap::new();
        for (ff, lut) in &self.ff_d_lut {
            let init = self.lut_init.get(lut).copied().unwrap_or(0);
            let i0 = self
                .lut_i0_ff
                .get(lut)
                .and_then(|src| self.ff_q.get(src))
                .copied()
                .unwrap_or(false);
            let o = (init >> (i0 as u64)) & 1 == 1;
            next_q.insert(ff.clone(), o);
        }
        for (k, v) in next_q {
            self.ff_q.insert(k, v);
        }
        self.led = self
            .iob_from_ff
            .as_ref()
            .and_then(|f| self.ff_q.get(f))
            .copied()
            .unwrap_or(false);
    }
}

pub fn run_tb(design: &Design, cycles: u32) -> Vec<bool> {
    let mut s = Sim::new(design);
    let mut wave = Vec::new();
    for _ in 0..cycles {
        s.step_posedge(10);
        wave.push(s.led);
    }
    wave
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_ir::{CellKind, Design, PortDir};

    #[test]
    fn inverter_toggles_const0_does_not() {
        let inv = run_tb(&Design::structural_blinky(), 4);
        assert!(inv.contains(&true) && inv.contains(&false));

        let mut z = Design::new("z");
        z.add_port("clk", PortDir::In);
        z.add_port("led", PortDir::Out);
        z.add_cell("u_lut", CellKind::Lut6 { init: 0 });
        z.add_cell("u_ff", CellKind::Hff);
        z.add_cell("u_iob", CellKind::IobOut);
        z.connect("d", "u_lut", "O");
        z.connect("d", "u_ff", "D");
        z.connect("q", "u_ff", "Q");
        z.connect("q", "u_lut", "I0");
        z.connect("q", "u_iob", "I");
        let w = run_tb(&z, 4);
        assert!(w.iter().all(|&b| !b), "const0 LUT must not toggle LED: {w:?}");
    }
}

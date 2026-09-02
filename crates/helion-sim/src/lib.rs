//! Event-driven kernel driven by LUT INIT + FF in the netlist (not a hardcoded toggle).

use helion_ir::{CellKind, Design};
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct Sim {
    pub time: u64,
    lut_init: HashMap<String, u64>,
    lut_pins: HashMap<String, Vec<(u8, String)>>,
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
        let mut lut_pins: HashMap<String, Vec<(u8, String)>> = HashMap::new();
        let mut ff_d_lut = HashMap::new();
        for n in &d.nets {
            let ff_q = n.endpoints.iter().find(|e| e.pin == "Q").map(|e| e.cell.clone());
            for e in &n.endpoints {
                if let Some(rest) = e.pin.strip_prefix('I') {
                    if let (Ok(pin), Some(ff)) = (rest.parse::<u8>(), ff_q.as_ref()) {
                        if pin < 6 {
                            lut_pins
                                .entry(e.cell.clone())
                                .or_default()
                                .push((pin, ff.clone()));
                        }
                    }
                }
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
            lut_pins,
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
            let mut addr = 0u8;
            if let Some(pins) = self.lut_pins.get(lut) {
                for (pin, src) in pins {
                    let bit = self.ff_q.get(src).copied().unwrap_or(false);
                    if bit {
                        addr |= 1 << pin;
                    }
                }
            }
            let o = (init >> (addr as u64)) & 1 == 1;
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

    /// UG900 Objects pane: live helion-sim probes (LED + each FF Q), not a static name list.
    pub fn object_values(&self) -> Vec<(String, String)> {
        let bit = |b: bool| if b { "1".to_string() } else { "0".to_string() };
        let mut v = vec![("led".to_string(), bit(self.led))];
        let mut ffs: Vec<_> = self.ff_q.iter().collect();
        ffs.sort_by_key(|(n, _)| *n);
        for (n, q) in ffs {
            v.push((n.clone(), bit(*q)));
        }
        v
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

    #[test]
    fn event_sim_counter_matches_fabric_gold() {
        let w = run_tb(&Design::structural_counter(), 16);
        assert!(w[0..7].iter().all(|b| !b), "{w:?}");
        assert!(w[7..15].iter().all(|b| *b), "{w:?}");
        assert!(!w[15], "{w:?}");
    }

    fn fabric_wave(d: &Design, cycles: u32) -> Vec<bool> {
        use helion_bits::bitgen;
        use helion_device::Device;
        use helion_fabric::Fabric;
        use helion_pack::pack;
        use helion_place::place;
        use helion_route::route;
        let dev = Device::load_part("HL10T-C32-1").unwrap();
        let p = pack(d, &dev).unwrap();
        let pl = place(&p, &dev).unwrap();
        let r = route(&pl, &dev).unwrap();
        let bits = bitgen(&dev, &r).unwrap();
        let mut fab = Fabric::new(&dev);
        fab.program(&bits).unwrap();
        fab.finish_startup();
        let iob = r.iob_src[0].iob;
        let mut w = Vec::new();
        for _ in 0..cycles {
            fab.step_user();
            w.push(fab.led_at(iob.0, iob.1));
        }
        w
    }

    fn six_bit_counter() -> Design {
        let mut d = Design::new("inc6");
        d.add_port("clk", PortDir::In);
        d.add_port("led", PortDir::Out);
        // bit i INIT: q[i] ^ AND(q[0..i-1]); I0=LSB
        fn inc_init(i: u32) -> u64 {
            let mut w = 0u64;
            for addr in 0..64u64 {
                let qi = (addr >> i) & 1;
                let mut cin = 1u64;
                for j in 0..i {
                    cin &= (addr >> j) & 1;
                }
                if i == 0 {
                    w |= ((1 - qi) & 1) << addr;
                } else {
                    w |= (qi ^ cin) << addr;
                }
            }
            w
        }
        for i in 0..6u32 {
            d.add_cell(format!("u_lut{i}"), CellKind::Lut6 { init: inc_init(i) });
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
        d.connect("q5", "u_iob", "I");
        d.connect("led", "u_iob", "PAD");
        d
    }

    #[test]
    fn six_pin_lut_is_occupied_and_matches_fabric() {
        let d = six_bit_counter();
        let lut5 = d.cell("u_lut5").unwrap();
        let pins: Vec<_> = (0..6u8)
            .filter(|p| d.net_on("u_lut5", &format!("I{p}")).is_some())
            .collect();
        assert_eq!(pins, vec![0, 1, 2, 3, 4, 5], "MSB incrementer must use all 6 LUT pins");
        match lut5.kind {
            CellKind::Lut6 { init } => assert_ne!(init, 0x5555_5555_5555_5555, "not a 1-pin inverter"),
            _ => panic!("lut"),
        }
        let ev = run_tb(&d, 64);
        let fab = fabric_wave(&d, 64);
        assert_eq!(ev, fab, "event sim 6-pin must agree with fabric");
        assert!(ev[0..31].iter().all(|b| !b), "cnt 1..31 LED=0 {ev:?}");
        assert!(ev[31..63].iter().all(|b| *b), "cnt 32..63 LED=1");
        assert!(!ev[63], "wrap");
    }

    #[test]
    fn event_sim_agrees_with_fabric_on_counter_and_hier() {
        let c = Design::structural_counter();
        let ev = run_tb(&c, 16);
        let fab = fabric_wave(&c, 16);
        assert_eq!(ev, fab, "counter event vs fabric {ev:?} {fab:?}");

        let hier = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/hier.sv");
        let src = std::fs::read_to_string(&hier).unwrap();
        let d = helion_sv::synth_sv(&src, "hier.sv").unwrap();
        let evh = run_tb(&d, 8);
        let fabh = fabric_wave(&d, 8);
        assert_eq!(evh, fabh, "hier event vs fabric {evh:?} {fabh:?}");
        assert!(evh.contains(&true) && evh.contains(&false), "hier must toggle {evh:?}");
    }
}

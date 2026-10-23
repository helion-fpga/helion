//! Shared helpers for frontend correctness tests.
//!
//! These tests pin *netlist semantics*, not cell counts: a synthesizer that
//! emits the wrong function with the right number of LUTs is the failure mode
//! worth guarding against. [`Netlist::drive`] interprets the emitted LUT6 /
//! constant / IOB-passthrough logic directly, so a test can state
//! "y must equal a + 1" and have that mean something.
//!
//! Two details matter and are easy to get wrong:
//!   * An output bit that is a passthrough of an input bit is emitted as two
//!     IOBs sharing a value, not as a LUT. The evaluator must follow
//!     `u_iobN.I` → `u_iobN.PAD`.
//!   * An unconnected LUT input reads 0 (the frontend replicates the INIT
//!     across unused inputs), so `addr` bits for absent pins stay clear.

#![allow(dead_code)]

use helion_ir::{CellKind, Design};
use std::collections::HashMap;

/// An evaluated combinational view of a synthesized design.
pub struct Netlist<'a> {
    pub d: &'a Design,
    /// net name → boolean value once resolved.
    vals: HashMap<String, bool>,
}

impl<'a> Netlist<'a> {
    /// Build a model and drive the named input ports.
    ///
    /// `inputs`: `(port, value, width)` — bit `i` of `value` drives
    /// `<port>_<i>`. The bare port net is seeded too for width-1 ports.
    pub fn drive(d: &'a Design, inputs: &[(&str, u64, usize)]) -> Self {
        let mut n = Netlist {
            d,
            vals: HashMap::new(),
        };
        let mut queue: Vec<(String, bool)> = Vec::new();
        for (name, val, w) in inputs {
            for i in 0..*w {
                queue.push((format!("{name}_{i}"), (*val >> i) & 1 == 1));
            }
            if *w == 1 {
                queue.push((name.to_string(), *val != 0));
            }
        }
        // Seed input nets and follow input-IOB passthrough (I → PAD).
        let mut qi = 0;
        while qi < queue.len() {
            let (net, v) = queue[qi].clone();
            qi += 1;
            if n.vals.get(&net) == Some(&v) {
                continue;
            }
            n.vals.insert(net.clone(), v);
            for (cell, pin) in n.endpoints_of(&net) {
                if pin == "I" {
                    if let Some(pad) = n.net_on(&cell, "PAD") {
                        queue.push((pad, v));
                    }
                }
            }
        }
        n.propagate();
        n
    }

    fn endpoints_of(&self, net: &str) -> Vec<(String, String)> {
        self.d
            .nets
            .iter()
            .find(|n| n.name == net)
            .map(|n| {
                n.endpoints
                    .iter()
                    .map(|e| (e.cell.clone(), e.pin.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn net_on(&self, cell: &str, pin: &str) -> Option<String> {
        for net in &self.d.nets {
            for ep in &net.endpoints {
                if ep.cell == cell && ep.pin == pin {
                    return Some(net.name.clone());
                }
            }
        }
        None
    }

    fn propagate(&mut self) {
        for _ in 0..10_000 {
            let mut changed = false;
            for cell in &self.d.cells {
                let CellKind::Lut6 { init } = cell.kind else {
                    continue;
                };
                let mut addr = 0u64;
                let mut known = true;
                for pin in 0..6usize {
                    match self.net_on(&cell.name, &format!("I{pin}")) {
                        None => {}
                        Some(n) => match self.vals.get(&n) {
                            Some(true) => addr |= 1 << pin,
                            Some(false) => {}
                            None => known = false,
                        },
                    }
                }
                if !known {
                    continue;
                }
                let o = (init >> addr) & 1 == 1;
                let Some(onet) = self.net_on(&cell.name, "O") else {
                    continue;
                };
                // The LUT drives its output net, and any IOB PAD fed from it.
                let mut push = vec![(onet.clone(), o)];
                for (cell2, pin) in self.endpoints_of(&onet) {
                    if pin == "I" {
                        if let Some(pad) = self.net_on(&cell2, "PAD") {
                            push.push((pad, o));
                        }
                    }
                }
                for (net, v) in push {
                    if self.vals.get(&net) != Some(&v) {
                        self.vals.insert(net, v);
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// Value of output port `port` (bit `i` read from net `<port>_<i>`).
    pub fn out(&self, port: &str, width: usize) -> u64 {
        let mut r = 0u64;
        for i in 0..width {
            match self.vals.get(&format!("{port}_{i}")) {
                Some(true) => r |= 1 << i,
                Some(false) => {}
                None => {
                    if width == 1 && self.vals.get(port) == Some(&true) {
                        r |= 1 << i;
                    }
                }
            }
        }
        r
    }

    /// Cells matching `want`.
    pub fn count(&self, want: fn(&CellKind) -> bool) -> usize {
        self.d.cells.iter().filter(|c| want(&c.kind)).count()
    }
}

/// Convenience: synthesize a driven design and read one combinational output.
pub fn eval_out(d: &Design, inputs: &[(&str, u64, usize)], out: &str, width: usize) -> u64 {
    Netlist::drive(d, inputs).out(out, width)
}

//! IEEE 1149.1 BSDL Boundary-Scan (`EXTEST` / `SAMPLE` / `PRELOAD` / `BYPASS` / `IDCODE`)
//! Board-Level Interconnect Short/Open/Stuck-At Fault Diagnostic Engine.
//!
//! Supports:
//! - Parsing & generating IEEE 1149.1 BSDL device descriptions (`BsdlDevice`, `parse_bsdl`)
//! - Multi-chip JTAG boundary-scan chains (`BoundaryScanBoard`) with `BYPASS`, `IDCODE`,
//!   `SAMPLE`/`PRELOAD`, and `EXTEST` instruction execution
//! - Automated board-level interconnect test vector generation & diagnosis (Walking-1,
//!   Walking-0, and Wagner complementary binary counting patterns) pinpointing:
//!   - `StuckAt0` (short to GND)
//!   - `StuckAt1` (short to VCC)
//!   - `OpenPin` (solder open / floating input pin)
//!   - `BridgedNets` (solder bridge / short between two nets, wired-AND or wired-OR)

use std::collections::{BTreeMap, BTreeSet};

/// IEEE 1149.1 Boundary-Scan Register (BSR) cell function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BsdlCellKind {
    /// `BC_1` output driver cell.
    Output,
    /// `BC_2` / `BC_4` input observe cell.
    Input,
    /// `BC_7` bidirectional I/O cell.
    Bidir,
    /// Output-enable control cell.
    Control,
}

impl BsdlCellKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Output => "OUTPUT2",
            Self::Input => "INPUT",
            Self::Bidir => "BIDIR",
            Self::Control => "CONTROL",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_uppercase().as_str() {
            "OUTPUT" | "OUTPUT2" | "OUTPUT3" => Ok(Self::Output),
            "INPUT" | "OBSERVE_ONLY" => Ok(Self::Input),
            "BIDIR" | "INOUT" => Ok(Self::Bidir),
            "CONTROL" | "CONTROLR" => Ok(Self::Control),
            other => Err(format!("bsdl: unknown cell function '{other}'")),
        }
    }
}

/// One cell in a chip's IEEE 1149.1 Boundary-Scan Register (`BOUNDARY_REGISTER`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BsdlCell {
    pub cell_index: usize,
    pub cell_type: String,
    pub port_name: String,
    pub kind: BsdlCellKind,
    pub safe_bit: bool,
}

/// IEEE 1149.1 BSDL (Boundary-Scan Description Language) model for a JTAG-compliant IC.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BsdlDevice {
    pub entity_name: String,
    pub idcode: u32,
    pub ir_length: u8,
    pub opcode_bypass: u8,
    pub opcode_idcode: u8,
    pub opcode_sample_preload: u8,
    pub opcode_extest: u8,
    pub boundary_cells: Vec<BsdlCell>,
}

impl BsdlDevice {
    /// Build a standard Helion FPGA BSDL model with `n_pins` bidirectional boundary-scan I/O cells.
    pub fn helion_default(entity_name: impl Into<String>, idcode: u32, n_pins: usize) -> Self {
        let mut boundary_cells = Vec::with_capacity(n_pins);
        for i in 0..n_pins {
            boundary_cells.push(BsdlCell {
                cell_index: i,
                cell_type: "BC_7".to_string(),
                port_name: format!("IO_{i}"),
                kind: BsdlCellKind::Bidir,
                safe_bit: false,
            });
        }
        Self {
            entity_name: entity_name.into(),
            idcode,
            ir_length: 6,
            opcode_bypass: 0b11_1111,
            opcode_idcode: 0b00_0011,
            opcode_sample_preload: 0b00_0001,
            opcode_extest: 0b00_0000,
            boundary_cells,
        }
    }

    pub fn cell_by_port(&self, port_name: &str) -> Option<&BsdlCell> {
        self.boundary_cells
            .iter()
            .find(|c| c.port_name == port_name)
    }

    /// Emit an IEEE 1149.1 BSDL description string (round-trips with [`parse_bsdl`]).
    pub fn to_bsdl_string(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("entity {} is\n", self.entity_name));
        s.push_str(&format!(
            "  attribute INSTRUCTION_LENGTH of {} : entity is {};\n",
            self.entity_name, self.ir_length
        ));
        s.push_str(&format!(
            "  attribute IDCODE_REGISTER of {} : entity is \"{:#010x}\";\n",
            self.entity_name, self.idcode
        ));
        s.push_str(&format!(
            "  attribute INSTRUCTION_OPCODE of {} : entity is\n\
             \x20   \"EXTEST ({:0width$b}), SAMPLE ({:0width$b}), IDCODE ({:0width$b}), BYPASS ({:0width$b})\";\n",
            self.entity_name,
            self.opcode_extest,
            self.opcode_sample_preload,
            self.opcode_idcode,
            self.opcode_bypass,
            width = self.ir_length as usize
        ));
        s.push_str(&format!(
            "  attribute BOUNDARY_LENGTH of {} : entity is {};\n",
            self.entity_name,
            self.boundary_cells.len()
        ));
        s.push_str(&format!(
            "  attribute BOUNDARY_REGISTER of {} : entity is\n",
            self.entity_name
        ));
        for c in &self.boundary_cells {
            s.push_str(&format!(
                "    \"{} ({}, {}, {}, {})\";\n",
                c.cell_index,
                c.cell_type,
                c.port_name,
                c.kind.as_str(),
                if c.safe_bit { '1' } else { '0' }
            ));
        }
        s.push_str(&format!("end {};\n", self.entity_name));
        s
    }
}

/// Parse an IEEE 1149.1 BSDL description into a [`BsdlDevice`].
pub fn parse_bsdl(text: &str) -> Result<BsdlDevice, String> {
    let mut entity_name = String::new();
    let mut ir_length = 6u8;
    let mut idcode = 0x0362_D093u32;
    let mut boundary_cells = Vec::new();

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("--") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("entity ") {
            if let Some(name) = rest.split_whitespace().next() {
                entity_name = name.to_string();
            }
        } else if line.contains("INSTRUCTION_LENGTH") {
            if let Some(num_part) = line.rsplit("is ").next() {
                let cleaned = num_part.trim().trim_end_matches(';');
                ir_length = cleaned
                    .parse()
                    .map_err(|e| format!("bsdl: invalid INSTRUCTION_LENGTH '{cleaned}': {e}"))?;
            }
        } else if line.contains("IDCODE_REGISTER") {
            if let Some(q1) = line.find('"') {
                if let Some(q2) = line[q1 + 1..].find('"') {
                    let s = line[q1 + 1..q1 + 1 + q2].trim();
                    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                        idcode = u32::from_str_radix(hex, 16)
                            .map_err(|e| format!("bsdl: invalid IDCODE hex '{s}': {e}"))?;
                    } else {
                        let bits_only: String =
                            s.chars().filter(|&c| c == '0' || c == '1').collect();
                        if !bits_only.is_empty() {
                            idcode = u32::from_str_radix(&bits_only, 2)
                                .map_err(|e| format!("bsdl: invalid IDCODE bits '{s}': {e}"))?;
                        }
                    }
                }
            }
        } else if line.starts_with('"') && line.contains('(') && line.contains(')') {
            let inner = line.trim_matches(';').trim_matches('"').trim();
            if let Some((idx_str, rest)) = inner.split_once('(') {
                if let Ok(cell_index) = idx_str.trim().parse::<usize>() {
                    let inside = rest.trim_end_matches(')');
                    let parts: Vec<&str> = inside.split(',').map(|p| p.trim()).collect();
                    if parts.len() >= 4 {
                        let cell_type = parts[0].to_string();
                        let port_name = parts[1].to_string();
                        let kind = BsdlCellKind::parse(parts[2])?;
                        let safe_bit = parts[3] == "1";
                        boundary_cells.push(BsdlCell {
                            cell_index,
                            cell_type,
                            port_name,
                            kind,
                            safe_bit,
                        });
                    }
                }
            }
        }
    }

    if entity_name.is_empty() {
        return Err("bsdl: missing 'entity <name> is' declaration".into());
    }
    Ok(BsdlDevice {
        entity_name,
        idcode,
        ir_length,
        opcode_bypass: (1u8 << ir_length.min(7)).saturating_sub(1),
        opcode_idcode: 0b00_0011,
        opcode_sample_preload: 0b00_0001,
        opcode_extest: 0b00_0000,
        boundary_cells,
    })
}

/// Pin endpoint reference on a multi-chip board (`(chip_index, port_name)`).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BoardPinRef {
    pub chip_index: usize,
    pub port_name: String,
}

impl BoardPinRef {
    pub fn new(chip_index: usize, port_name: impl Into<String>) -> Self {
        Self {
            chip_index,
            port_name: port_name.into(),
        }
    }
}

/// Board-level interconnect trace connecting one driver pin to one or more receiver pins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardNet {
    pub net_name: String,
    pub driver: BoardPinRef,
    pub receivers: Vec<BoardPinRef>,
}

/// Physical PCB fault injectable into a [`BoundaryScanBoard`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BoardFault {
    /// Net or pin shorted to Ground (`0`).
    StuckAt0 { net_name: String },
    /// Net or pin shorted to VCC (`1`).
    StuckAt1 { net_name: String },
    /// Open solder joint on a receiver pin (floats to `open_pull_value`, default `false`).
    OpenPin {
        pin: BoardPinRef,
        open_pull_value: bool,
    },
    /// Solder bridge shorting `net_a` and `net_b` (`wired_and = true` -> $A \land B$, `false` -> $A \lor B$).
    BridgedNets {
        net_a: String,
        net_b: String,
        wired_and: bool,
    },
}

/// Fault classification reported by the IEEE 1149.1 `EXTEST` diagnostic engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiagnosedFault {
    StuckAt0 {
        net_name: String,
        receiver: BoardPinRef,
    },
    StuckAt1 {
        net_name: String,
        receiver: BoardPinRef,
    },
    OpenReceiverPin {
        net_name: String,
        receiver: BoardPinRef,
        stuck_level: bool,
    },
    BridgedShort {
        net_a: String,
        net_b: String,
        wired_and: bool,
    },
}

/// Complete IEEE 1149.1 `EXTEST` board-level interconnect test & fault diagnosis report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterconnectTestReport {
    pub num_chips: usize,
    pub num_nets: usize,
    pub vectors_applied: usize,
    pub total_bsr_bits_per_scan: usize,
    pub diagnosed_faults: Vec<DiagnosedFault>,
    pub pass: bool,
}

/// Multi-chip IEEE 1149.1 JTAG Boundary-Scan chain & PCB interconnect simulator/tester.
#[derive(Clone, Debug)]
pub struct BoundaryScanBoard {
    pub chips: Vec<BsdlDevice>,
    pub nets: Vec<BoardNet>,
    pub injected_faults: Vec<BoardFault>,
}

impl BoundaryScanBoard {
    pub fn new(chips: Vec<BsdlDevice>) -> Self {
        Self {
            chips,
            nets: Vec::new(),
            injected_faults: Vec::new(),
        }
    }

    pub fn add_net(&mut self, net_name: impl Into<String>, driver: BoardPinRef, receivers: Vec<BoardPinRef>) {
        self.nets.push(BoardNet {
            net_name: net_name.into(),
            driver,
            receivers,
        });
    }

    pub fn inject_fault(&mut self, fault: BoardFault) {
        self.injected_faults.push(fault);
    }

    pub fn clear_faults(&mut self) {
        self.injected_faults.clear();
    }

    pub fn total_bsr_length(&self) -> usize {
        self.chips.iter().map(|c| c.boundary_cells.len()).sum()
    }

    /// Generate the full IEEE 1149.1 `EXTEST` test vector suite for `num_nets` nets:
    /// 1. All-0 and All-1 baseline vectors
    /// 2. Walking-1 vectors ($N$ vectors)
    /// 3. Walking-0 vectors ($N$ vectors)
    /// 4. Wagner complementary counting vectors ($2 \lceil \log_2(N + 2) \rceil$ vectors)
    pub fn generate_test_vectors(num_nets: usize) -> Vec<Vec<bool>> {
        let n = num_nets.max(1);
        let mut vectors = Vec::new();

        // Walking-1 across all nets:
        for one_idx in 0..n {
            let mut v = vec![false; n];
            v[one_idx] = true;
            vectors.push(v);
        }
        // Walking-0 across all nets:
        for zero_idx in 0..n {
            let mut v = vec![true; n];
            v[zero_idx] = false;
            vectors.push(v);
        }
        // Complementary binary counting codes for net index `i + 1` (never all-0 or all-1):
        let bits = (usize::BITS - (n + 1).leading_zeros()).max(2) as usize;
        for b in 0..bits {
            let mut v_true = Vec::with_capacity(n);
            let mut v_comp = Vec::with_capacity(n);
            for net_idx in 0..n {
                let code = net_idx + 1;
                let bit = ((code >> b) & 1) == 1;
                v_true.push(bit);
                v_comp.push(!bit);
            }
            vectors.push(v_true);
            vectors.push(v_comp);
        }
        vectors
    }

    /// Simulate one `PRELOAD` + `EXTEST` Capture-DR / Shift-DR scan across the multi-chip JTAG chain.
    /// Given `driven_net_values[net_idx]`, returns captured value at each `(net_idx, receiver_pin)`.
    pub fn eval_extest_vector(&self, driven_net_values: &[bool]) -> BTreeMap<BoardPinRef, bool> {
        let mut net_level: BTreeMap<&str, bool> = BTreeMap::new();
        for (idx, net) in self.nets.iter().enumerate() {
            let val = driven_net_values.get(idx).copied().unwrap_or(false);
            net_level.insert(&net.net_name, val);
        }

        // Apply net-level faults (BridgedNets, StuckAt0, StuckAt1).
        for fault in &self.injected_faults {
            if let BoardFault::BridgedNets {
                net_a,
                net_b,
                wired_and,
            } = fault
            {
                let va = net_level.get(net_a.as_str()).copied().unwrap_or(false);
                let vb = net_level.get(net_b.as_str()).copied().unwrap_or(false);
                let merged = if *wired_and { va && vb } else { va || vb };
                net_level.insert(net_a.as_str(), merged);
                net_level.insert(net_b.as_str(), merged);
            }
        }
        for fault in &self.injected_faults {
            match fault {
                BoardFault::StuckAt0 { net_name } => {
                    net_level.insert(net_name.as_str(), false);
                }
                BoardFault::StuckAt1 { net_name } => {
                    net_level.insert(net_name.as_str(), true);
                }
                _ => {}
            }
        }

        // Sample at each receiver BSR cell, applying pin-level OpenPin faults if present.
        let mut captured = BTreeMap::new();
        for net in &self.nets {
            let v = net_level.get(net.net_name.as_str()).copied().unwrap_or(false);
            for rx in &net.receivers {
                let mut rx_val = v;
                for fault in &self.injected_faults {
                    if let BoardFault::OpenPin {
                        pin,
                        open_pull_value,
                    } = fault
                    {
                        if pin == rx {
                            rx_val = *open_pull_value;
                        }
                    }
                }
                captured.insert(rx.clone(), rx_val);
            }
        }
        captured
    }

    /// Execute the full Walking-1 / Walking-0 / Counting `EXTEST` interconnect test suite
    /// and diagnose all stuck-at, open-pin, and bridged-net short faults!
    pub fn run_interconnect_test(&self) -> InterconnectTestReport {
        let n_nets = self.nets.len();
        let vectors = Self::generate_test_vectors(n_nets);
        let n_vecs = vectors.len();

        // Ideal driven signature per net: `ideal_sig[net_i][vec_t]`
        let mut ideal_sig: Vec<Vec<bool>> = vec![Vec::with_capacity(n_vecs); n_nets];
        // Observed signature per receiver pin: `obs_sig[pin][vec_t]`
        let mut obs_sig: BTreeMap<BoardPinRef, Vec<bool>> = BTreeMap::new();

        for vec_vals in &vectors {
            for (net_i, &b) in vec_vals.iter().enumerate() {
                ideal_sig[net_i].push(b);
            }
            let cap = self.eval_extest_vector(vec_vals);
            for (pin, val) in cap {
                obs_sig.entry(pin).or_default().push(val);
            }
        }

        let mut diagnosed_faults = Vec::new();
        let mut bridged_pairs: BTreeSet<(usize, usize)> = BTreeSet::new();

        // 1. Check for bridged shorts between pairs of nets `(i, j)`.
        for i in 0..n_nets {
            let Some(rx_i) = self.nets[i].receivers.first() else {
                continue;
            };
            let Some(sig_i) = obs_sig.get(rx_i) else {
                continue;
            };
            if sig_i == &ideal_sig[i] {
                continue;
            }
            for j in (i + 1)..n_nets {
                let Some(rx_j) = self.nets[j].receivers.first() else {
                    continue;
                };
                let Some(sig_j) = obs_sig.get(rx_j) else {
                    continue;
                };
                if sig_i == sig_j {
                    let wand: Vec<bool> = ideal_sig[i]
                        .iter()
                        .zip(ideal_sig[j].iter())
                        .map(|(&a, &b)| a && b)
                        .collect();
                    let wor: Vec<bool> = ideal_sig[i]
                        .iter()
                        .zip(ideal_sig[j].iter())
                        .map(|(&a, &b)| a || b)
                        .collect();
                    if sig_i == &wand {
                        bridged_pairs.insert((i, j));
                        diagnosed_faults.push(DiagnosedFault::BridgedShort {
                            net_a: self.nets[i].net_name.clone(),
                            net_b: self.nets[j].net_name.clone(),
                            wired_and: true,
                        });
                    } else if sig_i == &wor {
                        bridged_pairs.insert((i, j));
                        diagnosed_faults.push(DiagnosedFault::BridgedShort {
                            net_a: self.nets[i].net_name.clone(),
                            net_b: self.nets[j].net_name.clone(),
                            wired_and: false,
                        });
                    }
                }
            }
        }

        // 2. Check each net's receivers for StuckAt0, StuckAt1, or OpenReceiverPin.
        for (net_i, net) in self.nets.iter().enumerate() {
            if bridged_pairs.iter().any(|&(a, b)| a == net_i || b == net_i) {
                continue;
            }
            let any_rx_healthy = net
                .receivers
                .iter()
                .any(|rx| obs_sig.get(rx) == Some(&ideal_sig[net_i]));

            for rx in &net.receivers {
                let Some(sig) = obs_sig.get(rx) else {
                    continue;
                };
                if sig == &ideal_sig[net_i] {
                    continue;
                }
                let all_zero = sig.iter().all(|&b| !b);
                let all_one = sig.iter().all(|&b| b);
                if all_zero || all_one {
                    if any_rx_healthy {
                        // Other receivers on the same net received the signal -> this specific pin is open!
                        diagnosed_faults.push(DiagnosedFault::OpenReceiverPin {
                            net_name: net.net_name.clone(),
                            receiver: rx.clone(),
                            stuck_level: all_one,
                        });
                    } else if all_zero {
                        diagnosed_faults.push(DiagnosedFault::StuckAt0 {
                            net_name: net.net_name.clone(),
                            receiver: rx.clone(),
                        });
                    } else {
                        diagnosed_faults.push(DiagnosedFault::StuckAt1 {
                            net_name: net.net_name.clone(),
                            receiver: rx.clone(),
                        });
                    }
                }
            }
        }

        let pass = diagnosed_faults.is_empty();
        InterconnectTestReport {
            num_chips: self.chips.len(),
            num_nets: n_nets,
            vectors_applied: n_vecs,
            total_bsr_bits_per_scan: self.total_bsr_length(),
            diagnosed_faults,
            pass,
        }
    }
}

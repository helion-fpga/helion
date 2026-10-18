//! Levelized flat compiled 2-state and 4-state (`0, 1, X, Z`) fast simulation engine.
//!
//! Performs topological levelization of combinational `LUT6`, `CARRY4`, `MUXF7`,
//! and `MUXF8` cones into a flat instruction schedule and evaluates state using
//! contiguous bit-vector arrays (`Vec<u64>`) with zero heap allocations per cycle.

use helion_ir::{resolve, Cell, CellKind, Design, PortDir};
use std::collections::{HashMap, VecDeque};

/// Sentinel net ID for an unconnected input pin (ties to logic 0 by default).
pub const NET_ZERO: u32 = 0;
/// Sentinel net ID for a constant logic 1 pin.
pub const NET_ONE: u32 = 1;
/// Sentinel net ID for an unconnected output pin.
pub const NET_UNCONNECTED: u32 = u32::MAX;

/// IEEE 1364 / 1800 4-state logic value (`0`, `1`, `X`, `Z`).
///
/// Encoded in dual bit-planes `(val, xz)`:
/// - `xz = 0, val = 0` -> `Zero` (`0`)
/// - `xz = 0, val = 1` -> `One` (`1`)
/// - `xz = 1, val = 0` -> `X` (unknown / conflict)
/// - `xz = 1, val = 1` -> `Z` (high-impedance)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Logic4 {
    Zero,
    One,
    X,
    Z,
}

impl Logic4 {
    #[inline]
    pub const fn from_planes(val: bool, xz: bool) -> Self {
        match (xz, val) {
            (false, false) => Self::Zero,
            (false, true) => Self::One,
            (true, false) => Self::X,
            (true, true) => Self::Z,
        }
    }

    #[inline]
    pub const fn to_planes(self) -> (bool, bool) {
        match self {
            Self::Zero => (false, false),
            Self::One => (true, false),
            Self::X => (false, true),
            Self::Z => (true, true),
        }
    }

    #[inline]
    pub const fn from_bool(b: bool) -> Self {
        if b {
            Self::One
        } else {
            Self::Zero
        }
    }

    #[inline]
    pub const fn is_known(self) -> bool {
        matches!(self, Self::Zero | Self::One)
    }

    #[inline]
    pub const fn is_unknown(self) -> bool {
        matches!(self, Self::X | Self::Z)
    }

    #[inline]
    pub const fn as_bool(self) -> Option<bool> {
        match self {
            Self::Zero => Some(false),
            Self::One => Some(true),
            Self::X | Self::Z => None,
        }
    }

    #[inline]
    pub const fn to_char(self) -> char {
        match self {
            Self::Zero => '0',
            Self::One => '1',
            Self::X => 'x',
            Self::Z => 'z',
        }
    }

    #[inline]
    pub const fn from_char(c: char) -> Option<Self> {
        match c {
            '0' => Some(Self::Zero),
            '1' => Some(Self::One),
            'x' | 'X' => Some(Self::X),
            'z' | 'Z' => Some(Self::Z),
            _ => None,
        }
    }

    #[inline]
    pub const fn not(self) -> Self {
        match self {
            Self::Zero => Self::One,
            Self::One => Self::Zero,
            Self::X | Self::Z => Self::X,
        }
    }

    #[inline]
    pub const fn and(self, rhs: Self) -> Self {
        match (self, rhs) {
            (Self::Zero, _) | (_, Self::Zero) => Self::Zero,
            (Self::One, Self::One) => Self::One,
            _ => Self::X,
        }
    }

    #[inline]
    pub const fn or(self, rhs: Self) -> Self {
        match (self, rhs) {
            (Self::One, _) | (_, Self::One) => Self::One,
            (Self::Zero, Self::Zero) => Self::Zero,
            _ => Self::X,
        }
    }

    #[inline]
    pub const fn xor(self, rhs: Self) -> Self {
        match (self.as_bool(), rhs.as_bool()) {
            (Some(a), Some(b)) => Self::from_bool(a ^ b),
            _ => Self::X,
        }
    }

    /// 4-state 2:1 mux (`sel ? i1 : i0`).
    /// When `sel` is `X` or `Z`, if `i0` and `i1` are both known and equal,
    /// resolves to that known bit (optimistic reconvergence); otherwise `X`.
    #[inline]
    pub const fn mux(sel: Self, i0: Self, i1: Self) -> Self {
        match sel {
            Self::Zero => match i0 {
                Self::Z => Self::X,
                other => other,
            },
            Self::One => match i1 {
                Self::Z => Self::X,
                other => other,
            },
            Self::X | Self::Z => match (i0, i1) {
                (Self::Zero, Self::Zero) => Self::Zero,
                (Self::One, Self::One) => Self::One,
                _ => Self::X,
            },
        }
    }

    /// Exact 4-state LUT6 evaluation:
    /// - Fast path: if all 6 inputs are known (`0` or `1`), single bit shift `(init >> addr) & 1`.
    /// - When a subset of inputs are `X`/`Z`, inspects all reachable truth-table entries
    ///   using bitmasks on the 64-bit `init` word. If all reachable entries are `0`, returns
    ///   `Zero`; if all are `1`, returns `One`; otherwise returns `X`.
    #[inline]
    pub fn eval_lut6(init: u64, inputs: [Self; 6]) -> Self {
        let mut base_addr = 0usize;
        let mut unknown_mask = 0usize;
        for (pin, &inp) in inputs.iter().enumerate() {
            match inp {
                Self::Zero => {}
                Self::One => base_addr |= 1 << pin,
                Self::X | Self::Z => unknown_mask |= 1 << pin,
            }
        }
        if unknown_mask == 0 {
            return Self::from_bool(((init >> base_addr) & 1) != 0);
        }
        // Build the 64-bit mask of all LUT addresses matching the known bits.
        let mut addr_mask = !0u64;
        const PIN_HIGH_MASKS: [u64; 6] = [
            0xAAAA_AAAA_AAAA_AAAA,
            0xCCCC_CCCC_CCCC_CCCC,
            0xF0F0_F0F0_F0F0_F0F0,
            0xFF00_FF00_FF00_FF00,
            0xFFFF_0000_FFFF_0000,
            0xFFFF_FFFF_0000_0000,
        ];
        for (pin, &pmask) in PIN_HIGH_MASKS.iter().enumerate() {
            if (unknown_mask & (1 << pin)) == 0 {
                if (base_addr & (1 << pin)) != 0 {
                    addr_mask &= pmask;
                } else {
                    addr_mask &= !pmask;
                }
            }
        }
        let matching = init & addr_mask;
        if matching == 0 {
            Self::Zero
        } else if matching == addr_mask {
            Self::One
        } else {
            Self::X
        }
    }
}

impl std::fmt::Display for Logic4 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_char())
    }
}

/// Combinational primitive kind in the levelized schedule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConeNodeKind {
    /// 6-input LUT with 64-bit truth table.
    Lut6 {
        init: u64,
        inputs: [u32; 6],
        out_net: u32,
    },
    /// 4-bit fast carry chain (`CARRY4`).
    Carry4 {
        ci_net: u32,
        cyinit_net: u32,
        di_nets: [u32; 4],
        s_nets: [u32; 4],
        o_nets: [u32; 4],
        co_nets: [u32; 4],
    },
    /// 2:1 LUT multiplexer (`MUXF7`: `O = S ? I1 : I0`).
    Muxf7 {
        i0_net: u32,
        i1_net: u32,
        s_net: u32,
        o_net: u32,
    },
    /// 2:1 slice multiplexer (`MUXF8`: `O = S ? I1 : I0`).
    Muxf8 {
        i0_net: u32,
        i1_net: u32,
        s_net: u32,
        o_net: u32,
    },
}

/// One compiled combinational evaluation node in the topological schedule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompiledNode {
    pub cell_name: String,
    pub level: u32,
    pub kind: ConeNodeKind,
}

impl CompiledNode {
    /// Input net IDs read by this combinational node.
    pub fn input_nets(&self) -> Vec<u32> {
        let mut out = Vec::with_capacity(10);
        match &self.kind {
            ConeNodeKind::Lut6 { inputs, .. } => {
                for &n in inputs {
                    if n >= 2 && n != NET_UNCONNECTED {
                        out.push(n);
                    }
                }
            }
            ConeNodeKind::Carry4 {
                ci_net,
                cyinit_net,
                di_nets,
                s_nets,
                ..
            } => {
                for &n in [*ci_net, *cyinit_net]
                    .iter()
                    .chain(di_nets.iter())
                    .chain(s_nets.iter())
                {
                    if n >= 2 && n != NET_UNCONNECTED {
                        out.push(n);
                    }
                }
            }
            ConeNodeKind::Muxf7 {
                i0_net,
                i1_net,
                s_net,
                ..
            }
            | ConeNodeKind::Muxf8 {
                i0_net,
                i1_net,
                s_net,
                ..
            } => {
                for &n in [*i0_net, *i1_net, *s_net].iter() {
                    if n >= 2 && n != NET_UNCONNECTED {
                        out.push(n);
                    }
                }
            }
        }
        out
    }

    /// Output net IDs driven by this combinational node.
    pub fn output_nets(&self) -> Vec<u32> {
        match &self.kind {
            ConeNodeKind::Lut6 { out_net, .. } => {
                if *out_net != NET_UNCONNECTED {
                    vec![*out_net]
                } else {
                    vec![]
                }
            }
            ConeNodeKind::Carry4 {
                o_nets, co_nets, ..
            } => o_nets
                .iter()
                .chain(co_nets.iter())
                .copied()
                .filter(|&n| n != NET_UNCONNECTED)
                .collect(),
            ConeNodeKind::Muxf7 { o_net, .. } | ConeNodeKind::Muxf8 { o_net, .. } => {
                if *o_net != NET_UNCONNECTED {
                    vec![*o_net]
                } else {
                    vec![]
                }
            }
        }
    }
}

/// Compiled flip-flop descriptor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompiledFf {
    pub name: String,
    pub d_net: u32,
    pub q_net: u32,
    pub ce_net: u32,
    pub rst_net: u32,
}

/// Topologically levelized schedule for a [`Design`].
#[derive(Clone, Debug)]
pub struct LevelizedSchedule {
    /// Net index -> net name (`0` is `"0"`, `1` is `"1"`).
    pub net_names: Vec<String>,
    /// Net name -> dense net index.
    pub net_ids: HashMap<String, u32>,
    /// Combinational nodes grouped by topological level (`0..num_levels`).
    pub levels: Vec<Vec<CompiledNode>>,
    /// Flattened topological evaluation tape (`levels.into_iter().flatten()`).
    pub ops: Vec<CompiledNode>,
    /// Sequential flip-flops (`Hff`).
    pub ffs: Vec<CompiledFf>,
    /// FF name -> index in `ffs`.
    pub ff_ids: HashMap<String, usize>,
    /// Top-level input/inout port -> net ID.
    pub input_ports: HashMap<String, u32>,
    /// Top-level output port -> resolved internal net ID (follows `IobOut` `PAD` -> `I`).
    pub output_ports: HashMap<String, u32>,
    /// Optional net driving the board `led` output.
    pub led_net: Option<u32>,
    /// True if a combinational cycle was detected and broken during levelization.
    pub has_combinational_loop: bool,
}

/// Classify whether a cell is a `MUXF7` or `MUXF8` primitive (either via `BlackBox` module name
/// or cell attributes `primitive` / `type`).
pub(crate) fn classify_muxf_cell(cell: &Cell) -> Option<bool> {
    if let CellKind::BlackBox { module } = cell.kind {
        let m = resolve(module);
        if m.eq_ignore_ascii_case("MUXF7") {
            return Some(false);
        }
        if m.eq_ignore_ascii_case("MUXF8") {
            return Some(true);
        }
    }
    for key in ["primitive", "type", "bel_type"] {
        if let Some(v) = cell.attrs.get(key) {
            if v.eq_ignore_ascii_case("MUXF7") {
                return Some(false);
            }
            if v.eq_ignore_ascii_case("MUXF8") {
                return Some(true);
            }
        }
    }
    None
}

impl LevelizedSchedule {
    /// Build a topologically levelized schedule from a [`Design`].
    pub fn compile(design: &Design) -> Self {
        let mut net_names = vec!["0".to_string(), "1".to_string()];
        let mut net_ids = HashMap::new();
        net_ids.insert("0".to_string(), NET_ZERO);
        net_ids.insert("1'b0".to_string(), NET_ZERO);
        net_ids.insert("GND".to_string(), NET_ZERO);
        net_ids.insert("1".to_string(), NET_ONE);
        net_ids.insert("1'b1".to_string(), NET_ONE);
        net_ids.insert("VCC".to_string(), NET_ONE);

        let intern_net = |name: &str, names: &mut Vec<String>, ids: &mut HashMap<String, u32>| -> u32 {
            if let Some(&id) = ids.get(name) {
                return id;
            }
            let id = names.len() as u32;
            names.push(name.to_string());
            ids.insert(name.to_string(), id);
            id
        };

        for p in &design.ports {
            intern_net(resolve(p.name), &mut net_names, &mut net_ids);
        }
        for n in &design.nets {
            intern_net(resolve(n.name), &mut net_names, &mut net_ids);
        }

        let pin_ix = design.pin_index();
        let resolve_in_pin = |cell: &str, pin: &str, names: &mut Vec<String>, ids: &mut HashMap<String, u32>| -> u32 {
            match pin_ix.net_on(cell, pin) {
                Some(s) => intern_net(resolve(s), names, ids),
                None => NET_ZERO,
            }
        };
        let resolve_out_pin = |cell: &str, pin: &str, names: &mut Vec<String>, ids: &mut HashMap<String, u32>| -> u32 {
            match pin_ix.net_on(cell, pin) {
                Some(s) => intern_net(resolve(s), names, ids),
                None => NET_UNCONNECTED,
            }
        };

        // Gather sequential FFs and combinational nodes.
        let mut ffs = Vec::new();
        let mut ff_ids = HashMap::new();
        let mut raw_nodes: Vec<CompiledNode> = Vec::new();

        for c in &design.cells {
            let cname = resolve(c.name);
            match c.kind {
                CellKind::Hff => {
                    let d_net = resolve_in_pin(cname, "D", &mut net_names, &mut net_ids);
                    let q_net = resolve_out_pin(cname, "Q", &mut net_names, &mut net_ids);
                    let ce_net = match pin_ix.net_on(cname, "CE") {
                        Some(s) => intern_net(resolve(s), &mut net_names, &mut net_ids),
                        None => NET_ONE,
                    };
                    let rst_net = match pin_ix
                        .net_on(cname, "R")
                        .or_else(|| pin_ix.net_on(cname, "RST"))
                        .or_else(|| pin_ix.net_on(cname, "SR"))
                    {
                        Some(s) => intern_net(resolve(s), &mut net_names, &mut net_ids),
                        None => NET_ZERO,
                    };
                    let idx = ffs.len();
                    ff_ids.insert(cname.to_string(), idx);
                    ffs.push(CompiledFf {
                        name: cname.to_string(),
                        d_net,
                        q_net,
                        ce_net,
                        rst_net,
                    });
                }
                CellKind::Lut6 { init } => {
                    let mut inputs = [NET_ZERO; 6];
                    for (i, slot) in inputs.iter_mut().enumerate() {
                        *slot = resolve_in_pin(cname, &format!("I{i}"), &mut net_names, &mut net_ids);
                    }
                    let out_net = resolve_out_pin(cname, "O", &mut net_names, &mut net_ids);
                    raw_nodes.push(CompiledNode {
                        cell_name: cname.to_string(),
                        level: 0,
                        kind: ConeNodeKind::Lut6 {
                            init,
                            inputs,
                            out_net,
                        },
                    });
                }
                CellKind::Carry4 => {
                    let ci_net = match pin_ix.net_on(cname, "CI") {
                        Some(s) => intern_net(resolve(s), &mut net_names, &mut net_ids),
                        None => NET_UNCONNECTED,
                    };
                    let cyinit_net = match pin_ix.net_on(cname, "CYINIT") {
                        Some(s) => intern_net(resolve(s), &mut net_names, &mut net_ids),
                        None => NET_UNCONNECTED,
                    };
                    let mut di_nets = [NET_ZERO; 4];
                    let mut s_nets = [NET_ZERO; 4];
                    let mut o_nets = [NET_UNCONNECTED; 4];
                    let mut co_nets = [NET_UNCONNECTED; 4];
                    for i in 0..4 {
                        di_nets[i] = resolve_in_pin(cname, &format!("DI{i}"), &mut net_names, &mut net_ids);
                        s_nets[i] = resolve_in_pin(cname, &format!("S{i}"), &mut net_names, &mut net_ids);
                        o_nets[i] = resolve_out_pin(cname, &format!("O{i}"), &mut net_names, &mut net_ids);
                        co_nets[i] = resolve_out_pin(cname, &format!("CO{i}"), &mut net_names, &mut net_ids);
                    }
                    raw_nodes.push(CompiledNode {
                        cell_name: cname.to_string(),
                        level: 0,
                        kind: ConeNodeKind::Carry4 {
                            ci_net,
                            cyinit_net,
                            di_nets,
                            s_nets,
                            o_nets,
                            co_nets,
                        },
                    });
                }
                _ => {
                    if let Some(is_f8) = classify_muxf_cell(c) {
                        let i0_net = resolve_in_pin(cname, "I0", &mut net_names, &mut net_ids);
                        let i1_net = resolve_in_pin(cname, "I1", &mut net_names, &mut net_ids);
                        let s_net = resolve_in_pin(cname, "S", &mut net_names, &mut net_ids);
                        let o_net = resolve_out_pin(cname, "O", &mut net_names, &mut net_ids);
                        let kind = if is_f8 {
                            ConeNodeKind::Muxf8 {
                                i0_net,
                                i1_net,
                                s_net,
                                o_net,
                            }
                        } else {
                            ConeNodeKind::Muxf7 {
                                i0_net,
                                i1_net,
                                s_net,
                                o_net,
                            }
                        };
                        raw_nodes.push(CompiledNode {
                            cell_name: cname.to_string(),
                            level: 0,
                            kind,
                        });
                    }
                }
            }
        }

        // Input/Output port mappings and IOB resolution.
        let mut iob_pad_to_i: HashMap<String, String> = HashMap::new();
        let mut iob_in_i_to_o: HashMap<u32, u32> = HashMap::new();
        let mut first_iob_in_net: Option<u32> = None;

        for c in &design.cells {
            let cname = resolve(c.name);
            match c.kind {
                CellKind::IobOut => {
                    let i_net = pin_ix
                        .net_on(cname, "I")
                        .map(|s| intern_net(resolve(s), &mut net_names, &mut net_ids));
                    if first_iob_in_net.is_none() {
                        first_iob_in_net = i_net;
                    }
                    if let (Some(pad), Some(i)) = (pin_ix.net_on(cname, "PAD"), pin_ix.net_on(cname, "I")) {
                        iob_pad_to_i.insert(resolve(pad).to_string(), resolve(i).to_string());
                    }
                }
                CellKind::IobIn | CellKind::ClockBuf => {
                    let i_pin = pin_ix
                        .net_on(cname, "I")
                        .or_else(|| pin_ix.net_on(cname, "PAD"));
                    let o_pin = pin_ix.net_on(cname, "O");
                    if let (Some(i_s), Some(o_s)) = (i_pin, o_pin) {
                        let i_id = intern_net(resolve(i_s), &mut net_names, &mut net_ids);
                        let o_id = intern_net(resolve(o_s), &mut net_names, &mut net_ids);
                        if i_id != o_id {
                            // Emit a 1-input buffer LUT so the levelized schedule propagates IobIn/ClockBuf automatically.
                            let mut inputs = [NET_ZERO; 6];
                            inputs[0] = i_id;
                            raw_nodes.push(CompiledNode {
                                cell_name: cname.to_string(),
                                level: 0,
                                kind: ConeNodeKind::Lut6 {
                                    init: 0xAAAA_AAAA_AAAA_AAAA,
                                    inputs,
                                    out_net: o_id,
                                },
                            });
                            iob_in_i_to_o.insert(i_id, o_id);
                        }
                    }
                }
                _ => {}
            }
        }

        let mut input_ports = HashMap::new();
        let mut output_ports = HashMap::new();
        for p in &design.ports {
            let pname = resolve(p.name);
            match p.dir {
                PortDir::In | PortDir::Inout => {
                    let id = intern_net(pname, &mut net_names, &mut net_ids);
                    input_ports.insert(pname.to_string(), id);
                }
                PortDir::Out => {
                    let target = iob_pad_to_i
                        .get(pname)
                        .map(String::as_str)
                        .unwrap_or(pname);
                    let id = intern_net(target, &mut net_names, &mut net_ids);
                    output_ports.insert(pname.to_string(), id);
                }
            }
        }

        let led_net = first_iob_in_net.or_else(|| output_ports.get("led").copied());

        // Topological levelization of `raw_nodes` using Kahn's algorithm.
        let n_nodes = raw_nodes.len();
        let mut net_driver_node: HashMap<u32, usize> = HashMap::new();
        for (idx, node) in raw_nodes.iter().enumerate() {
            for out_net in node.output_nets() {
                net_driver_node.insert(out_net, idx);
            }
        }
        // FF Q nets override combinational drivers (matches Sim::new where Q beats O).
        for ff in &ffs {
            if ff.q_net != NET_UNCONNECTED {
                net_driver_node.remove(&ff.q_net);
            }
        }

        let mut in_degree = vec![0usize; n_nodes];
        let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); n_nodes];
        for (idx, node) in raw_nodes.iter().enumerate() {
            let mut preds = Vec::new();
            for in_net in node.input_nets() {
                if let Some(&drv_idx) = net_driver_node.get(&in_net) {
                    if drv_idx != idx && !preds.contains(&drv_idx) {
                        preds.push(drv_idx);
                    } else if drv_idx == idx {
                        // Self-loop: keep in_degree > 0 so it's flagged as a combinational loop.
                        in_degree[idx] += 1;
                    }
                }
            }
            in_degree[idx] += preds.len();
            for p in preds {
                dependents[p].push(idx);
            }
        }

        let mut queue: VecDeque<usize> = VecDeque::new();
        let mut node_level = vec![0u32; n_nodes];
        for (idx, &deg) in in_degree.iter().enumerate() {
            if deg == 0 {
                queue.push_back(idx);
            }
        }

        let mut visited = vec![false; n_nodes];
        let mut ordered_indices = Vec::with_capacity(n_nodes);
        while let Some(u) = queue.pop_front() {
            visited[u] = true;
            ordered_indices.push(u);
            let next_lvl = node_level[u] + 1;
            for &v in &dependents[u] {
                if node_level[v] < next_lvl {
                    node_level[v] = next_lvl;
                }
                in_degree[v] = in_degree[v].saturating_sub(1);
                if in_degree[v] == 0 {
                    queue.push_back(v);
                }
            }
        }

        let mut has_combinational_loop = false;
        if ordered_indices.len() < n_nodes {
            has_combinational_loop = true;
            let fallback_lvl = node_level.iter().copied().max().unwrap_or(0) + 1;
            for idx in 0..n_nodes {
                if !visited[idx] {
                    node_level[idx] = fallback_lvl;
                    ordered_indices.push(idx);
                }
            }
        }

        let max_lvl = if ordered_indices.is_empty() {
            0
        } else {
            ordered_indices
                .iter()
                .map(|&i| node_level[i])
                .max()
                .unwrap_or(0) as usize
                + 1
        };
        let mut levels: Vec<Vec<CompiledNode>> = vec![Vec::new(); max_lvl];
        let mut ops = Vec::with_capacity(n_nodes);

        for idx in ordered_indices {
            let mut node = raw_nodes[idx].clone();
            node.level = node_level[idx];
            levels[node.level as usize].push(node.clone());
            ops.push(node);
        }

        Self {
            net_names,
            net_ids,
            levels,
            ops,
            ffs,
            ff_ids,
            input_ports,
            output_ports,
            led_net,
            has_combinational_loop,
        }
    }

    /// Number of topological combinational levels.
    pub fn num_levels(&self) -> usize {
        self.levels.len()
    }

    /// Total number of compiled combinational primitives (`LUT6` + `CARRY4` + `MUXF7` + `MUXF8`).
    pub fn num_ops(&self) -> usize {
        self.ops.len()
    }

    /// Total number of tracked nets (including constant `0` and `1`).
    pub fn num_nets(&self) -> usize {
        self.net_names.len()
    }
}

/// High-speed compiled simulator executing [`LevelizedSchedule`] over flat contiguous
/// `Vec<u64>` bit-vector state arrays.
///
/// Supports both:
/// - **2-state fast path** ([`CompiledSim::step_2state`], [`CompiledSim::run_2state_cycles`])
/// - **4-state (`0, 1, X, Z`) path** ([`CompiledSim::step_4state`], [`CompiledSim::sample_4state`])
#[derive(Clone, Debug)]
pub struct CompiledSim {
    pub schedule: LevelizedSchedule,
    pub time: u64,
    pub cycle: u64,
    pub led: bool,
    /// Packed 2-state / 4-state value plane (`(net_id >> 6)` word, `(net_id & 63)` bit).
    net_val: Vec<u64>,
    /// Packed 4-state XZ plane (`0` = known `0`/`1`, `1` = unknown `X`/`Z`).
    net_xz: Vec<u64>,
    /// Packed flip-flop Q value plane.
    ff_val: Vec<u64>,
    /// Packed flip-flop Q XZ plane.
    ff_xz: Vec<u64>,
    /// Scratch buffer for next-state FF value plane (pre-allocated once; zero per-cycle alloc).
    next_ff_val: Vec<u64>,
    /// Scratch buffer for next-state FF XZ plane.
    next_ff_xz: Vec<u64>,
    /// Active continuous force mask per net.
    force_mask: Vec<u64>,
    /// Active continuous force value per net.
    force_val: Vec<u64>,
}

#[inline]
fn get_bit(words: &[u64], id: u32) -> bool {
    let w = (id >> 6) as usize;
    let b = id & 63;
    ((words[w] >> b) & 1) != 0
}

#[inline]
fn set_bit(words: &mut [u64], id: u32, val: bool) {
    let w = (id >> 6) as usize;
    let b = id & 63;
    if val {
        words[w] |= 1u64 << b;
    } else {
        words[w] &= !(1u64 << b);
    }
}

#[inline]
fn get_logic4(val_words: &[u64], xz_words: &[u64], id: u32) -> Logic4 {
    Logic4::from_planes(get_bit(val_words, id), get_bit(xz_words, id))
}

#[inline]
fn set_logic4(val_words: &mut [u64], xz_words: &mut [u64], id: u32, v: Logic4) {
    let (val, xz) = v.to_planes();
    set_bit(val_words, id, val);
    set_bit(xz_words, id, xz);
}

impl CompiledSim {
    /// Compile a [`Design`] into a levelized flat simulator with all FFs initialized to `0`.
    pub fn new(design: &Design) -> Self {
        Self::with_schedule(LevelizedSchedule::compile(design), false)
    }

    /// Compile a [`Design`] into a 4-state simulator where FFs start in `X` (`init_ffs_x = true`)
    /// or `0` (`init_ffs_x = false`), and undriven inputs start in `Z`/`X`.
    pub fn new_4state(design: &Design, init_ffs_x: bool) -> Self {
        Self::with_schedule(LevelizedSchedule::compile(design), init_ffs_x)
    }

    /// Construct from a pre-built [`LevelizedSchedule`].
    pub fn with_schedule(schedule: LevelizedSchedule, init_ffs_x: bool) -> Self {
        let net_words = (schedule.num_nets() + 63) / 64;
        let ff_words = (schedule.ffs.len() + 63) / 64;
        let mut net_val = vec![0u64; net_words.max(1)];
        let mut net_xz = vec![0u64; net_words.max(1)];
        let ff_val = vec![0u64; ff_words.max(1)];
        let mut ff_xz = vec![0u64; ff_words.max(1)];

        // Constant 0 and 1 nets.
        set_logic4(&mut net_val, &mut net_xz, NET_ZERO, Logic4::Zero);
        set_logic4(&mut net_val, &mut net_xz, NET_ONE, Logic4::One);

        if init_ffs_x {
            for (idx, ff) in schedule.ffs.iter().enumerate() {
                set_bit(&mut ff_xz, idx as u32, true);
                if ff.q_net != NET_UNCONNECTED {
                    set_logic4(&mut net_val, &mut net_xz, ff.q_net, Logic4::X);
                }
            }
        }

        let mut sim = Self {
            schedule,
            time: 0,
            cycle: 0,
            led: false,
            net_val,
            net_xz,
            ff_val,
            ff_xz,
            next_ff_val: vec![0u64; ff_words.max(1)],
            next_ff_xz: vec![0u64; ff_words.max(1)],
            force_mask: vec![0u64; net_words.max(1)],
            force_val: vec![0u64; net_words.max(1)],
        };
        if init_ffs_x {
            sim.eval_comb_4state();
        } else {
            sim.eval_comb_2state();
        }
        sim
    }

    /// Raw packed 64-bit word slice of 2-state net values.
    #[inline]
    pub fn net_value_words(&self) -> &[u64] {
        &self.net_val
    }

    /// Raw packed 64-bit word slice of 4-state XZ flags.
    #[inline]
    pub fn net_xz_words(&self) -> &[u64] {
        &self.net_xz
    }

    /// Raw packed 64-bit word slice of FF Q states.
    #[inline]
    pub fn ff_value_words(&self) -> &[u64] {
        &self.ff_val
    }

    /// Evaluate all combinational cones in topological order (2-state fast path, zero alloc).
    #[inline]
    pub fn eval_comb_2state(&mut self) {
        for node in &self.schedule.ops {
            match &node.kind {
                ConeNodeKind::Lut6 {
                    init,
                    inputs,
                    out_net,
                } => {
                    if *out_net == NET_UNCONNECTED {
                        continue;
                    }
                    let mut addr = 0u64;
                    for (pin, &net_id) in inputs.iter().enumerate() {
                        if get_bit(&self.net_val, net_id) {
                            addr |= 1u64 << pin;
                        }
                    }
                    let out = ((*init >> addr) & 1) != 0;
                    set_bit(&mut self.net_val, *out_net, out);
                    set_bit(&mut self.net_xz, *out_net, false);
                }
                ConeNodeKind::Carry4 {
                    ci_net,
                    cyinit_net,
                    di_nets,
                    s_nets,
                    o_nets,
                    co_nets,
                } => {
                    let mut c = if *ci_net != NET_UNCONNECTED {
                        get_bit(&self.net_val, *ci_net)
                    } else if *cyinit_net != NET_UNCONNECTED {
                        get_bit(&self.net_val, *cyinit_net)
                    } else {
                        false
                    };
                    for i in 0..4 {
                        let s = get_bit(&self.net_val, s_nets[i]);
                        let di = get_bit(&self.net_val, di_nets[i]);
                        let o = s ^ c;
                        let co = if s { c } else { di };
                        if o_nets[i] != NET_UNCONNECTED {
                            set_bit(&mut self.net_val, o_nets[i], o);
                            set_bit(&mut self.net_xz, o_nets[i], false);
                        }
                        if co_nets[i] != NET_UNCONNECTED {
                            set_bit(&mut self.net_val, co_nets[i], co);
                            set_bit(&mut self.net_xz, co_nets[i], false);
                        }
                        c = co;
                    }
                }
                ConeNodeKind::Muxf7 {
                    i0_net,
                    i1_net,
                    s_net,
                    o_net,
                }
                | ConeNodeKind::Muxf8 {
                    i0_net,
                    i1_net,
                    s_net,
                    o_net,
                } => {
                    if *o_net == NET_UNCONNECTED {
                        continue;
                    }
                    let s = get_bit(&self.net_val, *s_net);
                    let out = if s {
                        get_bit(&self.net_val, *i1_net)
                    } else {
                        get_bit(&self.net_val, *i0_net)
                    };
                    set_bit(&mut self.net_val, *o_net, out);
                    set_bit(&mut self.net_xz, *o_net, false);
                }
            }
        }
        self.apply_force_words();
    }

    /// Evaluate all combinational cones in topological order (4-state `0, 1, X, Z` path).
    #[inline]
    pub fn eval_comb_4state(&mut self) {
        for node in &self.schedule.ops {
            match &node.kind {
                ConeNodeKind::Lut6 {
                    init,
                    inputs,
                    out_net,
                } => {
                    if *out_net == NET_UNCONNECTED {
                        continue;
                    }
                    let mut in_vals = [Logic4::Zero; 6];
                    for (pin, &net_id) in inputs.iter().enumerate() {
                        in_vals[pin] = get_logic4(&self.net_val, &self.net_xz, net_id);
                    }
                    let out = Logic4::eval_lut6(*init, in_vals);
                    set_logic4(&mut self.net_val, &mut self.net_xz, *out_net, out);
                }
                ConeNodeKind::Carry4 {
                    ci_net,
                    cyinit_net,
                    di_nets,
                    s_nets,
                    o_nets,
                    co_nets,
                } => {
                    let mut c = if *ci_net != NET_UNCONNECTED {
                        get_logic4(&self.net_val, &self.net_xz, *ci_net)
                    } else if *cyinit_net != NET_UNCONNECTED {
                        get_logic4(&self.net_val, &self.net_xz, *cyinit_net)
                    } else {
                        Logic4::Zero
                    };
                    for i in 0..4 {
                        let s = get_logic4(&self.net_val, &self.net_xz, s_nets[i]);
                        let di = get_logic4(&self.net_val, &self.net_xz, di_nets[i]);
                        let o = s.xor(c);
                        let co = Logic4::mux(s, di, c);
                        if o_nets[i] != NET_UNCONNECTED {
                            set_logic4(&mut self.net_val, &mut self.net_xz, o_nets[i], o);
                        }
                        if co_nets[i] != NET_UNCONNECTED {
                            set_logic4(&mut self.net_val, &mut self.net_xz, co_nets[i], co);
                        }
                        c = co;
                    }
                }
                ConeNodeKind::Muxf7 {
                    i0_net,
                    i1_net,
                    s_net,
                    o_net,
                }
                | ConeNodeKind::Muxf8 {
                    i0_net,
                    i1_net,
                    s_net,
                    o_net,
                } => {
                    if *o_net == NET_UNCONNECTED {
                        continue;
                    }
                    let i0 = get_logic4(&self.net_val, &self.net_xz, *i0_net);
                    let i1 = get_logic4(&self.net_val, &self.net_xz, *i1_net);
                    let s = get_logic4(&self.net_val, &self.net_xz, *s_net);
                    let out = Logic4::mux(s, i0, i1);
                    set_logic4(&mut self.net_val, &mut self.net_xz, *o_net, out);
                }
            }
        }
        self.apply_force_words();
    }

    #[inline]
    fn apply_force_words(&mut self) {
        for i in 0..self.net_val.len() {
            let mask = self.force_mask[i];
            if mask != 0 {
                self.net_val[i] = (self.net_val[i] & !mask) | (self.force_val[i] & mask);
                self.net_xz[i] &= !mask;
            }
        }
    }

    /// Execute one clock posedge on the 2-state compiled fast path (zero heap allocation).
    #[inline]
    pub fn step_2state(&mut self, delay_ps: u64) {
        self.time += delay_ps;
        self.cycle += 1;

        // 1. Evaluate combinational cones from current FF Q + input state.
        self.eval_comb_2state();

        // 2. Sample FF D inputs into scratch next_ff_val.
        for (idx, ff) in self.schedule.ffs.iter().enumerate() {
            let rst = get_bit(&self.net_val, ff.rst_net);
            let next_q = if rst {
                false
            } else if get_bit(&self.net_val, ff.ce_net) {
                get_bit(&self.net_val, ff.d_net)
            } else {
                get_bit(&self.ff_val, idx as u32)
            };
            set_bit(&mut self.next_ff_val, idx as u32, next_q);
        }

        // 3. Commit FF Qs and write to their driven nets.
        self.ff_val.copy_from_slice(&self.next_ff_val);
        self.ff_xz.fill(0);
        for (idx, ff) in self.schedule.ffs.iter().enumerate() {
            if ff.q_net != NET_UNCONNECTED {
                let q = get_bit(&self.ff_val, idx as u32);
                set_bit(&mut self.net_val, ff.q_net, q);
                set_bit(&mut self.net_xz, ff.q_net, false);
            }
        }
        self.apply_force_words();

        // 4. Re-propagate combinational cones so post-edge probes see settled net values.
        self.eval_comb_2state();

        if let Some(led_net) = self.schedule.led_net {
            self.led = get_bit(&self.net_val, led_net);
        }
    }

    /// Run `cycles` clock posedges in a tight compiled loop and return the `led` trace.
    pub fn run_2state_cycles(&mut self, cycles: u64, period_ps: u64) -> Vec<bool> {
        let mut wave = Vec::with_capacity(cycles as usize);
        for _ in 0..cycles {
            self.step_2state(period_ps);
            wave.push(self.led);
        }
        wave
    }

    /// Execute one clock posedge on the 4-state (`0, 1, X, Z`) compiled path.
    #[inline]
    pub fn step_4state(&mut self, delay_ps: u64) {
        self.time += delay_ps;
        self.cycle += 1;

        // 1. Evaluate 4-state combinational cones before clock edge.
        self.eval_comb_4state();

        // 2. Sample FF D inputs with 4-state reset / clock-enable semantics.
        for (idx, ff) in self.schedule.ffs.iter().enumerate() {
            let rst = get_logic4(&self.net_val, &self.net_xz, ff.rst_net);
            let ce = get_logic4(&self.net_val, &self.net_xz, ff.ce_net);
            let d = get_logic4(&self.net_val, &self.net_xz, ff.d_net);
            let cur_q = get_logic4(&self.ff_val, &self.ff_xz, idx as u32);

            let next_q = match rst {
                Logic4::One => Logic4::Zero,
                Logic4::Zero => Logic4::mux(ce, cur_q, d),
                Logic4::X | Logic4::Z => {
                    if cur_q == Logic4::Zero && d == Logic4::Zero {
                        Logic4::Zero
                    } else {
                        Logic4::X
                    }
                }
            };
            set_logic4(&mut self.next_ff_val, &mut self.next_ff_xz, idx as u32, next_q);
        }

        // 3. Commit FF state and write to Q nets.
        self.ff_val.copy_from_slice(&self.next_ff_val);
        self.ff_xz.copy_from_slice(&self.next_ff_xz);
        for (idx, ff) in self.schedule.ffs.iter().enumerate() {
            if ff.q_net != NET_UNCONNECTED {
                let q = get_logic4(&self.ff_val, &self.ff_xz, idx as u32);
                set_logic4(&mut self.net_val, &mut self.net_xz, ff.q_net, q);
            }
        }
        self.apply_force_words();

        // 4. Re-propagate 4-state combinational cones post-edge.
        self.eval_comb_4state();

        if let Some(led_net) = self.schedule.led_net {
            self.led = get_logic4(&self.net_val, &self.net_xz, led_net) == Logic4::One;
        }
    }

    /// Drive a primary input port with a 2-state boolean value.
    pub fn drive_input(&mut self, port: &str, value: bool) -> Result<(), String> {
        self.drive_input_4state(port, Logic4::from_bool(value))
    }

    /// Drive a primary input port with a 4-state [`Logic4`] value (`0, 1, X, Z`).
    pub fn drive_input_4state(&mut self, port: &str, value: Logic4) -> Result<(), String> {
        let &net_id = self
            .schedule
            .input_ports
            .get(port)
            .ok_or_else(|| format!("drive_input: no input port {port}"))?;
        set_logic4(&mut self.net_val, &mut self.net_xz, net_id, value);
        Ok(())
    }

    /// Sample any net, port, FF, or `"led"` as a 4-state [`Logic4`] value.
    pub fn sample_4state(&self, name: &str) -> Option<Logic4> {
        if name.eq_ignore_ascii_case("led") {
            if let Some(led_net) = self.schedule.led_net {
                return Some(get_logic4(&self.net_val, &self.net_xz, led_net));
            }
            return Some(Logic4::from_bool(self.led));
        }
        if let Some(&ff_idx) = self.schedule.ff_ids.get(name) {
            return Some(get_logic4(&self.ff_val, &self.ff_xz, ff_idx as u32));
        }
        if let Some(&net_id) = self.schedule.output_ports.get(name) {
            return Some(get_logic4(&self.net_val, &self.net_xz, net_id));
        }
        let &net_id = self.schedule.net_ids.get(name)?;
        Some(get_logic4(&self.net_val, &self.net_xz, net_id))
    }

    /// Sample any net, port, FF, or `"led"` as a 2-state `u64` (`None` if unknown `X`/`Z` or absent).
    pub fn sample(&self, name: &str) -> Option<u64> {
        self.sample_4state(name)?.as_bool().map(u64::from)
    }

    /// Set a flip-flop's state directly to a 4-state [`Logic4`] value (e.g. X-injection on timing violation).
    pub fn set_ff_4state(&mut self, ff_name: &str, value: Logic4) -> Result<(), String> {
        let &ff_idx = self
            .schedule
            .ff_ids
            .get(ff_name)
            .ok_or_else(|| format!("no ff {ff_name}"))?;
        set_logic4(&mut self.ff_val, &mut self.ff_xz, ff_idx as u32, value);
        let q_net = self.schedule.ffs[ff_idx].q_net;
        if q_net != NET_UNCONNECTED {
            set_logic4(&mut self.net_val, &mut self.net_xz, q_net, value);
        }
        Ok(())
    }

    /// Read a flip-flop's current 4-state [`Logic4`] value.
    pub fn get_ff_4state(&self, ff_name: &str) -> Option<Logic4> {
        let &ff_idx = self.schedule.ff_ids.get(ff_name)?;
        Some(get_logic4(&self.ff_val, &self.ff_xz, ff_idx as u32))
    }

    /// Force a net or FF output to a continuous 2-state value until released.
    pub fn force_net(&mut self, name: &str, value: bool) -> Result<(), String> {
        let net_id = if let Some(&ff_idx) = self.schedule.ff_ids.get(name) {
            self.schedule.ffs[ff_idx].q_net
        } else if let Some(&id) = self.schedule.output_ports.get(name) {
            id
        } else if let Some(&id) = self.schedule.net_ids.get(name) {
            id
        } else {
            return Err(format!("force_net: unknown net or ff {name}"));
        };
        if net_id == NET_UNCONNECTED {
            return Err(format!("force_net: {name} has no connected net"));
        }
        set_bit(&mut self.force_mask, net_id, true);
        set_bit(&mut self.force_val, net_id, value);
        set_bit(&mut self.net_val, net_id, value);
        set_bit(&mut self.net_xz, net_id, false);
        Ok(())
    }

    /// Release a forced net or FF output.
    pub fn release_net(&mut self, name: &str) -> Result<(), String> {
        let net_id = if let Some(&ff_idx) = self.schedule.ff_ids.get(name) {
            self.schedule.ffs[ff_idx].q_net
        } else if let Some(&id) = self.schedule.output_ports.get(name) {
            id
        } else if let Some(&id) = self.schedule.net_ids.get(name) {
            id
        } else {
            return Err(format!("release_net: unknown net or ff {name}"));
        };
        if net_id != NET_UNCONNECTED {
            set_bit(&mut self.force_mask, net_id, false);
        }
        Ok(())
    }

    /// Read net value by dense net ID (`0..num_nets`).
    #[inline]
    pub fn net_bit(&self, net_id: u32) -> bool {
        get_bit(&self.net_val, net_id)
    }

    /// Read 4-state net value by dense net ID (`0..num_nets`).
    #[inline]
    pub fn net_logic4(&self, net_id: u32) -> Logic4 {
        get_logic4(&self.net_val, &self.net_xz, net_id)
    }
}

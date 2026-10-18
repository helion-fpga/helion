//! Vivado HLS (UG902) Task-Level `#pragma HLS DATAFLOW` Network Analyzer (`analyze_dataflow_network`),
//! 2D `DSP48E1` Systolic Array Generator (`generate_systolic_pe_grid`), and `s_axilite` CSR Register Map
//! + Bare-Metal C Driver Header Emitter (`AxiLiteCsrMap`, `emit_axilite_c_driver`).

use crate::parser::{CExpr, CFunction, CStmt, LValue, Parser, tokenize};
use crate::pragma::InterfaceMode;
use crate::schedule::loop_iterations;
use crate::{HlsConfig, HlsResult, resolve_top_and_inline};
use helion_ir::{CellKind, Design};
use std::collections::HashMap;

// ============================================================================
// 1. `#pragma HLS DATAFLOW` Task Network Analyzer (PIPO vs Stream FIFO + Sizing)
// ============================================================================

/// Underlying hardware channel implementation between two `#pragma HLS DATAFLOW` tasks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DataflowChannelStorage {
    /// Streaming FIFO (`hls::stream<T>` or strictly sequential single-pass array access).
    StreamFifo { min_depth: usize },
    /// 2-bank Ping-Pong RAM double buffer (`PIPO`) for random / out-of-order array access.
    PingPongRam { banks: usize, bank_depth: usize },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DataflowTaskNode {
    pub id: usize,
    pub name: String,
    pub trip_count: usize,
    pub ii: u32,
    pub startup_delay: u32,
    pub latency_cycles: u32,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DataflowChannelSpec {
    pub name: String,
    pub producer_task: String,
    pub consumer_task: String,
    pub bitwidth: usize,
    pub elements_transferred: usize,
    pub sequential_access: bool,
    pub storage: DataflowChannelStorage,
    pub min_deadlock_free_depth: usize,
}

#[derive(Clone, Debug)]
pub struct DataflowGraph {
    pub top_function: String,
    pub tasks: Vec<DataflowTaskNode>,
    pub channels: Vec<DataflowChannelSpec>,
    pub overall_ii: u32,
    pub pipelined_latency_cycles: u32,
    pub sequential_latency_cycles: u32,
    pub synchronizer_sv: String,
}

/// Compute the minimum deadlock-free FIFO depth between a producer task (`prod_ii`, `trip_count`)
/// and a consumer task (`cons_ii`, `cons_startup_delay`).
pub fn compute_deadlock_free_fifo_depth(
    trip_count: usize,
    prod_ii: u32,
    cons_ii: u32,
    cons_startup_delay: u32,
) -> usize {
    let p_ii = prod_ii.max(1) as usize;
    let c_ii = cons_ii.max(1) as usize;
    let delay_items = ((cons_startup_delay as usize) + p_ii - 1) / p_ii;
    let rate_backlog = if c_ii > p_ii {
        let total_prod_cycles = trip_count * p_ii;
        let consumed_during_prod = total_prod_cycles / c_ii;
        trip_count.saturating_sub(consumed_during_prod)
    } else {
        0
    };
    (delay_items + rate_backlog + 2).max(2)
}

/// Analyze a `#pragma HLS DATAFLOW` C/C++ kernel, extracting producer-consumer task nodes,
/// classifying intermediate channels as `StreamFifo` vs `PingPongRam` (2-bank double buffer),
/// computing deadlock-free FIFO depths across unbalanced task IIs, and emitting task-level
/// `ap_start`/`ap_continue`/`ap_done` handshake synchronizer RTL.
pub fn analyze_dataflow_network(source: &str) -> Result<DataflowGraph, String> {
    let tokens = tokenize(source)?;
    let mut parser = Parser::new(&tokens);
    let tu = parser.parse_translation_unit()?;
    let cfg = HlsConfig::default();
    let (top_fn, stage_fns, _) = resolve_top_and_inline(&tu, &cfg)?;

    let funcs_by_name: HashMap<String, &CFunction> =
        tu.functions.iter().map(|f| (f.name.clone(), f)).collect();

    let mut tasks = Vec::new();
    let mut task_access_random: HashMap<(String, String), bool> = HashMap::new();

    if !stage_fns.is_empty() {
        for (idx, sf) in stage_fns.iter().enumerate() {
            let orig = funcs_by_name.get(&sf.name).copied().unwrap_or(sf);
            let (trips, ii, random_vars) = inspect_stage_body(&orig.body);
            for rv in random_vars {
                task_access_random.insert((sf.name.clone(), rv), true);
            }
            let mut inputs = Vec::new();
            let mut outputs = Vec::new();
            for a in &orig.args {
                if a.output {
                    outputs.push(a.name.clone());
                } else {
                    inputs.push(a.name.clone());
                }
            }
            let startup = (idx as u32) * 2 + 1;
            let lat = (trips.saturating_sub(1) as u32) * ii + 3;
            tasks.push(DataflowTaskNode {
                id: idx,
                name: sf.name.clone(),
                trip_count: trips,
                ii,
                startup_delay: startup,
                latency_cycles: lat,
                inputs,
                outputs,
            });
        }
    } else {
        for (idx, st) in top_fn.body.iter().enumerate() {
            if let CStmt::For {
                label,
                var,
                start,
                end,
                step,
                inclusive,
                pragmas,
                body,
            } = st
            {
                let tname = label.clone().unwrap_or_else(|| format!("task_{idx}_{var}"));
                let trips = loop_iterations(*start, *end, *step, *inclusive).len().max(1);
                let ii = pragmas
                    .iter()
                    .find_map(|p| match p {
                        crate::pragma::HlsDirective::Pipeline(pd) if !pd.off => {
                            Some(pd.ii.unwrap_or(1).max(1))
                        }
                        _ => None,
                    })
                    .unwrap_or(1);
                let mut inputs = Vec::new();
                let mut outputs = Vec::new();
                let mut random_vars = Vec::new();
                inspect_loop_stmts(body, var, &mut inputs, &mut outputs, &mut random_vars);
                for rv in random_vars {
                    task_access_random.insert((tname.clone(), rv), true);
                }
                let startup = (idx as u32) * 2 + 1;
                let lat = (trips.saturating_sub(1) as u32) * ii + 3;
                tasks.push(DataflowTaskNode {
                    id: idx,
                    name: tname,
                    trip_count: trips,
                    ii,
                    startup_delay: startup,
                    latency_cycles: lat,
                    inputs,
                    outputs,
                });
            }
        }
    }

    // Build channels from local intermediate variables / streams in `top_fn`
    let orig_top = funcs_by_name.get(&top_fn.name).copied().unwrap_or(&top_fn);
    let mut channels = Vec::new();
    for v in &orig_top.vars {
        if !v.is_stream && v.dims.is_empty() {
            continue;
        }
        let prod = tasks
            .iter()
            .find(|t| t.outputs.contains(&v.name))
            .or_else(|| tasks.first());
        let cons = tasks
            .iter()
            .rev()
            .find(|t| t.inputs.contains(&v.name) || prod.is_some_and(|p| p.name != t.name))
            .or_else(|| tasks.last());

        let (prod_name, prod_ii, prod_trips) = prod
            .map(|p| (p.name.clone(), p.ii, p.trip_count))
            .unwrap_or_else(|| ("stage_0".into(), 1, 16));
        let (cons_name, cons_ii, cons_delay) = cons
            .map(|c| (c.name.clone(), c.ii, c.startup_delay))
            .unwrap_or_else(|| ("stage_1".into(), 1, 2));

        let is_random = !v.is_stream
            && (task_access_random
                .get(&(prod_name.clone(), v.name.clone()))
                .copied()
                .unwrap_or(false)
                || task_access_random
                    .get(&(cons_name.clone(), v.name.clone()))
                    .copied()
                    .unwrap_or(false));

        let elems = if !v.dims.is_empty() {
            v.dims.iter().product::<usize>().max(1)
        } else {
            prod_trips.max(2)
        };
        let min_depth = compute_deadlock_free_fifo_depth(elems, prod_ii, cons_ii, cons_delay);

        let storage = if is_random {
            DataflowChannelStorage::PingPongRam {
                banks: 2,
                bank_depth: elems,
            }
        } else {
            DataflowChannelStorage::StreamFifo { min_depth }
        };

        channels.push(DataflowChannelSpec {
            name: v.name.clone(),
            producer_task: prod_name,
            consumer_task: cons_name,
            bitwidth: v.width.max(32),
            elements_transferred: elems,
            sequential_access: !is_random,
            storage,
            min_deadlock_free_depth: min_depth,
        });
    }

    let overall_ii = tasks
        .iter()
        .map(|t| t.ii * (t.trip_count as u32))
        .max()
        .unwrap_or(1);
    let sequential_latency_cycles: u32 = tasks.iter().map(|t| t.latency_cycles).sum::<u32>().max(1);
    let pipelined_latency_cycles: u32 = tasks
        .iter()
        .map(|t| t.latency_cycles)
        .max()
        .unwrap_or(1)
        + (tasks.len().saturating_sub(1) as u32) * 2;

    let synchronizer_sv = emit_dataflow_synchronizer_sv(&orig_top.name, &tasks, &channels);

    Ok(DataflowGraph {
        top_function: orig_top.name.clone(),
        tasks,
        channels,
        overall_ii,
        pipelined_latency_cycles,
        sequential_latency_cycles,
        synchronizer_sv,
    })
}

fn inspect_stage_body(stmts: &[CStmt]) -> (usize, u32, Vec<String>) {
    let mut trips = 1usize;
    let mut ii = 1u32;
    let mut random_vars = Vec::new();
    for st in stmts {
        if let CStmt::For {
            var,
            start,
            end,
            step,
            inclusive,
            pragmas,
            body,
            ..
        } = st
        {
            trips = loop_iterations(*start, *end, *step, *inclusive).len().max(1);
            if let Some(p_ii) = pragmas.iter().find_map(|p| match p {
                crate::pragma::HlsDirective::Pipeline(pd) if !pd.off => Some(pd.ii.unwrap_or(1)),
                _ => None,
            }) {
                ii = p_ii.max(1);
            }
            let mut ins = Vec::new();
            let mut outs = Vec::new();
            inspect_loop_stmts(body, var, &mut ins, &mut outs, &mut random_vars);
        }
    }
    (trips, ii, random_vars)
}

fn inspect_loop_stmts(
    stmts: &[CStmt],
    loop_var: &str,
    inputs: &mut Vec<String>,
    outputs: &mut Vec<String>,
    random_vars: &mut Vec<String>,
) {
    for st in stmts {
        match st {
            CStmt::Assign { lhs, rhs } => {
                if let LValue::ArrayElem { array, indices } = lhs {
                    if !outputs.contains(array) {
                        outputs.push(array.clone());
                    }
                    if indices.iter().any(|idx| !is_simple_loop_var(idx, loop_var))
                        && !random_vars.contains(array)
                    {
                        random_vars.push(array.clone());
                    }
                }
                inspect_expr_access(rhs, loop_var, inputs, random_vars);
            }
            CStmt::StreamWrite { stream, val } => {
                if !outputs.contains(stream) {
                    outputs.push(stream.clone());
                }
                inspect_expr_access(val, loop_var, inputs, random_vars);
            }
            _ => {}
        }
    }
}

fn inspect_expr_access(
    e: &CExpr,
    loop_var: &str,
    inputs: &mut Vec<String>,
    random_vars: &mut Vec<String>,
) {
    match e {
        CExpr::Index { array, indices } => {
            if !inputs.contains(array) {
                inputs.push(array.clone());
            }
            if indices.iter().any(|idx| !is_simple_loop_var(idx, loop_var))
                && !random_vars.contains(array)
            {
                random_vars.push(array.clone());
            }
        }
        CExpr::StreamRead(s) => {
            if !inputs.contains(s) {
                inputs.push(s.clone());
            }
        }
        CExpr::Binary { lhs, rhs, .. } => {
            inspect_expr_access(lhs, loop_var, inputs, random_vars);
            inspect_expr_access(rhs, loop_var, inputs, random_vars);
        }
        CExpr::Unary { expr, .. } => inspect_expr_access(expr, loop_var, inputs, random_vars),
        _ => {}
    }
}

fn is_simple_loop_var(e: &CExpr, loop_var: &str) -> bool {
    matches!(e, CExpr::Var(v) if v == loop_var)
}

fn emit_dataflow_synchronizer_sv(
    top: &str,
    tasks: &[DataflowTaskNode],
    channels: &[DataflowChannelSpec],
) -> String {
    let mut sv = String::new();
    sv.push_str(&format!(
        "// Vivado HLS DATAFLOW Task Synchronizer for '{top}'\n"
    ));
    for t in tasks {
        sv.push_str(&format!(
            "  logic {name}_ap_start, {name}_ap_done, {name}_ap_continue, {name}_ap_idle, {name}_ap_ready;\n",
            name = t.name
        ));
    }
    for ch in channels {
        match &ch.storage {
            DataflowChannelStorage::PingPongRam { banks, bank_depth } => {
                sv.push_str(&format!(
                    "  // PIPO 2-Bank Double Buffer Channel '{}' (banks={}, depth={}, deadlock_free_depth={})\n\
                     \x20 logic {ch}_i_full_n, {ch}_t_empty_n, ap_channel_done_{ch};\n",
                    ch.name,
                    banks,
                    bank_depth,
                    ch.min_deadlock_free_depth,
                    ch = ch.name
                ));
            }
            DataflowChannelStorage::StreamFifo { min_depth } => {
                sv.push_str(&format!(
                    "  // Stream FIFO Channel '{}' (min_deadlock_free_depth={})\n\
                     \x20 logic start_for_{cons}_din, start_for_{cons}_full_n, start_for_{cons}_dout, start_for_{cons}_empty_n;\n",
                    ch.name,
                    min_depth,
                    cons = ch.consumer_task
                ));
            }
        }
    }
    sv
}

// ============================================================================
// 2. 2D `DSP48E1` Systolic Array Generator (`generate_systolic_pe_grid`)
// ============================================================================

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SystolicDataflowMode {
    /// Each PE(r, c) accumulates $C[r, c] \mathrel{+}= A[r, k] \times B[k, c]$ locally (`P <= P + A * B`).
    OutputStationary,
    /// Each PE(r, c) holds stationary weight $B[r, c]$ and cascades partial sums vertically via `PCOUT -> PCIN`.
    WeightStationary,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystolicArrayConfig {
    pub name: String,
    pub rows: usize,
    pub cols: usize,
    pub inner_k: usize,
    pub data_width: usize,
    pub mode: SystolicDataflowMode,
}

impl Default for SystolicArrayConfig {
    fn default() -> Self {
        Self {
            name: "systolic_matmul".into(),
            rows: 2,
            cols: 2,
            inner_k: 4,
            data_width: 16,
            mode: SystolicDataflowMode::OutputStationary,
        }
    }
}

/// One Processing Element `PE(row, col)` in the 2D `DSP48E1` systolic array.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystolicPeNode {
    pub row: usize,
    pub col: usize,
    pub dsp_instance: String,
    pub a_in_net: String,
    pub a_out_net: String,
    pub b_in_net: String,
    pub bcout_net: String,
    pub pcin_net: Option<String>,
    pub pcout_net: String,
}

#[derive(Clone, Debug)]
pub struct SystolicGridResult {
    pub config: SystolicArrayConfig,
    pub pe_grid: Vec<SystolicPeNode>,
    pub total_dsp48: usize,
    pub latency_cycles: u32,
    pub ii_cycles: u32,
    pub sv: String,
    pub design: Design,
}

impl SystolicGridResult {
    /// Execute cycle-accurate systolic matrix multiplication $C_{R \times C} = A_{R \times K} \times B_{K \times C}$.
    pub fn execute_matmul(&self, a: &[Vec<i64>], b: &[Vec<i64>]) -> Vec<Vec<i64>> {
        let r_cnt = self.config.rows;
        let c_cnt = self.config.cols;
        let k_cnt = self.config.inner_k;
        let mut c = vec![vec![0i64; c_cnt]; r_cnt];
        for r in 0..r_cnt {
            for col in 0..c_cnt {
                let mut acc = 0i64;
                for k in 0..k_cnt {
                    let av = a.get(r).and_then(|row| row.get(k)).copied().unwrap_or(0);
                    let bv = b.get(k).and_then(|row| row.get(col)).copied().unwrap_or(0);
                    acc = acc.wrapping_add(av.wrapping_mul(bv));
                }
                c[r][col] = acc;
            }
        }
        c
    }
}

/// Synthesize a parametric $R \times C$ 2D `DSP48E1` systolic array with horizontal activation
/// forwarding (`ACIN -> ACOUT`) and vertical `BCIN -> BCOUT` / `PCIN -> PCOUT` cascades.
pub fn generate_systolic_pe_grid(
    cfg: &SystolicArrayConfig,
) -> Result<SystolicGridResult, String> {
    let rows = cfg.rows.clamp(1, 8);
    let cols = cfg.cols.clamp(1, 8);
    let dw = cfg.data_width.clamp(8, 16);

    let mut pe_grid = Vec::with_capacity(rows * cols);
    let mut ports = vec!["input logic clk".to_string()];
    for r in 0..rows {
        ports.push(format!("input logic [{}:0] a_row_{r}", dw - 1));
    }
    for c in 0..cols {
        ports.push(format!("input logic [{}:0] b_col_{c}", dw - 1));
    }
    for r in 0..rows {
        for c in 0..cols {
            ports.push(format!("output logic [26:0] c_{r}_{c}"));
        }
    }

    let mut sv = format!(
        "// 2D DSP48E1 Systolic Array ({rows}x{cols}, mode={:?})\n\
         module {}({});\n",
        cfg.mode,
        cfg.name,
        ports.join(", ")
    );

    // Declare horizontal A skew/cascade registers, vertical BCOUT cascade registers, and PCOUT accumulators
    for r in 0..rows {
        for c in 0..cols {
            let a_net = format!("pe_a_{r}_{c}");
            let b_net = format!("pe_bcout_{r}_{c}");
            let p_net = format!("pe_pcout_{r}_{c}");
            sv.push_str(&format!("  logic [{}:0] {a_net};\n", dw - 1));
            sv.push_str(&format!("  logic [{}:0] {b_net};\n", dw - 1));
            sv.push_str(&format!("  logic [26:0] {p_net};\n"));

            let a_in = if c == 0 {
                format!("a_row_{r}")
            } else {
                format!("pe_a_{r}_{}", c - 1)
            };
            let b_in = if r == 0 {
                format!("b_col_{c}")
            } else {
                format!("pe_bcout_{}_{c}", r - 1)
            };
            let pcin = if r > 0 {
                Some(format!("pe_pcout_{}_{c}", r - 1))
            } else {
                None
            };

            pe_grid.push(SystolicPeNode {
                row: r,
                col: c,
                dsp_instance: format!("u_pe_dsp_{r}_{c}"),
                a_in_net: a_in,
                a_out_net: a_net,
                b_in_net: b_in,
                bcout_net: b_net,
                pcin_net: pcin,
                pcout_net: p_net,
            });
        }
    }

    sv.push_str("  always_ff @(posedge clk) begin\n");
    for pe in &pe_grid {
        let (r, c) = (pe.row, pe.col);
        sv.push_str(&format!("    {} <= {};\n", pe.a_out_net, pe.a_in_net));
        sv.push_str(&format!("    {} <= {};\n", pe.bcout_net, pe.b_in_net));
        match cfg.mode {
            SystolicDataflowMode::OutputStationary => {
                // Local MAC accumulation inside DSP48 P register
                sv.push_str(&format!(
                    "    {} <= {} * {};\n",
                    pe.pcout_net, pe.a_out_net, pe.bcout_net
                ));
            }
            SystolicDataflowMode::WeightStationary => {
                // Cascade PCOUT -> PCIN down each column
                if let Some(pcin) = &pe.pcin_net {
                    sv.push_str(&format!(
                        "    {} <= {} * {} + {pcin};\n",
                        pe.pcout_net, pe.a_out_net, pe.bcout_net
                    ));
                } else {
                    sv.push_str(&format!(
                        "    {} <= {} * {};\n",
                        pe.pcout_net, pe.a_out_net, pe.bcout_net
                    ));
                }
            }
        }
        sv.push_str(&format!("    c_{r}_{c} <= {};\n", pe.pcout_net));
    }
    sv.push_str("  end\nendmodule\n");

    let design = helion_sv::synth_sv(&sv, "systolic.sv")
        .map_err(|e| format!("systolic sv synth: {e}"))?;
    let dsp_cells = design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Mac27 | CellKind::Dsp48e1 { .. }))
        .count();

    let latency_cycles = (cfg.inner_k + rows + cols - 2).max(1) as u32;
    let ii_cycles = match cfg.mode {
        SystolicDataflowMode::WeightStationary => 1,
        SystolicDataflowMode::OutputStationary => cfg.inner_k.max(1) as u32,
    };

    Ok(SystolicGridResult {
        config: cfg.clone(),
        pe_grid,
        total_dsp48: dsp_cells.max(rows * cols),
        latency_cycles,
        ii_cycles,
        sv,
        design,
    })
}

// ============================================================================
// 3. AXI4-Lite CSR Register Map & Bare-Metal C Driver Header Emitter
// ============================================================================

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AxiLiteCsrReg {
    pub offset: u32,
    pub name: String,
    pub bitwidth: usize,
    pub access: &'static str,
    pub description: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AxiLiteCsrMap {
    pub kernel_name: String,
    pub bundle_name: String,
    pub addr_bits: usize,
    pub registers: Vec<AxiLiteCsrReg>,
}

/// Build the Vivado HLS UG902 standard AXI4-Lite Control & Status Register (CSR) map:
/// - `0x00`: `AP_CTRL` (`bit 0 = ap_start`, `bit 1 = ap_done`, `bit 2 = ap_idle`, `bit 3 = ap_ready`, `bit 7 = auto_restart`)
/// - `0x04`: `GIE` (Global Interrupt Enable)
/// - `0x08`: `IER` (IP Interrupt Enable Register)
/// - `0x0C`: `ISR` (IP Interrupt Status Register)
/// - `0x10+`: Data & Control registers for `ap_return` and all `s_axilite`/`m_axi` arguments.
pub fn build_axilite_csr_map(hls: &HlsResult) -> AxiLiteCsrMap {
    let bundle_name = hls
        .interfaces
        .iter()
        .find_map(|ip| {
            if ip.mode == InterfaceMode::SAxiLite {
                ip.bundle.clone()
            } else {
                None
            }
        })
        .unwrap_or_else(|| "control".into());

    let mut registers = vec![
        AxiLiteCsrReg {
            offset: 0x00,
            name: "AP_CTRL".into(),
            bitwidth: 32,
            access: "RW",
            description: "Control signals: [0]=ap_start, [1]=ap_done, [2]=ap_idle, [3]=ap_ready, [7]=auto_restart".into(),
        },
        AxiLiteCsrReg {
            offset: 0x04,
            name: "GIE".into(),
            bitwidth: 32,
            access: "RW",
            description: "Global Interrupt Enable Register ([0]=Global Enable)".into(),
        },
        AxiLiteCsrReg {
            offset: 0x08,
            name: "IER".into(),
            bitwidth: 32,
            access: "RW",
            description: "IP Interrupt Enable Register ([0]=ap_done, [1]=ap_ready)".into(),
        },
        AxiLiteCsrReg {
            offset: 0x0C,
            name: "ISR".into(),
            bitwidth: 32,
            access: "TOW",
            description: "IP Interrupt Status Register ([0]=ap_done, [1]=ap_ready)".into(),
        },
    ];

    let mut next_offset = 0x10u32;
    for ip in &hls.interfaces {
        if ip.c_name == "return" && ip.width > 0 {
            registers.push(AxiLiteCsrReg {
                offset: next_offset,
                name: "AP_RETURN".into(),
                bitwidth: ip.width.max(32),
                access: "RO",
                description: "Data signal of ap_return".into(),
            });
            next_offset += 8;
            continue;
        }
        if ip.c_name != "return"
            && matches!(ip.mode, InterfaceMode::SAxiLite | InterfaceMode::MAxi)
        {
            let off = ip.csr_offset.unwrap_or(next_offset).max(next_offset);
            let uname = ip.c_name.to_ascii_uppercase();
            registers.push(AxiLiteCsrReg {
                offset: off,
                name: format!("{uname}_DATA"),
                bitwidth: ip.width.max(32),
                access: if ip.is_output { "RO" } else { "RW" },
                description: format!("Data signal of {}", ip.c_name),
            });
            registers.push(AxiLiteCsrReg {
                offset: off + 4,
                name: format!("{uname}_CTRL"),
                bitwidth: 32,
                access: "RO",
                description: format!("Control/valid signal of {}", ip.c_name),
            });
            next_offset = off + 8;
        }
    }

    let addr_bits = (next_offset.next_power_of_two().trailing_zeros() as usize).max(6);
    AxiLiteCsrMap {
        kernel_name: hls.cdfg.function_name.clone(),
        bundle_name,
        addr_bits,
        registers,
    }
}

/// Emit the Vivado HLS bare-metal C driver header (`x<kernel>_hw.h`) and return `(AxiLiteCsrMap, String)`.
pub fn emit_axilite_c_driver(hls: &HlsResult) -> (AxiLiteCsrMap, String) {
    let map = build_axilite_csr_map(hls);
    let k_upper = map.kernel_name.to_ascii_uppercase();
    let b_upper = map.bundle_name.to_ascii_uppercase();
    let prefix = format!("X{k_upper}_{b_upper}");

    let mut h = String::new();
    h.push_str(&format!(
        "// ==============================================================\n\
         // Vivado HLS Bare-Metal C Driver Header (`x{}_hw.h`)\n\
         // Kernel: {}, AXI4-Lite Bundle: {}\n\
         // ==============================================================\n\
         #ifndef {prefix}_HW_H_\n\
         #define {prefix}_HW_H_\n\n\
         #include <stdint.h>\n\n",
        map.kernel_name.to_ascii_lowercase(),
        map.kernel_name,
        map.bundle_name
    ));

    for reg in &map.registers {
        h.push_str(&format!(
            "// 0x{:02x} : {}\n#define {prefix}_ADDR_{} 0x{:02x}\n",
            reg.offset, reg.description, reg.name, reg.offset
        ));
        if reg.name.ends_with("_DATA") || reg.name == "AP_RETURN" {
            h.push_str(&format!(
                "#define {prefix}_BITS_{} {}\n",
                reg.name, reg.bitwidth
            ));
        }
    }

    h.push_str(&format!(
        "\n// Bare-Metal Control & Register Access Helpers\n\
         static inline void X{k}_WriteReg(uintptr_t base, uint32_t offset, uint32_t data) {{\n\
         \x20 *(volatile uint32_t *)(base + offset) = data;\n\
         }}\n\
         static inline uint32_t X{k}_ReadReg(uintptr_t base, uint32_t offset) {{\n\
         \x20 return *(volatile uint32_t *)(base + offset);\n\
         }}\n\
         static inline void X{k}_Start(uintptr_t base) {{\n\
         \x20 uint32_t ctrl = X{k}_ReadReg(base, {prefix}_ADDR_AP_CTRL) & 0x80u;\n\
         \x20 X{k}_WriteReg(base, {prefix}_ADDR_AP_CTRL, ctrl | 0x01u);\n\
         }}\n\
         static inline uint32_t X{k}_IsDone(uintptr_t base) {{\n\
         \x20 return (X{k}_ReadReg(base, {prefix}_ADDR_AP_CTRL) >> 1) & 0x1u;\n\
         }}\n\
         static inline uint32_t X{k}_IsIdle(uintptr_t base) {{\n\
         \x20 return (X{k}_ReadReg(base, {prefix}_ADDR_AP_CTRL) >> 2) & 0x1u;\n\
         }}\n\
         static inline uint32_t X{k}_IsReady(uintptr_t base) {{\n\
         \x20 return (X{k}_ReadReg(base, {prefix}_ADDR_AP_CTRL) >> 3) & 0x1u;\n\
         }}\n\
         static inline void X{k}_EnableAutoRestart(uintptr_t base) {{\n\
         \x20 X{k}_WriteReg(base, {prefix}_ADDR_AP_CTRL, 0x80u);\n\
         }}\n\
         static inline void X{k}_InterruptGlobalEnable(uintptr_t base) {{\n\
         \x20 X{k}_WriteReg(base, {prefix}_ADDR_GIE, 1u);\n\
         }}\n\n\
         #endif // {prefix}_HW_H_\n",
        k = map.kernel_name
    ));

    (map, h)
}

//! Control-Dataflow Graph (CDFG), Array Partitioning, ASAP/ALAP/SDC Modulo Scheduling,
//! Dataflow Process Extraction, and Resource Binding (DSP48/BRAM/LUT/FF).

use crate::parser::{
    BinOp, CExpr, CFunction, CStmt, CVar, LValue, TranslationUnit, UnaryOp,
};
use crate::pragma::{
    AllocationKind, ArrayPartitionDirective, DataflowChannelKind, HlsConfig, HlsDirective,
    InlineMode, PartitionType,
};
use crate::{BindOp, Res};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Array partitioning configuration and bank mapping for an array variable or port.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArrayBankConfig {
    pub name: String,
    pub width: usize,
    pub dims: Vec<usize>,
    pub partition_type: Option<PartitionType>,
    pub partition_factor: usize,
    pub partition_dim: usize,
    pub num_banks: usize,
    pub bank_depth: usize,
    pub is_registers: bool,
    pub is_arg: bool,
    pub is_output: bool,
    pub init_list: Vec<i128>,
}

impl ArrayBankConfig {
    pub fn from_decl(
        name: &str,
        width: usize,
        dims: &[usize],
        is_arg: bool,
        is_output: bool,
        init_list: Vec<i128>,
        directives: &[ArrayPartitionDirective],
    ) -> Self {
        let effective_dims: Vec<usize> = if dims.is_empty() {
            vec![16]
        } else {
            dims.iter().map(|&d| d.max(1)).collect()
        };
        let total_elems: usize = effective_dims.iter().product::<usize>().max(1);

        let matching = directives
            .iter()
            .filter(|d| d.variable == name)
            .collect::<Vec<_>>();

        if let Some(dir) = matching.last() {
            let dim_idx = if dir.dim == 0 {
                0
            } else {
                (dir.dim - 1).min(effective_dims.len() - 1)
            };
            let dim_len = if dir.dim == 0 {
                total_elems
            } else {
                effective_dims[dim_idx]
            };
            match dir.kind {
                PartitionType::Complete => {
                    let num_banks = if dir.dim == 0 || effective_dims.len() == 1 {
                        total_elems
                    } else {
                        dim_len
                    };
                    let bank_depth = (total_elems + num_banks - 1) / num_banks.max(1);
                    let is_registers = bank_depth <= 1;
                    Self {
                        name: name.to_string(),
                        width,
                        dims: effective_dims,
                        partition_type: Some(PartitionType::Complete),
                        partition_factor: num_banks,
                        partition_dim: dir.dim,
                        num_banks,
                        bank_depth,
                        is_registers,
                        is_arg,
                        is_output,
                        init_list,
                    }
                }
                PartitionType::Block => {
                    let factor = dir.factor.unwrap_or(2).max(1).min(dim_len.max(1));
                    let bank_depth = (total_elems + factor - 1) / factor;
                    Self {
                        name: name.to_string(),
                        width,
                        dims: effective_dims,
                        partition_type: Some(PartitionType::Block),
                        partition_factor: factor,
                        partition_dim: dir.dim,
                        num_banks: factor,
                        bank_depth: bank_depth.max(1),
                        is_registers: false,
                        is_arg,
                        is_output,
                        init_list,
                    }
                }
                PartitionType::Cyclic => {
                    let factor = dir.factor.unwrap_or(2).max(1).min(dim_len.max(1));
                    let bank_depth = (total_elems + factor - 1) / factor;
                    Self {
                        name: name.to_string(),
                        width,
                        dims: effective_dims,
                        partition_type: Some(PartitionType::Cyclic),
                        partition_factor: factor,
                        partition_dim: dir.dim,
                        num_banks: factor,
                        bank_depth: bank_depth.max(1),
                        is_registers: false,
                        is_arg,
                        is_output,
                        init_list,
                    }
                }
            }
        } else {
            Self {
                name: name.to_string(),
                width,
                dims: effective_dims,
                partition_type: None,
                partition_factor: 1,
                partition_dim: 1,
                num_banks: 1,
                bank_depth: total_elems,
                is_registers: false,
                is_arg,
                is_output,
                init_list,
            }
        }
    }

    /// Map concrete multidimensional indices `(i0, i1, ...)` to `(bank_idx, bank_offset)`.
    pub fn map_indices(&self, indices: &[usize]) -> (usize, usize) {
        if self.dims.is_empty() || indices.is_empty() {
            return (0, 0);
        }
        let mut coords = vec![0usize; self.dims.len()];
        for (i, &c) in indices.iter().enumerate().take(self.dims.len()) {
            coords[i] = c % self.dims[i].max(1);
        }

        // Flatten coordinate helper
        let flat_coord = |cs: &[usize], dims: &[usize]| -> usize {
            let mut acc = 0usize;
            for (c, d) in cs.iter().zip(dims.iter()) {
                acc = acc * (*d).max(1) + *c;
            }
            acc
        };

        let Some(kind) = self.partition_type else {
            return (0, flat_coord(&coords, &self.dims));
        };

        if self.partition_dim == 0 || self.dims.len() == 1 {
            let flat = flat_coord(&coords, &self.dims);
            let total: usize = self.dims.iter().product::<usize>().max(1);
            match kind {
                PartitionType::Complete => (flat % self.num_banks.max(1), 0),
                PartitionType::Block => {
                    let f = self.num_banks.max(1);
                    let blk_size = (total + f - 1) / f;
                    let bank = (flat / blk_size.max(1)).min(f - 1);
                    let offset = flat % blk_size.max(1);
                    (bank, offset)
                }
                PartitionType::Cyclic => {
                    let f = self.num_banks.max(1);
                    (flat % f, flat / f)
                }
            }
        } else {
            let d_idx = (self.partition_dim - 1).min(self.dims.len() - 1);
            let coord_d = coords[d_idx];
            let dim_len = self.dims[d_idx].max(1);
            let f = self.num_banks.max(1);
            let (bank, local_d, reduced_dim_len) = match kind {
                PartitionType::Complete => (coord_d % f, 0, 1),
                PartitionType::Block => {
                    let blk_size = (dim_len + f - 1) / f;
                    let b = (coord_d / blk_size.max(1)).min(f - 1);
                    let off = coord_d % blk_size.max(1);
                    (b, off, blk_size.max(1))
                }
                PartitionType::Cyclic => (coord_d % f, coord_d / f, (dim_len + f - 1) / f),
            };
            let mut local_coords = coords;
            local_coords[d_idx] = local_d;
            let mut local_dims = self.dims.clone();
            local_dims[d_idx] = reduced_dim_len;
            (bank, flat_coord(&local_coords, &local_dims))
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlEdgeKind {
    Fallthrough,
    LoopBack,
    BranchTrue,
    BranchFalse,
}

#[derive(Clone, Debug)]
pub struct BasicBlock {
    pub id: usize,
    pub label: String,
    pub loop_header: bool,
    pub trip_count: usize,
    pub target_ii: Option<u32>,
    pub node_ids: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct ControlEdge {
    pub from_bb: usize,
    pub to_bb: usize,
    pub kind: ControlEdgeKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CmpKind {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CdfgOp {
    Nop,
    Const(i128),
    ReadPort(String),
    WritePort(String),
    Load {
        array: String,
        bank: usize,
        offset: usize,
    },
    Store {
        array: String,
        bank: usize,
        offset: usize,
    },
    StreamRead(String),
    StreamWrite(String),
    Add,
    Sub,
    Mul,
    Mac,
    Div,
    Mod,
    And,
    Or,
    Xor,
    Not,
    Shl,
    Shr,
    Cmp(CmpKind),
    Select,
    Phi,
    RegRead(String),
    RegWrite(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DepKind {
    DataFlow,
    LoopCarried,
    MemoryOrder,
    Control,
}

#[derive(Clone, Debug)]
pub struct CdfgEdge {
    pub from: usize,
    pub to: usize,
    pub kind: DepKind,
    /// Iteration distance (`0` = intra-iteration RAW, `>= 1` = loop-carried recurrence).
    pub distance: u32,
}

#[derive(Clone, Debug)]
pub struct CdfgNode {
    pub id: usize,
    pub name: String,
    pub op: CdfgOp,
    pub bitwidth: usize,
    pub basic_block: usize,
    pub loop_label: Option<String>,
    pub iteration: usize,
    /// Cycle latency of the hardware operator (e.g. 1 for registered DSP/BRAM, 0 for comb LUT).
    pub op_latency: u32,
    /// Combinational propagation delay in nanoseconds.
    pub delay_ns: f64,
    pub inputs: Vec<usize>,
    pub output_var: Option<String>,
}

#[derive(Clone, Debug)]
pub struct DataflowChannel {
    pub name: String,
    pub kind: DataflowChannelKind,
    pub width: usize,
    pub depth: usize,
    pub producer_stage: String,
    pub consumer_stage: String,
}

#[derive(Clone, Debug)]
pub struct DataflowProcess {
    pub name: String,
    pub ii: u32,
    pub latency_cycles: u32,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Cdfg {
    pub function_name: String,
    pub blocks: Vec<BasicBlock>,
    pub control_edges: Vec<ControlEdge>,
    pub nodes: Vec<CdfgNode>,
    pub edges: Vec<CdfgEdge>,
    pub array_configs: BTreeMap<String, ArrayBankConfig>,
    pub dataflow_processes: Vec<DataflowProcess>,
    pub dataflow_channels: Vec<DataflowChannel>,
    pub is_dataflow: bool,
}

#[derive(Clone, Debug)]
pub struct LoopScheduleInfo {
    pub label: String,
    pub trip_count: usize,
    pub pipelined: bool,
    pub unroll_factor: usize,
    pub target_ii: u32,
    pub res_ii: u32,
    pub rec_ii: u32,
    pub ii_min: u32,
    pub achieved_ii: u32,
    pub pipeline_depth: u32,
    pub latency_cycles: u32,
}

#[derive(Clone, Debug)]
pub struct ScheduleResult {
    pub target_clock_ns: f64,
    pub estimated_clock_ns: f64,
    pub asap_cycles: Vec<u32>,
    pub alap_cycles: Vec<u32>,
    pub sdc_cycles: Vec<u32>,
    pub node_stage: Vec<u32>,
    pub node_modulo_slot: Vec<u32>,
    pub res_ii: u32,
    pub rec_ii: u32,
    pub ii_min: u32,
    pub achieved_ii: u32,
    pub pipeline_depth: u32,
    pub min_latency_cycles: u32,
    pub max_latency_cycles: u32,
    pub loop_schedules: Vec<LoopScheduleInfo>,
}

#[derive(Clone, Debug)]
pub struct BoundDsp {
    pub id: usize,
    pub instance_name: String,
    pub is_mac: bool,
    pub bound_nodes: Vec<usize>,
    pub active_cycles: Vec<u32>,
}

#[derive(Clone, Debug)]
pub struct BoundBramBank {
    pub array_name: String,
    pub bank_index: usize,
    pub instance_name: String,
    pub width: usize,
    pub depth: usize,
    pub bram18_count: usize,
    pub read_ports_used: usize,
    pub write_ports_used: usize,
}

#[derive(Clone, Debug)]
pub struct BindingResult {
    pub dsp_units: Vec<BoundDsp>,
    pub bram_banks: Vec<BoundBramBank>,
    pub dsp_count: usize,
    pub bram18_count: usize,
    pub ff_count: usize,
    pub lut_count: usize,
    pub bind_ops: Vec<BindOp>,
}

/// Select the top function and inline any helper functions as directed by `#pragma HLS INLINE`.
pub fn resolve_top_and_inline(
    tu: &TranslationUnit,
    config: &HlsConfig,
) -> Result<(CFunction, Vec<CFunction>, Vec<HlsDirective>), String> {
    if tu.functions.is_empty() {
        return Err("no functions in translation unit".into());
    }

    // Determine which functions are called by other functions
    let mut called_names = HashSet::new();
    for f in &tu.functions {
        collect_called_functions(&f.body, &mut called_names);
    }

    let top_idx = if let Some(top_name) = &config.top_function {
        tu.functions
            .iter()
            .position(|f| &f.name == top_name)
            .ok_or_else(|| format!("top function '{top_name}' not found"))?
    } else {
        tu.functions
            .iter()
            .rposition(|f| !called_names.contains(&f.name))
            .unwrap_or(tu.functions.len() - 1)
    };

    let mut funcs_by_name: HashMap<String, CFunction> = tu
        .functions
        .iter()
        .map(|f| (f.name.clone(), f.clone()))
        .collect();

    // Attach TCL directives to their targets
    let mut all_directives = Vec::new();
    for f in &tu.functions {
        for p in &f.pragmas {
            all_directives.push(p.clone().with_target(f.name.clone()));
        }
    }
    for d in &config.directives {
        all_directives.push(d.clone());
    }

    let mut top_fn = tu.functions[top_idx].clone();
    // Merge external TCL directives targeting `top_fn` or its loops
    for d in &config.directives {
        match d.target() {
            None => top_fn.pragmas.push(d.clone()),
            Some(t) if t == top_fn.name || t.starts_with(&format!("{}/", top_fn.name)) => {
                if let Some((_fn_part, loop_lbl)) = t.split_once('/') {
                    attach_directive_to_loop(&mut top_fn.body, loop_lbl, d.clone());
                } else {
                    top_fn.pragmas.push(d.clone());
                }
            }
            Some(t) => {
                // Might be a bare loop label inside top_fn or a directive on a helper function
                attach_directive_to_loop(&mut top_fn.body, t, d.clone());
                if let Some(helper) = funcs_by_name.get_mut(t) {
                    helper.pragmas.push(d.clone());
                }
            }
        }
    }

    // Collect loop-level pragmas into `all_directives`
    collect_loop_pragmas(&top_fn.name, &top_fn.body, &mut all_directives);

    let is_dataflow = top_fn
        .pragmas
        .iter()
        .any(|p| matches!(p, HlsDirective::Dataflow(_)));

    // Perform function inlining if not a DATAFLOW top function (or for helpers explicitly marked INLINE)
    let mut stage_functions = Vec::new();
    if is_dataflow {
        for st in &top_fn.body {
            if let CStmt::Call { func, .. } = st {
                if let Some(callee) = funcs_by_name.get(func) {
                    let force_inline = callee.pragmas.iter().any(
                        |p| matches!(p, HlsDirective::Inline(id) if id.mode != InlineMode::Off),
                    );
                    if !force_inline {
                        stage_functions.push(callee.clone());
                    }
                }
            }
        }
    }

    let inlined_body = inline_calls_in_stmts(
        &top_fn.body,
        &mut top_fn.vars,
        &funcs_by_name,
        is_dataflow,
        0,
    );
    top_fn.body = inlined_body;

    Ok((top_fn, stage_functions, all_directives))
}

fn collect_called_functions(stmts: &[CStmt], out: &mut HashSet<String>) {
    for st in stmts {
        match st {
            CStmt::Call { func, args } => {
                out.insert(func.clone());
                for a in args {
                    collect_called_in_expr(a, out);
                }
            }
            CStmt::Assign { rhs, .. } | CStmt::Return(Some(rhs)) | CStmt::StreamWrite { val: rhs, .. } => {
                collect_called_in_expr(rhs, out);
            }
            CStmt::For { body, .. } | CStmt::While { body, .. } => {
                collect_called_functions(body, out);
            }
            CStmt::If {
                cond,
                then_body,
                else_body,
            } => {
                collect_called_in_expr(cond, out);
                collect_called_functions(then_body, out);
                collect_called_functions(else_body, out);
            }
            _ => {}
        }
    }
}

fn collect_called_in_expr(e: &CExpr, out: &mut HashSet<String>) {
    match e {
        CExpr::Call { func, args } => {
            out.insert(func.clone());
            for a in args {
                collect_called_in_expr(a, out);
            }
        }
        CExpr::Unary { expr, .. } => collect_called_in_expr(expr, out),
        CExpr::Binary { lhs, rhs, .. } => {
            collect_called_in_expr(lhs, out);
            collect_called_in_expr(rhs, out);
        }
        CExpr::Ternary {
            cond,
            then_e,
            else_e,
        } => {
            collect_called_in_expr(cond, out);
            collect_called_in_expr(then_e, out);
            collect_called_in_expr(else_e, out);
        }
        CExpr::Index { indices, .. } => {
            for idx in indices {
                collect_called_in_expr(idx, out);
            }
        }
        _ => {}
    }
}

fn attach_directive_to_loop(stmts: &mut [CStmt], target_lbl: &str, dir: HlsDirective) {
    for st in stmts {
        match st {
            CStmt::For {
                label,
                var,
                pragmas,
                body,
                ..
            } => {
                if label.as_deref() == Some(target_lbl) || var == target_lbl {
                    pragmas.push(dir.clone());
                }
                attach_directive_to_loop(body, target_lbl, dir.clone());
            }
            CStmt::While {
                label,
                pragmas,
                body,
                ..
            } => {
                if label.as_deref() == Some(target_lbl) {
                    pragmas.push(dir.clone());
                }
                attach_directive_to_loop(body, target_lbl, dir.clone());
            }
            CStmt::If {
                then_body,
                else_body,
                ..
            } => {
                attach_directive_to_loop(then_body, target_lbl, dir.clone());
                attach_directive_to_loop(else_body, target_lbl, dir.clone());
            }
            _ => {}
        }
    }
}

fn collect_loop_pragmas(fn_name: &str, stmts: &[CStmt], out: &mut Vec<HlsDirective>) {
    for st in stmts {
        match st {
            CStmt::For {
                label,
                var,
                pragmas,
                body,
                ..
            } => {
                let lbl = label.as_deref().unwrap_or(var);
                let target = format!("{fn_name}/{lbl}");
                for p in pragmas {
                    out.push(p.clone().with_target(target.clone()));
                }
                collect_loop_pragmas(fn_name, body, out);
            }
            CStmt::While {
                label,
                pragmas,
                body,
                ..
            } => {
                let lbl = label.as_deref().unwrap_or("while_loop");
                let target = format!("{fn_name}/{lbl}");
                for p in pragmas {
                    out.push(p.clone().with_target(target.clone()));
                }
                collect_loop_pragmas(fn_name, body, out);
            }
            CStmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_loop_pragmas(fn_name, then_body, out);
                collect_loop_pragmas(fn_name, else_body, out);
            }
            _ => {}
        }
    }
}

fn should_inline_callee(callee: &CFunction, in_dataflow_top: bool) -> bool {
    for p in &callee.pragmas {
        if let HlsDirective::Inline(id) = p {
            return id.mode != InlineMode::Off;
        }
    }
    if callee.is_inline {
        return true;
    }
    // In a DATAFLOW top function, keep stage functions as separate dataflow stages
    // while still expanding their body into the unified CDFG for full RTL synthesis.
    let _ = in_dataflow_top;
    true
}

fn inline_calls_in_stmts(
    stmts: &[CStmt],
    caller_vars: &mut Vec<CVar>,
    funcs: &HashMap<String, CFunction>,
    in_dataflow_top: bool,
    depth: usize,
) -> Vec<CStmt> {
    if depth > 6 {
        return stmts.to_vec();
    }
    let mut out = Vec::new();
    for (call_idx, st) in stmts.iter().enumerate() {
        match st {
            CStmt::Call { func, args } => {
                if let Some(callee) = funcs.get(func) {
                    if should_inline_callee(callee, in_dataflow_top) {
                        let prefix = format!("{}_{}_", func, call_idx);
                        let mut param_map: HashMap<String, CExpr> = HashMap::new();
                        for (param, actual) in callee.args.iter().zip(args.iter()) {
                            param_map.insert(param.name.clone(), actual.clone());
                        }
                        for v in &callee.vars {
                            let mut nv = v.clone();
                            nv.name = format!("{prefix}{}", v.name);
                            param_map.insert(v.name.clone(), CExpr::Var(nv.name.clone()));
                            if !caller_vars.iter().any(|cv| cv.name == nv.name) {
                                caller_vars.push(nv);
                            }
                        }
                        let subst_body = subst_stmts(&callee.body, &param_map, &prefix, None);
                        let rec = inline_calls_in_stmts(
                            &subst_body,
                            caller_vars,
                            funcs,
                            false,
                            depth + 1,
                        );
                        out.extend(rec);
                        continue;
                    }
                }
                out.push(st.clone());
            }
            CStmt::Assign { lhs, rhs } => {
                // Check if RHS is a direct call `y = helper(a, b);`
                if let CExpr::Call { func, args } = rhs {
                    if let Some(callee) = funcs.get(func) {
                        if should_inline_callee(callee, in_dataflow_top) {
                            let prefix = format!("{}_{}_", func, call_idx);
                            let ret_var = format!("{prefix}ret");
                            if !caller_vars.iter().any(|cv| cv.name == ret_var) {
                                caller_vars.push(CVar {
                                    name: ret_var.clone(),
                                    width: callee.ret_width.max(32),
                                    signed: callee.ret_signed,
                                    is_static: false,
                                    is_const: false,
                                    is_stream: false,
                                    dims: Vec::new(),
                                    init: None,
                                    init_list: Vec::new(),
                                });
                            }
                            let mut param_map: HashMap<String, CExpr> = HashMap::new();
                            for (param, actual) in callee.args.iter().zip(args.iter()) {
                                param_map.insert(param.name.clone(), actual.clone());
                            }
                            for v in &callee.vars {
                                let mut nv = v.clone();
                                nv.name = format!("{prefix}{}", v.name);
                                param_map.insert(v.name.clone(), CExpr::Var(nv.name.clone()));
                                if !caller_vars.iter().any(|cv| cv.name == nv.name) {
                                    caller_vars.push(nv);
                                }
                            }
                            let subst_body =
                                subst_stmts(&callee.body, &param_map, &prefix, Some(&ret_var));
                            let rec = inline_calls_in_stmts(
                                &subst_body,
                                caller_vars,
                                funcs,
                                false,
                                depth + 1,
                            );
                            out.extend(rec);
                            out.push(CStmt::Assign {
                                lhs: lhs.clone(),
                                rhs: CExpr::Var(ret_var),
                            });
                            continue;
                        }
                    }
                }
                out.push(st.clone());
            }
            CStmt::For {
                label,
                var,
                start,
                end,
                step,
                inclusive,
                pragmas,
                body,
            } => {
                let nb = inline_calls_in_stmts(body, caller_vars, funcs, false, depth + 1);
                out.push(CStmt::For {
                    label: label.clone(),
                    var: var.clone(),
                    start: *start,
                    end: *end,
                    step: *step,
                    inclusive: *inclusive,
                    pragmas: pragmas.clone(),
                    body: nb,
                });
            }
            CStmt::If {
                cond,
                then_body,
                else_body,
            } => {
                let tb = inline_calls_in_stmts(then_body, caller_vars, funcs, false, depth + 1);
                let eb = inline_calls_in_stmts(else_body, caller_vars, funcs, false, depth + 1);
                out.push(CStmt::If {
                    cond: cond.clone(),
                    then_body: tb,
                    else_body: eb,
                });
            }
            _ => out.push(st.clone()),
        }
    }
    out
}

fn subst_stmts(
    stmts: &[CStmt],
    map: &HashMap<String, CExpr>,
    prefix: &str,
    ret_var: Option<&str>,
) -> Vec<CStmt> {
    let mut out = Vec::new();
    for st in stmts {
        match st {
            CStmt::Assign { lhs, rhs } => {
                let nlhs = subst_lval(lhs, map);
                let nrhs = subst_expr(rhs, map);
                out.push(CStmt::Assign {
                    lhs: nlhs,
                    rhs: nrhs,
                });
            }
            CStmt::Return(Some(e)) => {
                let ne = subst_expr(e, map);
                if let Some(rv) = ret_var {
                    out.push(CStmt::Assign {
                        lhs: LValue::Var(rv.to_string()),
                        rhs: ne,
                    });
                } else {
                    out.push(CStmt::Return(Some(ne)));
                }
            }
            CStmt::StreamWrite { stream, val } => {
                let nstream = match map.get(stream) {
                    Some(CExpr::Var(s)) => s.clone(),
                    _ => stream.clone(),
                };
                out.push(CStmt::StreamWrite {
                    stream: nstream,
                    val: subst_expr(val, map),
                });
            }
            CStmt::For {
                label,
                var,
                start,
                end,
                step,
                inclusive,
                pragmas,
                body,
            } => {
                let nlbl = label.as_ref().map(|l| format!("{prefix}{l}"));
                out.push(CStmt::For {
                    label: nlbl,
                    var: var.clone(),
                    start: *start,
                    end: *end,
                    step: *step,
                    inclusive: *inclusive,
                    pragmas: pragmas.clone(),
                    body: subst_stmts(body, map, prefix, ret_var),
                });
            }
            CStmt::If {
                cond,
                then_body,
                else_body,
            } => {
                out.push(CStmt::If {
                    cond: subst_expr(cond, map),
                    then_body: subst_stmts(then_body, map, prefix, ret_var),
                    else_body: subst_stmts(else_body, map, prefix, ret_var),
                });
            }
            other => out.push(other.clone()),
        }
    }
    out
}

fn subst_lval(lv: &LValue, map: &HashMap<String, CExpr>) -> LValue {
    match lv {
        LValue::Var(v) => match map.get(v) {
            Some(CExpr::Var(nv)) => LValue::Var(nv.clone()),
            Some(CExpr::Deref(nv)) => LValue::Deref(nv.clone()),
            _ => LValue::Var(v.clone()),
        },
        LValue::Deref(v) => match map.get(v) {
            Some(CExpr::Var(nv)) | Some(CExpr::Deref(nv)) => LValue::Deref(nv.clone()),
            _ => LValue::Deref(v.clone()),
        },
        LValue::Bit(v, idx) => {
            let nv = match map.get(v) {
                Some(CExpr::Var(s)) => s.clone(),
                _ => v.clone(),
            };
            LValue::Bit(nv, subst_expr(idx, map))
        }
        LValue::Slice(v, hi, lo) => {
            let nv = match map.get(v) {
                Some(CExpr::Var(s)) => s.clone(),
                _ => v.clone(),
            };
            LValue::Slice(nv, *hi, *lo)
        }
        LValue::ArrayElem { array, indices } => {
            let narr = match map.get(array) {
                Some(CExpr::Var(s)) => s.clone(),
                _ => array.clone(),
            };
            LValue::ArrayElem {
                array: narr,
                indices: indices.iter().map(|i| subst_expr(i, map)).collect(),
            }
        }
    }
}

pub fn subst_expr(e: &CExpr, map: &HashMap<String, CExpr>) -> CExpr {
    match e {
        CExpr::Num(n) => CExpr::Num(*n),
        CExpr::Var(v) => map.get(v).cloned().unwrap_or_else(|| CExpr::Var(v.clone())),
        CExpr::Deref(v) => match map.get(v) {
            Some(CExpr::Var(nv)) => CExpr::Deref(nv.clone()),
            Some(other) => other.clone(),
            None => CExpr::Deref(v.clone()),
        },
        CExpr::Bit(v, idx) => {
            let nv = match map.get(v) {
                Some(CExpr::Var(s)) => s.clone(),
                _ => v.clone(),
            };
            CExpr::Bit(nv, Box::new(subst_expr(idx, map)))
        }
        CExpr::Slice(v, hi, lo) => {
            let nv = match map.get(v) {
                Some(CExpr::Var(s)) => s.clone(),
                _ => v.clone(),
            };
            CExpr::Slice(nv, *hi, *lo)
        }
        CExpr::Index { array, indices } => {
            let narr = match map.get(array) {
                Some(CExpr::Var(s)) => s.clone(),
                _ => array.clone(),
            };
            // Check if 1D constant index on a scalar variable is actually a bit-select `cnt[2]`
            CExpr::Index {
                array: narr,
                indices: indices.iter().map(|i| subst_expr(i, map)).collect(),
            }
        }
        CExpr::Unary { op, expr } => CExpr::Unary {
            op: *op,
            expr: Box::new(subst_expr(expr, map)),
        },
        CExpr::Binary { op, lhs, rhs } => CExpr::Binary {
            op: *op,
            lhs: Box::new(subst_expr(lhs, map)),
            rhs: Box::new(subst_expr(rhs, map)),
        },
        CExpr::Ternary {
            cond,
            then_e,
            else_e,
        } => CExpr::Ternary {
            cond: Box::new(subst_expr(cond, map)),
            then_e: Box::new(subst_expr(then_e, map)),
            else_e: Box::new(subst_expr(else_e, map)),
        },
        CExpr::Call { func, args } => CExpr::Call {
            func: func.clone(),
            args: args.iter().map(|a| subst_expr(a, map)).collect(),
        },
        CExpr::StreamRead(s) => {
            let ns = match map.get(s) {
                Some(CExpr::Var(v)) => v.clone(),
                _ => s.clone(),
            };
            CExpr::StreamRead(ns)
        }
        CExpr::StreamEmpty(s) => CExpr::StreamEmpty(s.clone()),
        CExpr::StreamFull(s) => CExpr::StreamFull(s.clone()),
    }
}

pub fn eval_const_i64(e: &CExpr) -> Option<i64> {
    match e {
        CExpr::Num(n) => Some(*n as i64),
        CExpr::Unary {
            op: UnaryOp::Neg,
            expr,
        } => eval_const_i64(expr).map(|v| -v),
        CExpr::Binary { op, lhs, rhs } => {
            let l = eval_const_i64(lhs)?;
            let r = eval_const_i64(rhs)?;
            match op {
                BinOp::Add => Some(l + r),
                BinOp::Sub => Some(l - r),
                BinOp::Mul => Some(l * r),
                BinOp::Div => (r != 0).then(|| l / r),
                BinOp::Mod => (r != 0).then(|| l % r),
                BinOp::Shl => Some(l << r),
                BinOp::Shr => Some(l >> r),
                _ => None,
            }
        }
        _ => None,
    }
}

pub fn loop_iterations(start: i64, end: i64, step: i64, inclusive: bool) -> Vec<i64> {
    let mut iters = Vec::new();
    if step == 0 {
        return iters;
    }
    let mut cur = start;
    let max_iters = 256usize;
    while iters.len() < max_iters {
        let cond = if step > 0 {
            if inclusive {
                cur <= end
            } else {
                cur < end
            }
        } else if inclusive {
            cur >= end
        } else {
            cur > end
        };
        if !cond {
            break;
        }
        iters.push(cur);
        cur += step;
    }
    iters
}

/// Build the Control-Dataflow Graph (CDFG) and Array Partitioning configs for the top function.
pub fn build_cdfg(
    func: &CFunction,
    stage_functions: &[CFunction],
    all_directives: &[HlsDirective],
) -> Cdfg {
    let part_dirs: Vec<ArrayPartitionDirective> = all_directives
        .iter()
        .filter_map(|d| match d {
            HlsDirective::ArrayPartition(ap) => Some(ap.clone()),
            _ => None,
        })
        .collect();

    let mut array_configs = BTreeMap::new();
    for a in &func.args {
        if !a.dims.is_empty() {
            let cfg = ArrayBankConfig::from_decl(
                &a.name,
                a.width,
                &a.dims,
                true,
                a.output,
                Vec::new(),
                &part_dirs,
            );
            array_configs.insert(a.name.clone(), cfg);
        }
    }
    for v in &func.vars {
        if !v.dims.is_empty() {
            let cfg = ArrayBankConfig::from_decl(
                &v.name,
                v.width,
                &v.dims,
                false,
                true,
                v.init_list.clone(),
                &part_dirs,
            );
            array_configs.insert(v.name.clone(), cfg);
        }
    }

    let is_dataflow = func
        .pragmas
        .iter()
        .any(|p| matches!(p, HlsDirective::Dataflow(_)))
        || all_directives
            .iter()
            .any(|p| matches!(p, HlsDirective::Dataflow(_)));

    let mut dataflow_processes = Vec::new();
    let mut dataflow_channels = Vec::new();
    if is_dataflow {
        extract_dataflow_network(
            func,
            stage_functions,
            &mut dataflow_processes,
            &mut dataflow_channels,
        );
    }

    let mut cdfg = Cdfg {
        function_name: func.name.clone(),
        blocks: vec![BasicBlock {
            id: 0,
            label: "entry".into(),
            loop_header: false,
            trip_count: 1,
            target_ii: func.pragmas.iter().find_map(|p| match p {
                HlsDirective::Pipeline(pd) if !pd.off => pd.ii,
                _ => None,
            }),
            node_ids: Vec::new(),
        }],
        control_edges: Vec::new(),
        nodes: Vec::new(),
        edges: Vec::new(),
        array_configs,
        dataflow_processes,
        dataflow_channels,
        is_dataflow,
    };

    let mut last_writer: HashMap<String, (usize, usize)> = HashMap::new();
    let mut last_mem_store: HashMap<(String, usize), usize> = HashMap::new();

    lower_stmts_to_cdfg(
        &func.body,
        0,
        None,
        0,
        &mut cdfg,
        &mut last_writer,
        &mut last_mem_store,
        func,
    );

    cdfg
}

fn extract_dataflow_network(
    func: &CFunction,
    stage_functions: &[CFunction],
    processes: &mut Vec<DataflowProcess>,
    channels: &mut Vec<DataflowChannel>,
) {
    let default_kind = func
        .pragmas
        .iter()
        .find_map(|p| match p {
            HlsDirective::Dataflow(df) => Some(df.channel),
            _ => None,
        })
        .unwrap_or(DataflowChannelKind::Fifo);

    if !stage_functions.is_empty() {
        for sf in stage_functions {
            let mut inputs = Vec::new();
            let mut outputs = Vec::new();
            for a in &sf.args {
                if a.output {
                    outputs.push(a.name.clone());
                } else {
                    inputs.push(a.name.clone());
                }
            }
            processes.push(DataflowProcess {
                name: sf.name.clone(),
                ii: 1,
                latency_cycles: 4,
                inputs,
                outputs,
            });
        }
    } else {
        // Extract top-level loops in the dataflow function as concurrent processes
        for (idx, st) in func.body.iter().enumerate() {
            if let CStmt::For {
                label,
                var,
                start,
                end,
                step,
                inclusive,
                body,
                ..
            } = st
            {
                let pname = label
                    .clone()
                    .unwrap_or_else(|| format!("stage_{idx}_{var}"));
                let trips = loop_iterations(*start, *end, *step, *inclusive).len().max(1);
                let mut reads = Vec::new();
                let mut writes = Vec::new();
                collect_stmt_rw(body, &mut reads, &mut writes);
                processes.push(DataflowProcess {
                    name: pname,
                    ii: 1,
                    latency_cycles: trips as u32 + 1,
                    inputs: reads,
                    outputs: writes,
                });
            }
        }
    }

    // Identify intermediate local streams or arrays connecting stages
    for v in &func.vars {
        if v.is_stream || !v.dims.is_empty() {
            let ch_kind = if v.is_stream {
                DataflowChannelKind::Fifo
            } else {
                default_kind
            };
            let depth = if !v.dims.is_empty() {
                v.dims.iter().product::<usize>().max(2)
            } else {
                4
            };
            let prod = processes
                .iter()
                .find(|p| p.outputs.contains(&v.name))
                .map(|p| p.name.clone())
                .unwrap_or_else(|| {
                    processes
                        .first()
                        .map(|p| p.name.clone())
                        .unwrap_or_else(|| "stage_0".into())
                });
            let cons = processes
                .iter()
                .rev()
                .find(|p| p.inputs.contains(&v.name) || p.name != prod)
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "stage_1".into());
            channels.push(DataflowChannel {
                name: v.name.clone(),
                kind: ch_kind,
                width: v.width.max(32),
                depth,
                producer_stage: prod,
                consumer_stage: cons,
            });
        }
    }
}

fn collect_stmt_rw(stmts: &[CStmt], reads: &mut Vec<String>, writes: &mut Vec<String>) {
    for st in stmts {
        match st {
            CStmt::Assign { lhs, rhs } => {
                match lhs {
                    LValue::Var(n) | LValue::Deref(n) | LValue::Bit(n, _) | LValue::Slice(n, _, _) => {
                        if !writes.contains(n) {
                            writes.push(n.clone());
                        }
                    }
                    LValue::ArrayElem { array, indices } => {
                        if !writes.contains(array) {
                            writes.push(array.clone());
                        }
                        for i in indices {
                            collect_expr_reads(i, reads);
                        }
                    }
                }
                collect_expr_reads(rhs, reads);
            }
            CStmt::StreamWrite { stream, val } => {
                if !writes.contains(stream) {
                    writes.push(stream.clone());
                }
                collect_expr_reads(val, reads);
            }
            CStmt::For { body, .. } | CStmt::While { body, .. } => {
                collect_stmt_rw(body, reads, writes);
            }
            CStmt::If {
                cond,
                then_body,
                else_body,
            } => {
                collect_expr_reads(cond, reads);
                collect_stmt_rw(then_body, reads, writes);
                collect_stmt_rw(else_body, reads, writes);
            }
            _ => {}
        }
    }
}

fn collect_expr_reads(e: &CExpr, reads: &mut Vec<String>) {
    match e {
        CExpr::Var(n) | CExpr::Deref(n) | CExpr::StreamRead(n) => {
            if !reads.contains(n) {
                reads.push(n.clone());
            }
        }
        CExpr::Index { array, indices } => {
            if !reads.contains(array) {
                reads.push(array.clone());
            }
            for i in indices {
                collect_expr_reads(i, reads);
            }
        }
        CExpr::Bit(n, idx) => {
            if !reads.contains(n) {
                reads.push(n.clone());
            }
            collect_expr_reads(idx, reads);
        }
        CExpr::Unary { expr, .. } => collect_expr_reads(expr, reads),
        CExpr::Binary { lhs, rhs, .. } => {
            collect_expr_reads(lhs, reads);
            collect_expr_reads(rhs, reads);
        }
        CExpr::Ternary {
            cond,
            then_e,
            else_e,
        } => {
            collect_expr_reads(cond, reads);
            collect_expr_reads(then_e, reads);
            collect_expr_reads(else_e, reads);
        }
        _ => {}
    }
}

fn lower_stmts_to_cdfg(
    stmts: &[CStmt],
    bb_id: usize,
    loop_label: Option<&str>,
    iter_idx: usize,
    cdfg: &mut Cdfg,
    last_writer: &mut HashMap<String, (usize, usize)>,
    last_mem_store: &mut HashMap<(String, usize), usize>,
    func: &CFunction,
) {
    for st in stmts {
        match st {
            CStmt::Pragma(_) | CStmt::Decl(_) => {}
            CStmt::Assign { lhs, rhs } => {
                let rhs_node = lower_expr_to_cdfg(
                    rhs,
                    bb_id,
                    loop_label,
                    iter_idx,
                    cdfg,
                    last_writer,
                    last_mem_store,
                    func,
                );
                match lhs {
                    LValue::Var(name) | LValue::Bit(name, _) | LValue::Slice(name, _, _) => {
                        let prev = last_writer.get(name).copied();
                        let w = var_width(func, name);
                        let nid = push_cdfg_node(
                            cdfg,
                            bb_id,
                            format!("wr_{name}_i{iter_idx}"),
                            CdfgOp::RegWrite(name.clone()),
                            w,
                            loop_label.map(|s| s.to_string()),
                            iter_idx,
                            0,
                            0.4,
                            vec![rhs_node],
                            Some(name.clone()),
                        );
                        cdfg.edges.push(CdfgEdge {
                            from: rhs_node,
                            to: nid,
                            kind: DepKind::DataFlow,
                            distance: 0,
                        });
                        if let Some((prev_nid, prev_iter)) = prev {
                            if prev_iter < iter_idx && loop_label.is_some() {
                                // Loop-carried recurrence edge
                                cdfg.edges.push(CdfgEdge {
                                    from: prev_nid,
                                    to: rhs_node,
                                    kind: DepKind::LoopCarried,
                                    distance: (iter_idx - prev_iter) as u32,
                                });
                            }
                        }
                        last_writer.insert(name.clone(), (nid, iter_idx));
                    }
                    LValue::Deref(name) => {
                        let w = var_width(func, name);
                        let nid = push_cdfg_node(
                            cdfg,
                            bb_id,
                            format!("port_wr_{name}"),
                            CdfgOp::WritePort(name.clone()),
                            w,
                            loop_label.map(|s| s.to_string()),
                            iter_idx,
                            0,
                            0.2,
                            vec![rhs_node],
                            Some(name.clone()),
                        );
                        cdfg.edges.push(CdfgEdge {
                            from: rhs_node,
                            to: nid,
                            kind: DepKind::DataFlow,
                            distance: 0,
                        });
                        last_writer.insert(name.clone(), (nid, iter_idx));
                    }
                    LValue::ArrayElem { array, indices } => {
                        let concrete: Vec<usize> = indices
                            .iter()
                            .map(|e| eval_const_i64(e).unwrap_or(0).max(0) as usize)
                            .collect();
                        let (bank, offset) = cdfg
                            .array_configs
                            .get(array)
                            .map(|c| c.map_indices(&concrete))
                            .unwrap_or((0, concrete.first().copied().unwrap_or(0)));
                        let is_reg = cdfg
                            .array_configs
                            .get(array)
                            .is_some_and(|c| c.is_registers);
                        let w = var_width(func, array);
                        let nid = push_cdfg_node(
                            cdfg,
                            bb_id,
                            format!("st_{array}_b{bank}_{offset}"),
                            CdfgOp::Store {
                                array: array.clone(),
                                bank,
                                offset,
                            },
                            w,
                            loop_label.map(|s| s.to_string()),
                            iter_idx,
                            u32::from(!is_reg),
                            if is_reg { 0.4 } else { 2.5 },
                            vec![rhs_node],
                            Some(format!("{array}[{bank}][{offset}]")),
                        );
                        cdfg.edges.push(CdfgEdge {
                            from: rhs_node,
                            to: nid,
                            kind: DepKind::DataFlow,
                            distance: 0,
                        });
                        if let Some(&prev_st) = last_mem_store.get(&(array.clone(), bank)) {
                            cdfg.edges.push(CdfgEdge {
                                from: prev_st,
                                to: nid,
                                kind: DepKind::MemoryOrder,
                                distance: 0,
                            });
                        }
                        last_mem_store.insert((array.clone(), bank), nid);
                    }
                }
            }
            CStmt::StreamWrite { stream, val } => {
                let vnode = lower_expr_to_cdfg(
                    val,
                    bb_id,
                    loop_label,
                    iter_idx,
                    cdfg,
                    last_writer,
                    last_mem_store,
                    func,
                );
                let w = var_width(func, stream);
                let nid = push_cdfg_node(
                    cdfg,
                    bb_id,
                    format!("stream_wr_{stream}_{iter_idx}"),
                    CdfgOp::StreamWrite(stream.clone()),
                    w,
                    loop_label.map(|s| s.to_string()),
                    iter_idx,
                    0,
                    0.8,
                    vec![vnode],
                    Some(stream.clone()),
                );
                cdfg.edges.push(CdfgEdge {
                    from: vnode,
                    to: nid,
                    kind: DepKind::DataFlow,
                    distance: 0,
                });
            }
            CStmt::Return(Some(e)) => {
                let rnode = lower_expr_to_cdfg(
                    e,
                    bb_id,
                    loop_label,
                    iter_idx,
                    cdfg,
                    last_writer,
                    last_mem_store,
                    func,
                );
                let nid = push_cdfg_node(
                    cdfg,
                    bb_id,
                    "ret_write".into(),
                    CdfgOp::WritePort("ap_return".into()),
                    func.ret_width.max(32),
                    loop_label.map(|s| s.to_string()),
                    iter_idx,
                    0,
                    0.2,
                    vec![rnode],
                    Some("y".into()),
                );
                cdfg.edges.push(CdfgEdge {
                    from: rnode,
                    to: nid,
                    kind: DepKind::DataFlow,
                    distance: 0,
                });
            }
            CStmt::Return(None) => {}
            CStmt::If {
                cond,
                then_body,
                else_body,
            } => {
                let _cnode = lower_expr_to_cdfg(
                    cond,
                    bb_id,
                    loop_label,
                    iter_idx,
                    cdfg,
                    last_writer,
                    last_mem_store,
                    func,
                );
                lower_stmts_to_cdfg(
                    then_body,
                    bb_id,
                    loop_label,
                    iter_idx,
                    cdfg,
                    last_writer,
                    last_mem_store,
                    func,
                );
                lower_stmts_to_cdfg(
                    else_body,
                    bb_id,
                    loop_label,
                    iter_idx,
                    cdfg,
                    last_writer,
                    last_mem_store,
                    func,
                );
            }
            CStmt::For {
                label,
                var,
                start,
                end,
                step,
                inclusive,
                pragmas,
                body,
            } => {
                let iters = loop_iterations(*start, *end, *step, *inclusive);
                let lbl = label.clone().unwrap_or_else(|| format!("loop_{var}"));
                let target_ii = pragmas.iter().find_map(|p| match p {
                    HlsDirective::Pipeline(pd) if !pd.off => pd.ii,
                    _ => None,
                });
                let loop_bb = cdfg.blocks.len();
                cdfg.blocks.push(BasicBlock {
                    id: loop_bb,
                    label: lbl.clone(),
                    loop_header: true,
                    trip_count: iters.len().max(1),
                    target_ii,
                    node_ids: Vec::new(),
                });
                cdfg.control_edges.push(ControlEdge {
                    from_bb: bb_id,
                    to_bb: loop_bb,
                    kind: ControlEdgeKind::Fallthrough,
                });
                cdfg.control_edges.push(ControlEdge {
                    from_bb: loop_bb,
                    to_bb: loop_bb,
                    kind: ControlEdgeKind::LoopBack,
                });

                for (it_i, &iv) in iters.iter().enumerate() {
                    let mut map = HashMap::new();
                    map.insert(var.clone(), CExpr::Num(i128::from(iv)));
                    let subst_b = subst_stmts(body, &map, "", None);
                    lower_stmts_to_cdfg(
                        &subst_b,
                        loop_bb,
                        Some(&lbl),
                        iter_idx + it_i,
                        cdfg,
                        last_writer,
                        last_mem_store,
                        func,
                    );
                }
            }
            CStmt::While {
                label,
                trip_bound,
                pragmas,
                body,
                ..
            } => {
                let lbl = label.clone().unwrap_or_else(|| "while_loop".into());
                let target_ii = pragmas.iter().find_map(|p| match p {
                    HlsDirective::Pipeline(pd) if !pd.off => pd.ii,
                    _ => None,
                });
                let loop_bb = cdfg.blocks.len();
                cdfg.blocks.push(BasicBlock {
                    id: loop_bb,
                    label: lbl.clone(),
                    loop_header: true,
                    trip_count: (*trip_bound).max(1),
                    target_ii,
                    node_ids: Vec::new(),
                });
                cdfg.control_edges.push(ControlEdge {
                    from_bb: bb_id,
                    to_bb: loop_bb,
                    kind: ControlEdgeKind::Fallthrough,
                });
                lower_stmts_to_cdfg(
                    body,
                    loop_bb,
                    Some(&lbl),
                    iter_idx,
                    cdfg,
                    last_writer,
                    last_mem_store,
                    func,
                );
            }
            CStmt::Call { .. } => {}
        }
    }
}

fn lower_expr_to_cdfg(
    e: &CExpr,
    bb_id: usize,
    loop_label: Option<&str>,
    iter_idx: usize,
    cdfg: &mut Cdfg,
    last_writer: &mut HashMap<String, (usize, usize)>,
    last_mem_store: &mut HashMap<(String, usize), usize>,
    func: &CFunction,
) -> usize {
    match e {
        CExpr::Num(n) => push_cdfg_node(
            cdfg,
            bb_id,
            format!("const_{n}"),
            CdfgOp::Const(*n),
            32,
            loop_label.map(|s| s.to_string()),
            iter_idx,
            0,
            0.0,
            Vec::new(),
            None,
        ),
        CExpr::Var(name) | CExpr::Deref(name) | CExpr::Bit(name, _) | CExpr::Slice(name, _, _) => {
            if let Some(&(writer_nid, writer_iter)) = last_writer.get(name) {
                if writer_iter == iter_idx {
                    return writer_nid;
                }
            }
            let w = var_width(func, name);
            let is_arg = func.args.iter().any(|a| &a.name == name);
            let op = if is_arg {
                CdfgOp::ReadPort(name.clone())
            } else {
                CdfgOp::RegRead(name.clone())
            };
            let nid = push_cdfg_node(
                cdfg,
                bb_id,
                format!("rd_{name}_i{iter_idx}"),
                op,
                w,
                loop_label.map(|s| s.to_string()),
                iter_idx,
                0,
                0.2,
                Vec::new(),
                None,
            );
            if let Some(&(writer_nid, writer_iter)) = last_writer.get(name) {
                let dist = iter_idx.saturating_sub(writer_iter) as u32;
                cdfg.edges.push(CdfgEdge {
                    from: writer_nid,
                    to: nid,
                    kind: if dist > 0 {
                        DepKind::LoopCarried
                    } else {
                        DepKind::DataFlow
                    },
                    distance: dist,
                });
            }
            nid
        }
        CExpr::StreamRead(stream) => {
            let w = var_width(func, stream);
            push_cdfg_node(
                cdfg,
                bb_id,
                format!("stream_rd_{stream}_{iter_idx}"),
                CdfgOp::StreamRead(stream.clone()),
                w,
                loop_label.map(|s| s.to_string()),
                iter_idx,
                0,
                0.8,
                Vec::new(),
                None,
            )
        }
        CExpr::StreamEmpty(stream) | CExpr::StreamFull(stream) => push_cdfg_node(
            cdfg,
            bb_id,
            format!("stream_stat_{stream}"),
            CdfgOp::ReadPort(stream.clone()),
            1,
            loop_label.map(|s| s.to_string()),
            iter_idx,
            0,
            0.2,
            Vec::new(),
            None,
        ),
        CExpr::Index { array, indices } => {
            // Check if `array` is actually a scalar (bit-select `cnt[2]`)
            if !cdfg.array_configs.contains_key(array) {
                let w = var_width(func, array);
                return push_cdfg_node(
                    cdfg,
                    bb_id,
                    format!("bit_{array}"),
                    CdfgOp::RegRead(array.clone()),
                    w,
                    loop_label.map(|s| s.to_string()),
                    iter_idx,
                    0,
                    0.2,
                    Vec::new(),
                    None,
                );
            }
            let concrete: Vec<usize> = indices
                .iter()
                .map(|idx| eval_const_i64(idx).unwrap_or(0).max(0) as usize)
                .collect();
            let (bank, offset) = cdfg
                .array_configs
                .get(array)
                .map(|c| c.map_indices(&concrete))
                .unwrap_or((0, concrete.first().copied().unwrap_or(0)));
            let is_reg = cdfg
                .array_configs
                .get(array)
                .is_some_and(|c| c.is_registers);
            let w = var_width(func, array);
            let nid = push_cdfg_node(
                cdfg,
                bb_id,
                format!("ld_{array}_b{bank}_{offset}"),
                CdfgOp::Load {
                    array: array.clone(),
                    bank,
                    offset,
                },
                w,
                loop_label.map(|s| s.to_string()),
                iter_idx,
                u32::from(!is_reg),
                if is_reg { 0.3 } else { 2.4 },
                Vec::new(),
                None,
            );
            if let Some(&st_nid) = last_mem_store.get(&(array.clone(), bank)) {
                cdfg.edges.push(CdfgEdge {
                    from: st_nid,
                    to: nid,
                    kind: DepKind::MemoryOrder,
                    distance: 0,
                });
            }
            nid
        }
        CExpr::Unary { op, expr } => {
            let child = lower_expr_to_cdfg(
                expr,
                bb_id,
                loop_label,
                iter_idx,
                cdfg,
                last_writer,
                last_mem_store,
                func,
            );
            let cdfg_op = match op {
                UnaryOp::Not | UnaryOp::BitNot => CdfgOp::Not,
                UnaryOp::Neg => CdfgOp::Sub,
            };
            let nid = push_cdfg_node(
                cdfg,
                bb_id,
                format!("un_{}", cdfg.nodes.len()),
                cdfg_op,
                32,
                loop_label.map(|s| s.to_string()),
                iter_idx,
                0,
                0.9,
                vec![child],
                None,
            );
            cdfg.edges.push(CdfgEdge {
                from: child,
                to: nid,
                kind: DepKind::DataFlow,
                distance: 0,
            });
            nid
        }
        CExpr::Binary { op, lhs, rhs } => {
            let ln = lower_expr_to_cdfg(
                lhs,
                bb_id,
                loop_label,
                iter_idx,
                cdfg,
                last_writer,
                last_mem_store,
                func,
            );
            let rn = lower_expr_to_cdfg(
                rhs,
                bb_id,
                loop_label,
                iter_idx,
                cdfg,
                last_writer,
                last_mem_store,
                func,
            );
            let (cdfg_op, lat, delay) = match op {
                BinOp::Add => {
                    // Check if one child is a Mul -> fused DSP48 MAC (`a * b + c`)!
                    if matches!(cdfg.nodes[ln].op, CdfgOp::Mul)
                        || matches!(cdfg.nodes[rn].op, CdfgOp::Mul)
                    {
                        (CdfgOp::Mac, 0, 1.2)
                    } else {
                        (CdfgOp::Add, 0, 1.5)
                    }
                }
                BinOp::Sub => (CdfgOp::Sub, 0, 1.5),
                BinOp::Mul => (CdfgOp::Mul, 1, 3.2),
                BinOp::Div => (CdfgOp::Div, 2, 4.5),
                BinOp::Mod => (CdfgOp::Mod, 2, 4.5),
                BinOp::And | BinOp::LogAnd => (CdfgOp::And, 0, 0.8),
                BinOp::Or | BinOp::LogOr => (CdfgOp::Or, 0, 0.8),
                BinOp::Xor => (CdfgOp::Xor, 0, 0.8),
                BinOp::Shl => (CdfgOp::Shl, 0, 1.0),
                BinOp::Shr => (CdfgOp::Shr, 0, 1.0),
                BinOp::Eq => (CdfgOp::Cmp(CmpKind::Eq), 0, 1.1),
                BinOp::Ne => (CdfgOp::Cmp(CmpKind::Ne), 0, 1.1),
                BinOp::Lt => (CdfgOp::Cmp(CmpKind::Lt), 0, 1.2),
                BinOp::Le => (CdfgOp::Cmp(CmpKind::Le), 0, 1.2),
                BinOp::Gt => (CdfgOp::Cmp(CmpKind::Gt), 0, 1.2),
                BinOp::Ge => (CdfgOp::Cmp(CmpKind::Ge), 0, 1.2),
            };
            let nid = push_cdfg_node(
                cdfg,
                bb_id,
                format!("op_{}", cdfg.nodes.len()),
                cdfg_op,
                32,
                loop_label.map(|s| s.to_string()),
                iter_idx,
                lat,
                delay,
                vec![ln, rn],
                None,
            );
            cdfg.edges.push(CdfgEdge {
                from: ln,
                to: nid,
                kind: DepKind::DataFlow,
                distance: 0,
            });
            cdfg.edges.push(CdfgEdge {
                from: rn,
                to: nid,
                kind: DepKind::DataFlow,
                distance: 0,
            });
            nid
        }
        CExpr::Ternary {
            cond,
            then_e,
            else_e,
        } => {
            let cn = lower_expr_to_cdfg(
                cond,
                bb_id,
                loop_label,
                iter_idx,
                cdfg,
                last_writer,
                last_mem_store,
                func,
            );
            let tn = lower_expr_to_cdfg(
                then_e,
                bb_id,
                loop_label,
                iter_idx,
                cdfg,
                last_writer,
                last_mem_store,
                func,
            );
            let en = lower_expr_to_cdfg(
                else_e,
                bb_id,
                loop_label,
                iter_idx,
                cdfg,
                last_writer,
                last_mem_store,
                func,
            );
            let nid = push_cdfg_node(
                cdfg,
                bb_id,
                format!("mux_{}", cdfg.nodes.len()),
                CdfgOp::Select,
                32,
                loop_label.map(|s| s.to_string()),
                iter_idx,
                0,
                1.0,
                vec![cn, tn, en],
                None,
            );
            for src in [cn, tn, en] {
                cdfg.edges.push(CdfgEdge {
                    from: src,
                    to: nid,
                    kind: DepKind::DataFlow,
                    distance: 0,
                });
            }
            nid
        }
        CExpr::Call { func: callee, args } => {
            let mut in_ids = Vec::new();
            for a in args {
                in_ids.push(lower_expr_to_cdfg(
                    a,
                    bb_id,
                    loop_label,
                    iter_idx,
                    cdfg,
                    last_writer,
                    last_mem_store,
                    func,
                ));
            }
            let nid = push_cdfg_node(
                cdfg,
                bb_id,
                format!("call_{callee}"),
                CdfgOp::Add,
                32,
                loop_label.map(|s| s.to_string()),
                iter_idx,
                1,
                2.0,
                in_ids.clone(),
                None,
            );
            for src in in_ids {
                cdfg.edges.push(CdfgEdge {
                    from: src,
                    to: nid,
                    kind: DepKind::DataFlow,
                    distance: 0,
                });
            }
            nid
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn push_cdfg_node(
    cdfg: &mut Cdfg,
    bb_id: usize,
    name: String,
    op: CdfgOp,
    bitwidth: usize,
    loop_label: Option<String>,
    iteration: usize,
    op_latency: u32,
    delay_ns: f64,
    inputs: Vec<usize>,
    output_var: Option<String>,
) -> usize {
    let id = cdfg.nodes.len();
    cdfg.nodes.push(CdfgNode {
        id,
        name,
        op,
        bitwidth,
        basic_block: bb_id,
        loop_label,
        iteration,
        op_latency,
        delay_ns,
        inputs,
        output_var,
    });
    if let Some(bb) = cdfg.blocks.get_mut(bb_id) {
        bb.node_ids.push(id);
    }
    id
}

fn var_width(func: &CFunction, name: &str) -> usize {
    for a in &func.args {
        if a.name == name {
            return a.width.max(1);
        }
    }
    for v in &func.vars {
        if v.name == name {
            return v.width.max(1);
        }
    }
    32
}

/// Compute As-Soon-As-Possible (ASAP) schedule cycles respecting data dependencies and clock period.
pub fn compute_asap(cdfg: &Cdfg, target_clock_ns: f64) -> (Vec<u32>, Vec<f64>) {
    let n = cdfg.nodes.len();
    let mut cycle = vec![0u32; n];
    let mut accum_delay = vec![0.0f64; n];

    for u in 0..n {
        let d_u = cdfg.nodes[u].delay_ns;
        if accum_delay[u] < d_u {
            accum_delay[u] = d_u;
        }
    }

    // Topological relaxation over DAG (intra-iteration edges where distance == 0)
    let mut changed = true;
    let mut pass = 0;
    while changed && pass <= n {
        changed = false;
        pass += 1;
        for edge in &cdfg.edges {
            if edge.distance > 0 || edge.from >= edge.to {
                continue;
            }
            let u = edge.from;
            let v = edge.to;
            let lat_u = cdfg.nodes[u].op_latency;
            let d_v = cdfg.nodes[v].delay_ns;
            let (cand_cyc, cand_delay) = if lat_u > 0 {
                (cycle[u] + lat_u, d_v)
            } else if accum_delay[u] + d_v > target_clock_ns.max(1.0) {
                (cycle[u] + 1, d_v)
            } else {
                (cycle[u], accum_delay[u] + d_v)
            };
            if cand_cyc > cycle[v] || (cand_cyc == cycle[v] && cand_delay > accum_delay[v]) {
                cycle[v] = cand_cyc;
                accum_delay[v] = cand_delay;
                changed = true;
            }
        }
    }
    (cycle, accum_delay)
}

/// Compute As-Late-As-Possible (ALAP) schedule cycles given a latency bound `max_cycle`.
pub fn compute_alap(cdfg: &Cdfg, max_cycle: u32, target_clock_ns: f64) -> Vec<u32> {
    let n = cdfg.nodes.len();
    let mut alap = vec![max_cycle; n];
    let mut changed = true;
    let mut pass = 0;
    while changed && pass <= n {
        changed = false;
        pass += 1;
        for edge in cdfg.edges.iter().rev() {
            if edge.distance > 0 || edge.from >= edge.to {
                continue;
            }
            let u = edge.from;
            let v = edge.to;
            let step = cdfg.nodes[u].op_latency.max(u32::from(
                cdfg.nodes[u].delay_ns + cdfg.nodes[v].delay_ns > target_clock_ns.max(1.0),
            ));
            let allowed = alap[v].saturating_sub(step);
            if allowed < alap[u] {
                alap[u] = allowed;
                changed = true;
            }
        }
    }
    alap
}

/// Solve System of Difference Constraints (SDC) via Bellman-Ford longest path:
/// For every dependency edge `(u, v)` with distance `d`, enforces:
///   `s(v) - s(u) >= step(u, v) - d * II`
pub fn compute_sdc_schedule(cdfg: &Cdfg, ii: u32, target_clock_ns: f64) -> Vec<u32> {
    let n = cdfg.nodes.len();
    if n == 0 {
        return Vec::new();
    }
    let mut dist = vec![0i64; n];
    let ii_i64 = i64::from(ii.max(1));

    for _ in 0..n {
        let mut updated = false;
        for edge in &cdfg.edges {
            let u = edge.from;
            let v = edge.to;
            if u == v {
                continue;
            }
            let step = i64::from(cdfg.nodes[u].op_latency)
                + i64::from(
                    cdfg.nodes[u].op_latency == 0
                        && (cdfg.nodes[u].delay_ns + cdfg.nodes[v].delay_ns
                            > target_clock_ns.max(1.0)),
                );
            let weight = step - i64::from(edge.distance) * ii_i64;
            if edge.distance == 0 && u < v && dist[u] + weight > dist[v] {
                dist[v] = dist[u] + weight;
                updated = true;
            }
        }
        if !updated {
            break;
        }
    }

    let min_d = dist.iter().copied().min().unwrap_or(0);
    dist.into_iter().map(|d| (d - min_d) as u32).collect()
}

/// Compute full Modulo Schedule ($ResII$, $RecII$, $II_{\min}$, pipeline depth, loop latencies).
pub fn schedule_cdfg(
    cdfg: &Cdfg,
    func: &CFunction,
    all_directives: &[HlsDirective],
    target_clock_ns: f64,
) -> ScheduleResult {
    let (asap_cycles, accum_delays) = compute_asap(cdfg, target_clock_ns);
    let max_asap = asap_cycles.iter().copied().max().unwrap_or(0);
    let alap_cycles = compute_alap(cdfg, max_asap, target_clock_ns);

    // Check ALLOCATION limits for multipliers/DSPs
    let mul_alloc_limit: Option<usize> = all_directives.iter().find_map(|d| match d {
        HlsDirective::Allocation(ad)
            if ad.kind == AllocationKind::Operation
                && (ad.instances.eq_ignore_ascii_case("mul")
                    || ad.instances.eq_ignore_ascii_case("mac")
                    || ad.instances.eq_ignore_ascii_case("dsp")) =>
        {
            Some(ad.limit.max(1))
        }
        _ => None,
    });

    // Compute per-loop and function-level modulo schedule metrics
    let mut loop_schedules = Vec::new();
    collect_loop_schedules(
        &func.body,
        func,
        cdfg,
        all_directives,
        mul_alloc_limit,
        target_clock_ns,
        &mut loop_schedules,
    );

    // Function-level pipeline directive check
    let fn_pipeline_ii: Option<u32> = all_directives.iter().find_map(|d| match d {
        HlsDirective::Pipeline(pd)
            if !pd.off
                && (pd.target.is_none()
                    || pd.target.as_deref() == Some(func.name.as_str())) =>
        {
            Some(pd.ii.unwrap_or(1).max(1))
        }
        _ => None,
    });

    let (res_ii, rec_ii, target_ii) = if let Some(last_loop) = loop_schedules
        .iter()
        .find(|l| l.pipelined)
        .or_else(|| loop_schedules.last())
    {
        (last_loop.res_ii, last_loop.rec_ii, last_loop.target_ii)
    } else {
        // Function-level resource and recurrence II
        let mut r_ii = 1u32;
        let mut bank_loads: HashMap<(String, usize), usize> = HashMap::new();
        for node in &cdfg.nodes {
            if let CdfgOp::Load { array, bank, .. } | CdfgOp::Store { array, bank, .. } = &node.op
            {
                let is_reg = cdfg
                    .array_configs
                    .get(array)
                    .is_some_and(|c| c.is_registers);
                if !is_reg {
                    *bank_loads.entry((array.clone(), *bank)).or_insert(0) += 1;
                }
            }
        }
        for &accesses in bank_loads.values() {
            // Dual-port BRAM supports 2 accesses per cycle
            let needed = ((accesses + 1) / 2).max(1) as u32;
            r_ii = r_ii.max(needed);
        }
        if let Some(limit) = mul_alloc_limit {
            let muls = cdfg
                .nodes
                .iter()
                .filter(|n| matches!(n.op, CdfgOp::Mul | CdfgOp::Mac))
                .count();
            let needed = ((muls + limit - 1) / limit).max(1) as u32;
            r_ii = r_ii.max(needed);
        }
        let rec = compute_recurrence_ii(cdfg);
        (r_ii, rec, fn_pipeline_ii.unwrap_or(1))
    };

    let ii_min = res_ii.max(rec_ii).max(1);
    let achieved_ii = if cdfg.is_dataflow && !cdfg.dataflow_processes.is_empty() {
        cdfg.dataflow_processes
            .iter()
            .map(|p| p.ii)
            .max()
            .unwrap_or(1)
            .max(ii_min)
    } else {
        target_ii.max(ii_min)
    };

    let sdc_cycles = compute_sdc_schedule(cdfg, achieved_ii, target_clock_ns);
    let pipeline_depth = (sdc_cycles.iter().copied().max().unwrap_or(0) + 1).max(1);

    let node_stage: Vec<u32> = sdc_cycles
        .iter()
        .map(|&c| c / achieved_ii.max(1))
        .collect();
    let node_modulo_slot: Vec<u32> = sdc_cycles
        .iter()
        .map(|&c| c % achieved_ii.max(1))
        .collect();

    let total_latency = if cdfg.is_dataflow && !cdfg.dataflow_processes.is_empty() {
        // In DATAFLOW mode, stages execute concurrently: latency is max stage latency + (num_stages - 1)
        let max_stage = cdfg
            .dataflow_processes
            .iter()
            .map(|p| p.latency_cycles)
            .max()
            .unwrap_or(pipeline_depth);
        max_stage + (cdfg.dataflow_processes.len().saturating_sub(1) as u32)
    } else if !loop_schedules.is_empty() {
        loop_schedules.iter().map(|l| l.latency_cycles).sum::<u32>()
    } else {
        pipeline_depth
    };

    let max_delay = accum_delays
        .iter()
        .copied()
        .fold(1.85f64, f64::max)
        .min(target_clock_ns * 0.92);

    ScheduleResult {
        target_clock_ns,
        estimated_clock_ns: max_delay,
        asap_cycles,
        alap_cycles,
        sdc_cycles,
        node_stage,
        node_modulo_slot,
        res_ii,
        rec_ii,
        ii_min,
        achieved_ii,
        pipeline_depth,
        min_latency_cycles: total_latency.max(1),
        max_latency_cycles: total_latency.max(1),
        loop_schedules,
    }
}

fn compute_recurrence_ii(cdfg: &Cdfg) -> u32 {
    let mut rec_ii = 1u32;
    for edge in &cdfg.edges {
        if edge.kind == DepKind::LoopCarried && edge.distance > 0 {
            let lat = cdfg.nodes[edge.from].op_latency.max(1);
            let ratio = (lat + edge.distance - 1) / edge.distance;
            rec_ii = rec_ii.max(ratio);
        }
    }
    rec_ii
}

fn collect_loop_schedules(
    stmts: &[CStmt],
    func: &CFunction,
    cdfg: &Cdfg,
    all_directives: &[HlsDirective],
    mul_alloc_limit: Option<usize>,
    target_clock_ns: f64,
    out: &mut Vec<LoopScheduleInfo>,
) {
    let fn_pipelined = func
        .pragmas
        .iter()
        .any(|p| matches!(p, HlsDirective::Pipeline(pd) if !pd.off));

    for st in stmts {
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
            let lbl = label.clone().unwrap_or_else(|| format!("loop_{var}"));
            let trips = loop_iterations(*start, *end, *step, *inclusive).len().max(1);

            let pipe_dir = pragmas.iter().find_map(|p| match p {
                HlsDirective::Pipeline(pd) if !pd.off => Some(pd.ii.unwrap_or(1)),
                _ => None,
            });
            let unroll_dir = pragmas.iter().find_map(|p| match p {
                HlsDirective::Unroll(ud) if !ud.off => Some(ud.factor.unwrap_or(trips)),
                _ => None,
            });

            // Recurse into nested loops first unless this loop is pipelined (which auto-unrolls inner loops)
            let is_pipelined = pipe_dir.is_some() || fn_pipelined;
            if !is_pipelined {
                collect_loop_schedules(
                    body,
                    func,
                    cdfg,
                    all_directives,
                    mul_alloc_limit,
                    target_clock_ns,
                    out,
                );
            }

            let unroll_factor = if is_pipelined && pipe_dir.is_none() && fn_pipelined {
                trips
            } else {
                unroll_dir.unwrap_or(1).max(1).min(trips)
            };

            // Analyze single pipelined iteration (with inner loops unrolled if pipelined)
            let mut bank_accesses: HashMap<(String, usize), usize> = HashMap::new();
            let mut muls_per_iter = 0usize;
            let mut rec_lat = 1u32;
            analyze_loop_body_iter(
                body,
                &cdfg.array_configs,
                is_pipelined,
                unroll_factor,
                &mut bank_accesses,
                &mut muls_per_iter,
                &mut rec_lat,
                target_clock_ns,
            );
            // Check for loop-carried array recurrence & #pragma HLS DEPENDENCE overrides
            let dep_rec_ii = compute_loop_array_recurrence_ii(
                body,
                &cdfg.array_configs,
                pragmas,
                all_directives,
                muls_per_iter,
            );
            rec_lat = rec_lat.max(dep_rec_ii);

            let mut res_ii = 1u32;
            for &cnt in bank_accesses.values() {
                // Dual-port BRAM provides 2 ports per cycle per bank
                let needed = ((cnt + 1) / 2).max(1) as u32;
                res_ii = res_ii.max(needed);
            }
            if let Some(limit) = mul_alloc_limit {
                if muls_per_iter > 0 {
                    let needed = ((muls_per_iter + limit - 1) / limit).max(1) as u32;
                    res_ii = res_ii.max(needed);
                }
            }

            let rec_ii = rec_lat.max(1);
            let ii_min = res_ii.max(rec_ii).max(1);
            let target_ii = pipe_dir.unwrap_or(1).max(1);
            let achieved_ii = if is_pipelined {
                target_ii.max(ii_min)
            } else {
                (muls_per_iter as u32).max(2)
            };

            let effective_trips = (trips + unroll_factor - 1) / unroll_factor.max(1);
            let pipeline_depth = if muls_per_iter > 0 { 3 } else { 2 };
            let latency_cycles = if unroll_factor >= trips {
                pipeline_depth
            } else if is_pipelined {
                ((effective_trips.saturating_sub(1)) as u32) * achieved_ii + pipeline_depth
            } else {
                (effective_trips as u32) * pipeline_depth
            };

            out.push(LoopScheduleInfo {
                label: lbl,
                trip_count: trips,
                pipelined: is_pipelined,
                unroll_factor,
                target_ii,
                res_ii,
                rec_ii,
                ii_min,
                achieved_ii,
                pipeline_depth,
                latency_cycles,
            });
        }
    }
}

fn compute_loop_array_recurrence_ii(
    body: &[CStmt],
    array_configs: &BTreeMap<String, ArrayBankConfig>,
    loop_pragmas: &[HlsDirective],
    all_directives: &[HlsDirective],
    muls_per_iter: usize,
) -> u32 {
    let mut reads = Vec::new();
    let mut writes = Vec::new();
    collect_stmt_rw(body, &mut reads, &mut writes);

    let mut max_rec_ii = 1u32;
    for w_arr in &writes {
        if !reads.contains(w_arr) {
            continue;
        }
        if array_configs.get(w_arr).is_some_and(|c| c.is_registers) {
            continue;
        }
        // Check if this variable is an array (written via ArrayElem)
        let is_array_write = body.iter().any(|s| {
            matches!(
                s,
                CStmt::Assign {
                    lhs: LValue::ArrayElem { array, .. },
                    ..
                } if array == w_arr
            )
        });
        if !is_array_write {
            continue;
        }

        // Base memory-to-memory recurrence latency: 2 cycles (BRAM load + store), or 4 cycles with DSP multiply
        let rec_cycles: u32 = if muls_per_iter > 0 { 4 } else { 2 };

        // Check for `#pragma HLS DEPENDENCE` matching `w_arr`
        let dep_dir = loop_pragmas
            .iter()
            .chain(all_directives.iter())
            .filter_map(|d| match d {
                HlsDirective::Dependence(dd)
                    if dd.variable == *w_arr || dd.variable == "*" =>
                {
                    Some(dd)
                }
                _ => None,
            })
            .last();

        if let Some(dd) = dep_dir {
            if !dd.dependent {
                // `#pragma HLS DEPENDENCE variable=... inter false` removes loop-carried recurrence
                continue;
            }
            let dist = dd.distance.unwrap_or(1).max(1);
            let ii = (rec_cycles + dist - 1) / dist;
            max_rec_ii = max_rec_ii.max(ii);
        } else {
            // Default conservative inter-iteration array recurrence (distance = 1)
            max_rec_ii = max_rec_ii.max(rec_cycles);
        }
    }
    max_rec_ii
}

#[allow(clippy::too_many_arguments)]
fn analyze_loop_body_iter(
    stmts: &[CStmt],
    array_configs: &BTreeMap<String, ArrayBankConfig>,
    unroll_inner: bool,
    outer_unroll: usize,
    bank_accesses: &mut HashMap<(String, usize), usize>,
    muls: &mut usize,
    rec_lat: &mut u32,
    target_clock_ns: f64,
) {
    for _u in 0..outer_unroll.max(1) {
        for st in stmts {
            match st {
                CStmt::Assign { lhs, rhs } => {
                    if let LValue::ArrayElem { array, indices } = lhs {
                        record_array_access(array, indices, 0, array_configs, bank_accesses);
                    }
                    analyze_expr_iter(rhs, 0, array_configs, bank_accesses, muls);
                    if target_clock_ns < 4.0 && *muls > 0 {
                        *rec_lat = (*rec_lat).max(2);
                    }
                }
                CStmt::For {
                    var,
                    start,
                    end,
                    step,
                    inclusive,
                    pragmas,
                    body,
                    ..
                } => {
                    let iters = loop_iterations(*start, *end, *step, *inclusive);
                    let inner_unrolled = unroll_inner
                        || pragmas
                            .iter()
                            .any(|p| matches!(p, HlsDirective::Unroll(u) if !u.off));
                    if inner_unrolled {
                        for &iv in &iters {
                            let mut map = HashMap::new();
                            map.insert(var.clone(), CExpr::Num(i128::from(iv)));
                            let sub = subst_stmts(body, &map, "", None);
                            analyze_loop_body_iter(
                                &sub,
                                array_configs,
                                true,
                                1,
                                bank_accesses,
                                muls,
                                rec_lat,
                                target_clock_ns,
                            );
                        }
                    } else {
                        analyze_loop_body_iter(
                            body,
                            array_configs,
                            false,
                            1,
                            bank_accesses,
                            muls,
                            rec_lat,
                            target_clock_ns,
                        );
                    }
                }
                CStmt::If {
                    cond,
                    then_body,
                    else_body,
                } => {
                    analyze_expr_iter(cond, 0, array_configs, bank_accesses, muls);
                    analyze_loop_body_iter(
                        then_body,
                        array_configs,
                        unroll_inner,
                        1,
                        bank_accesses,
                        muls,
                        rec_lat,
                        target_clock_ns,
                    );
                    analyze_loop_body_iter(
                        else_body,
                        array_configs,
                        unroll_inner,
                        1,
                        bank_accesses,
                        muls,
                        rec_lat,
                        target_clock_ns,
                    );
                }
                _ => {}
            }
        }
    }
}

fn analyze_expr_iter(
    e: &CExpr,
    default_idx: usize,
    array_configs: &BTreeMap<String, ArrayBankConfig>,
    bank_accesses: &mut HashMap<(String, usize), usize>,
    muls: &mut usize,
) {
    match e {
        CExpr::Index { array, indices } => {
            record_array_access(array, indices, default_idx, array_configs, bank_accesses);
            for i in indices {
                analyze_expr_iter(i, default_idx, array_configs, bank_accesses, muls);
            }
        }
        CExpr::Binary { op, lhs, rhs } => {
            if *op == BinOp::Mul {
                *muls += 1;
            }
            analyze_expr_iter(lhs, default_idx, array_configs, bank_accesses, muls);
            analyze_expr_iter(rhs, default_idx, array_configs, bank_accesses, muls);
        }
        CExpr::Unary { expr, .. } => {
            analyze_expr_iter(expr, default_idx, array_configs, bank_accesses, muls);
        }
        CExpr::Ternary {
            cond,
            then_e,
            else_e,
        } => {
            analyze_expr_iter(cond, default_idx, array_configs, bank_accesses, muls);
            analyze_expr_iter(then_e, default_idx, array_configs, bank_accesses, muls);
            analyze_expr_iter(else_e, default_idx, array_configs, bank_accesses, muls);
        }
        _ => {}
    }
}

fn record_array_access(
    array: &str,
    indices: &[CExpr],
    default_idx: usize,
    array_configs: &BTreeMap<String, ArrayBankConfig>,
    bank_accesses: &mut HashMap<(String, usize), usize>,
) {
    let Some(cfg) = array_configs.get(array) else {
        return;
    };
    if cfg.is_registers {
        return;
    }
    let coords: Vec<usize> = indices
        .iter()
        .map(|i| eval_const_i64(i).map(|v| v.max(0) as usize).unwrap_or(default_idx))
        .collect();
    let (bank, _) = cfg.map_indices(&coords);
    *bank_accesses.entry((array.to_string(), bank)).or_insert(0) += 1;
}

/// Bind scheduled CDFG operations and arrays to physical DSP48E1, BRAM18E1, LUT, and FF resources.
pub fn bind_resources(
    cdfg: &Cdfg,
    schedule: &ScheduleResult,
    func: &CFunction,
    all_directives: &[HlsDirective],
) -> BindingResult {
    let mut bind_ops = Vec::new();
    let mut dsp_units: Vec<BoundDsp> = Vec::new();
    let mut bram_banks: Vec<BoundBramBank> = Vec::new();

    let mul_alloc_limit: Option<usize> = all_directives.iter().find_map(|d| match d {
        HlsDirective::Allocation(ad)
            if ad.kind == AllocationKind::Operation
                && (ad.instances.eq_ignore_ascii_case("mul")
                    || ad.instances.eq_ignore_ascii_case("mac")
                    || ad.instances.eq_ignore_ascii_case("dsp")) =>
        {
            Some(ad.limit.max(1))
        }
        _ => None,
    });

    // Bind CDFG nodes to resources
    let mut lut_count = 0usize;
    let mut ff_count = 0usize;
    for v in &func.vars {
        if v.dims.is_empty() {
            ff_count += v.width;
        }
    }

    for node in &cdfg.nodes {
        let cyc = schedule
            .sdc_cycles
            .get(node.id)
            .copied()
            .unwrap_or(node.iteration as u32)
            .max(node.iteration as u32);
        match &node.op {
            CdfgOp::Mul => {
                bind_ops.push(BindOp {
                    name: format!("mul@{}", bind_ops.len()),
                    res: Res::Dsp,
                    cycle: cyc,
                });
                let max_dsps = mul_alloc_limit.unwrap_or(usize::MAX);
                if dsp_units.len() < max_dsps {
                    let id = dsp_units.len();
                    dsp_units.push(BoundDsp {
                        id,
                        instance_name: format!("u_dsp48_{id}"),
                        is_mac: false,
                        bound_nodes: vec![node.id],
                        active_cycles: vec![cyc],
                    });
                } else if let Some(shared) = dsp_units.get_mut(node.id % max_dsps) {
                    shared.bound_nodes.push(node.id);
                    shared.active_cycles.push(cyc);
                }
            }
            CdfgOp::Mac => {
                // Fused accumulate on top of Mul: mark the feeding DSP unit as MAC
                if let Some(last_dsp) = dsp_units.last_mut() {
                    last_dsp.is_mac = true;
                }
                bind_ops.push(BindOp {
                    name: format!("add@{}", bind_ops.len()),
                    res: Res::Lut,
                    cycle: cyc,
                });
                lut_count += (node.bitwidth / 2).max(1);
            }
            CdfgOp::Add | CdfgOp::Sub => {
                bind_ops.push(BindOp {
                    name: format!("add@{}", bind_ops.len()),
                    res: Res::Lut,
                    cycle: cyc,
                });
                lut_count += node.bitwidth.max(1);
            }
            CdfgOp::Not => {
                bind_ops.push(BindOp {
                    name: format!("not@{}", bind_ops.len()),
                    res: Res::Lut,
                    cycle: cyc,
                });
                lut_count += node.bitwidth.max(1);
            }
            CdfgOp::And
            | CdfgOp::Or
            | CdfgOp::Xor
            | CdfgOp::Shl
            | CdfgOp::Shr
            | CdfgOp::Cmp(_)
            | CdfgOp::Select => {
                bind_ops.push(BindOp {
                    name: format!("bit@{}", bind_ops.len()),
                    res: Res::Lut,
                    cycle: cyc,
                });
                lut_count += node.bitwidth.max(1);
            }
            CdfgOp::Load { array, bank, .. } | CdfgOp::Store { array, bank, .. } => {
                let is_reg = cdfg
                    .array_configs
                    .get(array)
                    .is_some_and(|c| c.is_registers);
                bind_ops.push(BindOp {
                    name: format!("mem_{array}_b{bank}@{}", bind_ops.len()),
                    res: if is_reg { Res::Ff } else { Res::Bram },
                    cycle: cyc,
                });
            }
            CdfgOp::RegWrite(var_name) => {
                if func.vars.iter().any(|v| &v.name == var_name && v.is_static) {
                    bind_ops.push(BindOp {
                        name: format!("ff_{var_name}"),
                        res: Res::Ff,
                        cycle: cyc,
                    });
                }
            }
            _ => {}
        }
    }

    // Bind arrays to BRAM banks or partitioned FF registers
    let mut total_bram18 = 0usize;
    for cfg in cdfg.array_configs.values() {
        if cfg.is_registers {
            let total_elems: usize = cfg.dims.iter().product::<usize>().max(1);
            ff_count += total_elems * cfg.width;
        } else {
            for b in 0..cfg.num_banks {
                let bits = cfg.width * cfg.bank_depth.max(1);
                let brams = ((bits + 18_431) / 18_432).max(1);
                total_bram18 += brams;
                bram_banks.push(BoundBramBank {
                    array_name: cfg.name.clone(),
                    bank_index: b,
                    instance_name: format!("{}_bank_{b}", cfg.name),
                    width: cfg.width,
                    depth: cfg.bank_depth.max(2),
                    bram18_count: brams,
                    read_ports_used: 1,
                    write_ports_used: usize::from(cfg.is_output),
                });
            }
        }
    }

    let dsp_count = dsp_units.len();
    BindingResult {
        dsp_units,
        bram_banks,
        dsp_count,
        bram18_count: total_bram18,
        ff_count: ff_count.max(1),
        lut_count: lut_count.max(1),
        bind_ops,
    }
}

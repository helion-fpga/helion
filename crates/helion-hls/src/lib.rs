//! Helion High-Level Synthesis (Vivado HLS UG902 parity).
//!
//! Supports C/C++ synthesis with:
//! - Inline `#pragma HLS ...` and TCL `set_directive_...` directives:
//!   `PIPELINE II=k`, `UNROLL factor=k`, `ARRAY_PARTITION variable=... type=complete|block|cyclic factor=k dim=d`,
//!   `INLINE`, `DATAFLOW`, `ALLOCATION`, and `INTERFACE mode=s_axilite|m_axi|axis|ap_fifo|ap_memory|ap_ctrl_hs|ap_ctrl_none`.
//! - Control-Dataflow Graph (CDFG) construction, ASAP/ALAP/SDC Bellman-Ford modulo scheduling
//!   ($II_{\min} = \max(ResII, RecII)$), and clock-period-aware operation chaining.
//! - Resource binding onto Helion `DSP48E1` / `Mac27`, multi-bank `RAMB18E1` (`Bram18`), `LUT6`, and `Hff` registers.
//! - Synthesizable SystemVerilog FSM + datapath + AXI4-Lite CSR / AXI4-Stream / `m_axi` / `ap_fifo` / `ap_memory`
//!   emission and structured Vivado HLS Synthesis & Latency/Utilization Report generation.

pub mod ap_math_stream;
pub mod cosim;
pub mod emit;
pub mod parser;
pub mod pragma;
pub mod schedule;
pub mod systolic_csr_dataflow;

pub use ap_math_stream::{
    ApFixed, ApInt, ApOverflowMode, ApQuantMode, HlsStream, hls_abs_i64, hls_clz_u64,
    hls_cordic_sincos_q14, hls_cos_q14, hls_max_i64, hls_min_i64, hls_popcount_u64, hls_sin_q14,
    hls_sqrt_u64,
};
pub use cosim::{
    AXI4_4KB_BOUNDARY_BYTES, AXI4_MAX_BURST_BEATS, AxiBurstKind, CosimReport, CosimStatus,
    CosimTestVector, CosimVectorResult, MAxiBurstDescriptor, coalesce_m_axi_bursts,
    evaluate_c_kernel_reference, extract_kernel_m_axi_bursts, generate_cosim_testbench,
    run_cosim_verification,
};
pub use emit::{generate_hls_report, resolve_interfaces, HlsSynthesisReport, InterfacePort};
pub use parser::{
    tokenize, BinOp, CArg, CExpr, CFunction, CStmt, CType, CVar, LValue, Parser, Tok,
    TranslationUnit, UnaryOp,
};
pub use pragma::{
    extract_inline_pragmas, parse_pragma_line, parse_tcl_config, parse_tcl_directives,
    AllocationDirective, AllocationKind, ArrayPartitionDirective, DataflowChannelKind,
    DataflowDirective, DependenceDirection, DependenceDirective, DependenceType, HlsConfig,
    HlsDirective, InlineDirective, InlineMode, InterfaceDirective, InterfaceMode, PartitionType,
    PipelineDirective, PipelineStyle, UnrollDirective,
};
pub use schedule::{
    bind_resources, build_cdfg, compute_alap, compute_asap, compute_sdc_schedule,
    resolve_top_and_inline, schedule_cdfg, ArrayBankConfig, BasicBlock, BindingResult,
    BoundBramBank, BoundDsp, Cdfg, CdfgEdge, CdfgNode, CdfgOp, CmpKind, ControlEdge,
    ControlEdgeKind, DataflowChannel, DataflowProcess, DepKind, LoopScheduleInfo, ScheduleResult,
};
pub use systolic_csr_dataflow::{
    AxiLiteCsrMap, AxiLiteCsrReg, DataflowChannelSpec, DataflowChannelStorage, DataflowGraph,
    DataflowTaskNode, SystolicArrayConfig, SystolicDataflowMode, SystolicGridResult,
    SystolicPeNode, analyze_dataflow_network, build_axilite_csr_map,
    compute_deadlock_free_fifo_depth, emit_axilite_c_driver, generate_systolic_pe_grid,
};

use helion_ir::{CellKind, Design};
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Res {
    Lut,
    Ff,
    Dsp,
    Bram,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BindOp {
    pub name: String,
    pub res: Res,
    pub cycle: u32,
}

#[derive(Clone, Debug)]
pub struct HlsResult {
    pub design: Design,
    pub ops: Vec<BindOp>,
    pub sv: String,
    pub cdfg: Cdfg,
    pub schedule: ScheduleResult,
    pub binding: BindingResult,
    pub directives: Vec<HlsDirective>,
    pub interfaces: Vec<InterfacePort>,
    pub report: HlsSynthesisReport,
    pub report_text: String,
}

/// Compile C/C++ HLS source with default configuration (`10.0ns` target clock).
pub fn hls_compile(source: &str) -> Result<HlsResult, String> {
    hls_compile_with_config(source, &HlsConfig::default())
}

/// Compile C/C++ HLS source alongside a Vivado HLS TCL directive script.
pub fn hls_compile_with_tcl(source: &str, tcl: &str) -> Result<HlsResult, String> {
    let cfg = parse_tcl_config(tcl)?;
    hls_compile_with_config(source, &cfg)
}

/// Compile C/C++ HLS source with an explicit [`HlsConfig`].
pub fn hls_compile_with_config(source: &str, config: &HlsConfig) -> Result<HlsResult, String> {
    let tokens = tokenize(source)?;
    if tokens.is_empty() {
        return Err("hls: empty".into());
    }
    let mut parser = Parser::new(&tokens);
    let tu = parser
        .parse_translation_unit()
        .map_err(|e| format!("hls: {e}"))?;

    let (top_fn, stage_fns, all_directives) = resolve_top_and_inline(&tu, config)?;
    if top_fn.body.is_empty() && top_fn.vars.is_empty() {
        return Err("hls: unsupported body".into());
    }

    let cdfg = build_cdfg(&top_fn, &stage_fns, &all_directives);
    let schedule = schedule_cdfg(&cdfg, &top_fn, &all_directives, config.target_clock_ns);
    let binding = bind_resources(&cdfg, &schedule, &top_fn, &all_directives);
    let interfaces = resolve_interfaces(&top_fn, &cdfg, &all_directives);

    let sv = emit::emit_rtl(
        &top_fn,
        &cdfg,
        &schedule,
        &binding,
        &interfaces,
        &all_directives,
    )?;

    let design =
        helion_sv::synth_sv(&sv, "hls.sv").map_err(|e| format!("sv synth: {e} :: {sv}"))?;

    let actual_dsp = design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Mac27 | CellKind::Dsp48e1 { .. }))
        .count();
    let actual_bram = design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Bram18 | CellKind::Bram18e1 { .. }))
        .count();
    let actual_ff = design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    let actual_lut = design
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
        .count();

    let (report, report_text) = generate_hls_report(
        &top_fn,
        &cdfg,
        &schedule,
        &binding,
        &interfaces,
        &config.part_name,
        actual_dsp,
        actual_bram,
        actual_ff,
        actual_lut,
    );

    Ok(HlsResult {
        design,
        ops: binding.bind_ops.clone(),
        sv,
        cdfg,
        schedule,
        binding,
        directives: all_directives,
        interfaces,
        report,
        report_text,
    })
}

pub fn synth_c(source: &str) -> Result<Design, String> {
    Ok(hls_compile(source)?.design)
}

pub fn synth_c_path(path: &Path) -> Result<Design, String> {
    let src = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    synth_c(&src)
}

#[cfg(test)]
mod tests {
    use super::*;
    use helion_ir::{CellKind, INC4_INIT};

    #[test]
    fn hls_blinky_static_invert() {
        let src = r#"
void blinky(bool *led) {
  static bool q = 0;
  q = !q;
  *led = q;
}
"#;
        let d = synth_c(src).unwrap();
        assert_eq!(d.lut_inits(), vec![0x5555_5555_5555_5555]);
    }

    #[test]
    fn hls_mul_is_mac27() {
        let src = r#"
int mac(int a, int b) { return a * b; }
"#;
        let d = synth_c(src).unwrap();
        assert!(d
            .cells
            .iter()
            .any(|c| matches!(c.kind, CellKind::Mac27 | CellKind::Dsp48e1 { .. })));
    }

    #[test]
    fn hls_garbage_fails() {
        assert!(synth_c("lorem ipsum").is_err());
    }

    #[test]
    fn hls_schedule_bind_is_not_pattern_match() {
        let inc3 = r#"
void counter(bool *led) {
  static ap_uint<3> cnt = 0;
  cnt = cnt + 1;
  *led = cnt[2];
}
"#;
        let inc4 = r#"
void counter(bool *led) {
  static ap_uint<4> cnt = 0;
  cnt = cnt + 1;
  *led = cnt[3];
}
"#;
        let r3 = hls_compile(inc3).unwrap();
        let r4 = hls_compile(inc4).unwrap();
        assert_eq!(
            r3.design.lut_inits().len(),
            3,
            "3-bit HLS incrementer {:?}",
            r3.sv
        );
        assert_eq!(
            r4.design.lut_inits(),
            INC4_INIT.to_vec(),
            "4-bit must match gold {}",
            r4.sv
        );
        assert_ne!(r3.design.lut_inits().len(), r4.design.lut_inits().len());
        assert!(r3.ops.iter().any(|o| o.res == Res::Lut));
        assert!(r3.ops.iter().any(|o| o.res == Res::Ff));

        let one = r#"
int mac1(int a, int b) { return a * b; }
"#;
        let two = r#"
int mac2(int a, int b) {
  int t = 0;
  for (int i = 0; i < 2; i++) {
    t = a * b;
  }
  return t;
}
"#;
        let m1 = hls_compile(one).unwrap();
        let m2 = hls_compile(two).unwrap();
        let n1 = m1
            .design
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Mac27 | CellKind::Dsp48e1 { .. }))
            .count();
        let n2 = m2
            .design
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Mac27 | CellKind::Dsp48e1 { .. }))
            .count();
        assert_eq!(n1, 1, "one mul → one DSP {}", m1.sv);
        assert_eq!(
            n2, 2,
            "unrolled 2-iter mul loop must bind 2 DSP, not 1: ops={:?} sv={}",
            m2.ops, m2.sv
        );
        assert!(m2.ops.iter().filter(|o| o.res == Res::Dsp).count() >= 2);
        assert!(
            m2.ops.iter().any(|o| o.cycle >= 1),
            "loop iterations must occupy distinct schedule cycles {:?}",
            m2.ops
        );
    }

    #[test]
    fn hls_pragma_and_tcl_directive_parser() {
        let src = r#"
#pragma HLS INTERFACE mode=s_axilite port=return bundle=control
#pragma HLS INTERFACE s_axilite port=a bundle=control
#pragma HLS INTERFACE m_axi port=gmem_buf bundle=gmem0 depth=256
#pragma HLS INTERFACE axis port=in_stream
#pragma HLS INTERFACE ap_fifo port=out_fifo
#pragma HLS INTERFACE ap_memory port=table
#pragma HLS ARRAY_PARTITION variable=A type=cyclic factor=4 dim=2
#pragma HLS ARRAY_PARTITION variable=B block factor=2 dim=1
#pragma HLS ARRAY_PARTITION variable=shift_reg complete dim=1
#pragma HLS PIPELINE II=2 rewind
#pragma HLS UNROLL factor=4 skip_exit_check
#pragma HLS INLINE recursive
#pragma HLS DATAFLOW interval=1
#pragma HLS ALLOCATION instances=mul limit=2 operation
"#;
        let dirs = extract_inline_pragmas(src).expect("parse inline pragmas");
        assert_eq!(dirs.len(), 14);
        assert!(dirs.contains(&HlsDirective::Pipeline(PipelineDirective {
            ii: Some(2),
            rewind: true,
            enable_flush: false,
            style: PipelineStyle::Stp,
            off: false,
            target: None,
        })));
        assert!(dirs.contains(&HlsDirective::ArrayPartition(ArrayPartitionDirective {
            variable: "A".into(),
            kind: PartitionType::Cyclic,
            factor: Some(4),
            dim: 2,
            target: None,
        })));

        let tcl = r#"
create_clock -period 5.0
set_top matmul
set_directive_pipeline -II 1 "matmul/LOOP_J"
set_directive_unroll -factor 4 "matmul/LOOP_K"
set_directive_array_partition -type cyclic -factor 4 -dim 2 "matmul" A
set_directive_array_partition -type block -factor 4 -dim 1 "matmul" B
set_directive_interface -mode s_axilite -bundle ctrl "matmul" return
set_directive_allocation -limit 2 -type operation "matmul" mul
"#;
        let cfg = parse_tcl_config(tcl).expect("parse tcl");
        assert_eq!(cfg.target_clock_ns, 5.0);
        assert_eq!(cfg.top_function.as_deref(), Some("matmul"));
        assert_eq!(cfg.directives.len(), 6);
    }

    #[test]
    fn hls_pipelined_fir_filter_with_complete_array_partition() {
        let src = r#"
#define N_TAPS 4
void fir_filter(int *y, int x) {
  #pragma HLS INTERFACE ap_ctrl_hs port=return
  #pragma HLS INTERFACE ap_vld port=y
  #pragma HLS PIPELINE II=1
  static int shift_reg[N_TAPS] = {0, 0, 0, 0};
  #pragma HLS ARRAY_PARTITION variable=shift_reg type=complete dim=1
  const int coeff[N_TAPS] = {3, 7, 11, 5};
  #pragma HLS ARRAY_PARTITION variable=coeff type=complete dim=1

  int acc = 0;
  SHIFT_LOOP: for (int i = 3; i >= 1; i--) {
    shift_reg[i] = shift_reg[i - 1];
  }
  shift_reg[0] = x;

  MAC_LOOP: for (int i = 0; i < 4; i++) {
    acc += shift_reg[i] * coeff[i];
  }
  *y = acc;
}
"#;
        let res = hls_compile(src).expect("fir_filter compile");
        assert_eq!(res.schedule.achieved_ii, 1, "FIR pipeline II must be 1");
        assert_eq!(res.schedule.res_ii, 1, "Complete partition eliminates BRAM port bottleneck");
        assert_eq!(res.schedule.rec_ii, 1);
        assert_eq!(
            res.binding.bram18_count, 0,
            "Complete array partition must map arrays to FFs, not BRAMs"
        );
        let dsp_cells = res
            .design
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Mac27 | CellKind::Dsp48e1 { .. }))
            .count();
        assert_eq!(
            dsp_cells, 4,
            "4-tap pipelined FIR must bind 4 parallel DSP48 blocks, sv={}",
            res.sv
        );
        assert!(res.sv.contains("ap_start"));
        assert!(res.sv.contains("ap_done"));
        assert!(res.sv.contains("y_ap_vld"));
        assert!(res.report_text.contains("Vivado HLS Synthesis Report for 'fir_filter'"));
    }

    #[test]
    fn hls_matrix_multiply_array_partition_and_modulo_scheduling() {
        // 1. Unpartitioned matrix multiplication: 4 reads to bank 0 of A and B per pipelined J iteration -> ResII = 2
        let unpartitioned = r#"
void matmul(int A[4][4], int B[4][4], int C[4][4]) {
  ROW: for (int i = 0; i < 2; i++) {
    COL: for (int j = 0; j < 2; j++) {
      #pragma HLS PIPELINE II=1
      int sum = 0;
      DOT: for (int k = 0; k < 4; k++) {
        sum += A[i][k] * B[k][j];
      }
      C[i][j] = sum;
    }
  }
}
"#;
        let r_unpart = hls_compile(unpartitioned).expect("unpartitioned matmul");
        assert!(
            r_unpart.schedule.res_ii >= 2,
            "Unpartitioned 4-element dot product exceeds 2-port BRAM limit (ResII={})",
            r_unpart.schedule.res_ii
        );
        assert!(r_unpart.schedule.achieved_ii >= 2);

        // 2. Partitioned matrix multiplication: A cyclic factor=4 dim=2, B block factor=4 dim=1 -> ResII = 1, II = 1
        let partitioned = r#"
void matmul(int A[4][4], int B[4][4], int C[4][4]) {
  #pragma HLS ARRAY_PARTITION variable=A type=cyclic factor=4 dim=2
  #pragma HLS ARRAY_PARTITION variable=B type=block factor=4 dim=1
  ROW: for (int i = 0; i < 2; i++) {
    COL: for (int j = 0; j < 2; j++) {
      #pragma HLS PIPELINE II=1
      int sum = 0;
      DOT: for (int k = 0; k < 4; k++) {
        sum += A[i][k] * B[k][j];
      }
      C[i][j] = sum;
    }
  }
}
"#;
        let r_part = hls_compile(partitioned).expect("partitioned matmul");
        assert_eq!(
            r_part.schedule.res_ii, 1,
            "4-bank partitioning resolves BRAM port conflict to ResII=1"
        );
        assert_eq!(r_part.schedule.achieved_ii, 1);
        assert_eq!(r_part.cdfg.array_configs["A"].num_banks, 4);
        assert_eq!(r_part.cdfg.array_configs["B"].num_banks, 4);
        assert_eq!(r_part.cdfg.array_configs["C"].num_banks, 1);
        // 4 banks for A + 4 banks for B + 1 bank for C = 9 BRAM18 banks inferred in netlist
        let bram_cells = r_part
            .design
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Bram18 | CellKind::Bram18e1 { .. }))
            .count();
        assert_eq!(bram_cells, 9, "Expected 9 BRAM18E1 cells for 4+4+1 banks");
        assert!(
            r_part.report.max_latency_cycles < r_unpart.report.max_latency_cycles,
            "Partitioned II=1 matmul must have lower latency than unpartitioned II=2"
        );
    }

    #[test]
    fn hls_allocation_limit_scales_res_ii_and_shares_dsps() {
        let src = r#"
void dot4(int a[4], int b[4], int *out) {
  #pragma HLS ARRAY_PARTITION variable=a complete
  #pragma HLS ARRAY_PARTITION variable=b complete
  #pragma HLS ALLOCATION instances=mul limit=2 operation
  #pragma HLS PIPELINE II=1
  int acc = 0;
  LOOP: for (int i = 0; i < 4; i++) {
    acc += a[i] * b[i];
  }
  *out = acc;
}
"#;
        let res = hls_compile(src).expect("dot4 with allocation limit=2");
        assert_eq!(
            res.binding.dsp_count, 2,
            "ALLOCATION limit=2 must cap bound DSP48 units at 2"
        );
        assert_eq!(
            res.schedule.res_ii, 2,
            "4 multiplies with 2 DSPs yields ResII = ceil(4/2) = 2"
        );
        assert_eq!(res.schedule.achieved_ii, 2);
        let dsp_cells = res
            .design
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Mac27 | CellKind::Dsp48e1 { .. }))
            .count();
        assert_eq!(dsp_cells, 2);
    }

    #[test]
    fn hls_axi_lite_axis_m_axi_and_fifo_interfaces() {
        let src = r#"
void stream_accel(
    hls::stream<int> &in_s,
    hls::stream<int> &out_s,
    int *dram_ptr,
    int *fifo_out,
    int gain,
    int bias
) {
  #pragma HLS INTERFACE s_axilite port=return bundle=control
  #pragma HLS INTERFACE s_axilite port=gain bundle=control
  #pragma HLS INTERFACE s_axilite port=bias bundle=control
  #pragma HLS INTERFACE axis port=in_s
  #pragma HLS INTERFACE axis port=out_s
  #pragma HLS INTERFACE m_axi port=dram_ptr bundle=gmem0 depth=64
  #pragma HLS INTERFACE ap_fifo port=fifo_out
  #pragma HLS PIPELINE II=1

  int x = in_s.read();
  int y = x * gain + bias + *dram_ptr;
  *fifo_out = y;
  out_s.write(y);
}
"#;
        let res = hls_compile(src).expect("stream_accel compile");
        assert!(res.sv.contains("s_axi_control_AWVALID"));
        assert!(res.sv.contains("s_axi_control_WDATA"));
        assert!(res.sv.contains("s_axi_control_RDATA"));
        assert!(res.sv.contains("m_axi_gmem0_AWVALID"));
        assert!(res.sv.contains("m_axi_gmem0_RDATA"));
        assert!(res.sv.contains("in_s_TDATA"));
        assert!(res.sv.contains("in_s_TVALID"));
        assert!(res.sv.contains("in_s_TREADY"));
        assert!(res.sv.contains("out_s_TDATA"));
        assert!(res.sv.contains("out_s_TVALID"));
        assert!(res.sv.contains("out_s_TLAST"));
        assert!(res.sv.contains("fifo_out_din"));
        assert!(res.sv.contains("fifo_out_full_n"));
        assert!(res.sv.contains("fifo_out_write"));
        assert!(res.report_text.contains("s_axilite"));
        assert!(res.report_text.contains("m_axi"));
        assert!(res.report_text.contains("axis"));
        assert!(res.report_text.contains("ap_fifo"));
    }

    #[test]
    fn hls_dataflow_and_inline_pipeline() {
        let src = r#"
int scale_elem(int x, int factor) {
  #pragma HLS INLINE
  return x * factor;
}

void stage_load(int in_arr[4], int buf[4], int k) {
  for (int i = 0; i < 4; i++) {
    #pragma HLS PIPELINE II=1
    buf[i] = scale_elem(in_arr[i], k);
  }
}

void stage_compute(int buf[4], int out_arr[4]) {
  for (int i = 0; i < 4; i++) {
    #pragma HLS PIPELINE II=1
    out_arr[i] = buf[i] + 1;
  }
}

void top_dataflow(int in_arr[4], int out_arr[4], int k) {
  #pragma HLS DATAFLOW
  int mid_buf[4];
  stage_load(in_arr, mid_buf, k);
  stage_compute(mid_buf, out_arr);
}
"#;
        let res = hls_compile(src).expect("top_dataflow compile");
        assert!(res.cdfg.is_dataflow);
        assert_eq!(res.cdfg.dataflow_processes.len(), 2);
        assert_eq!(res.cdfg.dataflow_processes[0].name, "stage_load");
        assert_eq!(res.cdfg.dataflow_processes[1].name, "stage_compute");
        assert!(!res.cdfg.dataflow_channels.is_empty());
        assert_eq!(res.cdfg.dataflow_channels[0].name, "mid_buf");
        assert!(res.sv.contains("df_fifo_mid_buf_din"));
        assert!(res.report.is_dataflow);
        assert!(res.report_text.contains("dataflow"));
    }

    #[test]
    fn hls_asap_alap_sdc_clock_period_chaining() {
        let src = r#"
void chained_alu(int a, int b, int c, int d, int *y) {
  #pragma HLS INTERFACE ap_ctrl_hs port=return
  int t1 = a + b;
  int t2 = t1 ^ c;
  int t3 = t2 + d;
  int t4 = t3 * a;
  *y = t4 + b;
}
"#;
        // Relaxed clock (10.0ns) chains fast LUT operations into fewer stages than tight clock (1.8ns)
        let relaxed_cfg = HlsConfig {
            target_clock_ns: 10.0,
            ..HlsConfig::default()
        };
        let tight_cfg = HlsConfig {
            target_clock_ns: 1.8,
            ..HlsConfig::default()
        };
        let r_rel = hls_compile_with_config(src, &relaxed_cfg).unwrap();
        let r_tight = hls_compile_with_config(src, &tight_cfg).unwrap();
        assert!(
            r_tight.schedule.pipeline_depth > r_rel.schedule.pipeline_depth,
            "Tight clock period must insert more pipeline stages ({}) than relaxed clock ({})",
            r_tight.schedule.pipeline_depth,
            r_rel.schedule.pipeline_depth
        );
        for i in 0..r_rel.cdfg.nodes.len() {
            assert!(
                r_rel.schedule.asap_cycles[i] <= r_rel.schedule.alap_cycles[i],
                "ASAP cycle must be <= ALAP cycle for node {i}"
            );
        }
    }

    #[test]
    fn hls_compile_with_tcl_directives_and_multibank_memory() {
        let src = r#"
void vec_mac(int a[8], int b[8], int *out) {
  int acc = 0;
  MAC_LOOP: for (int i = 0; i < 8; i++) {
    acc += a[i] * b[i];
  }
  *out = acc;
}
"#;
        let tcl = r#"
create_clock -period 8.0
set_top vec_mac
set_directive_interface -mode ap_ctrl_none "vec_mac" return
set_directive_array_partition -type cyclic -factor 4 -dim 1 "vec_mac" a
set_directive_array_partition -type block -factor 4 -dim 1 "vec_mac" b
set_directive_unroll -factor 2 "vec_mac/MAC_LOOP"
set_directive_pipeline -II 1 "vec_mac/MAC_LOOP"
"#;
        let res = hls_compile_with_tcl(src, tcl).expect("hls_compile_with_tcl");
        assert_eq!(res.schedule.target_clock_ns, 8.0);
        assert_eq!(res.schedule.res_ii, 1);
        assert_eq!(res.schedule.achieved_ii, 1);
        assert_eq!(res.schedule.loop_schedules[0].unroll_factor, 2);
        assert!(res.sv.contains("a_0_address0"));
        assert!(res.sv.contains("a_3_address0"));
        assert!(res.sv.contains("b_0_address0"));
        assert!(res.sv.contains("b_3_address0"));
        assert!(!res.sv.contains("ap_start"), "ap_ctrl_none must omit ap_start");
    }

    #[test]
    fn hls_recurrence_ii_under_tight_clock() {
        let src = r#"
void iir_filter(int x[8], int y[8], int alpha) {
  int state = 0;
  IIR_LOOP: for (int i = 0; i < 8; i++) {
    #pragma HLS PIPELINE II=1
    state = state * alpha + x[i];
    y[i] = state;
  }
}
"#;
        let tight_cfg = HlsConfig {
            target_clock_ns: 3.0,
            ..HlsConfig::default()
        };
        let res = hls_compile_with_config(src, &tight_cfg).expect("iir_filter");
        assert!(
            res.schedule.rec_ii >= 2,
            "Recurrence through multiplier under 3ns clock requires RecII >= 2, got {}",
            res.schedule.rec_ii
        );
        assert_eq!(
            res.schedule.ii_min,
            res.schedule.res_ii.max(res.schedule.rec_ii)
        );
        assert!(res.schedule.achieved_ii >= res.schedule.ii_min);
    }

    #[test]
    fn hls_ap_int_ap_fixed_quantization_overflow_and_range_slice() {
        // 1. ApInt bit-slice `.range(hi, lo)` and reversed `.range(lo, hi)`
        let mut x = ApInt::from_u128(16, false, 0xABCD);
        assert_eq!(x.range(15, 8).to_u128(), 0xAB);
        assert_eq!(x.range(7, 0).to_u128(), 0xCD);
        // Reverse 4-bit nibble 0b1101 (0xD) -> 0b1011 (0xB)
        assert_eq!(x.range(0, 3).to_u128(), 0xB);
        x.set_range(11, 4, 0x5A);
        assert_eq!(x.to_u128(), 0xA5AD);

        // 2. ApFixed quantization (`AP_TRN` vs `AP_RND` vs `AP_RND_CONV`)
        // Q4.2 format (width=6, int_bits=4 -> 2 fractional bits, step = 0.25)
        // Value 1.375 = 1 + 1.5 * 0.25 (halfway between 1.25 and 1.50)
        let f_trn = ApFixed::from_f64(
            1.375,
            6,
            4,
            true,
            ApQuantMode::ApTrn,
            ApOverflowMode::ApWrap,
        );
        let f_rnd = ApFixed::from_f64(
            1.375,
            6,
            4,
            true,
            ApQuantMode::ApRnd,
            ApOverflowMode::ApWrap,
        );
        assert_eq!(f_trn.to_f64(), 1.25, "AP_TRN truncates 1.375 to 1.25");
        assert_eq!(f_rnd.to_f64(), 1.50, "AP_RND rounds 1.375 up to 1.50");

        // 3. ApFixed overflow (`AP_SAT` vs `AP_WRAP` vs `AP_SAT_SYM`)
        // Signed Q4.2 range is [-8.0, +7.75] (raw [-32, +31])
        let f_sat = ApFixed::from_f64(12.5, 6, 4, true, ApQuantMode::ApRnd, ApOverflowMode::ApSat);
        let f_wrap =
            ApFixed::from_f64(12.5, 6, 4, true, ApQuantMode::ApRnd, ApOverflowMode::ApWrap);
        let f_sym = ApFixed::from_f64(
            -12.5,
            6,
            4,
            true,
            ApQuantMode::ApRnd,
            ApOverflowMode::ApSatSym,
        );
        assert_eq!(f_sat.to_f64(), 7.75, "AP_SAT clamps +12.5 to +7.75");
        assert!(f_wrap.to_f64() < 0.0, "AP_WRAP wraps +12.5 into negative range");
        assert_eq!(
            f_sym.to_f64(),
            -7.75,
            "AP_SAT_SYM clamps -12.5 symmetrically to -7.75"
        );
    }

    #[test]
    fn hls_stream_fifo_and_hls_math_intrinsics_synthesis() {
        // 1. HlsStream blocking & non-blocking FIFO semantics
        let mut fifo: HlsStream<i32> = HlsStream::new("data_ch", 2);
        assert!(fifo.empty());
        assert!(fifo.write_nb(10));
        assert!(fifo.write_nb(20));
        assert!(fifo.full());
        assert!(!fifo.write_nb(30), "write_nb on full stream must return false");
        assert_eq!(fifo.read().unwrap(), 10);
        assert_eq!(fifo.read_nb(), Some(20));
        assert_eq!(fifo.read_nb(), None);

        // 2. Bit-accurate `hls::math` intrinsics
        assert_eq!(hls_sqrt_u64(0), 0);
        assert_eq!(hls_sqrt_u64(144), 12);
        assert_eq!(hls_sqrt_u64(10_000), 100);
        assert_eq!(hls_abs_i64(-42), 42);
        assert_eq!(hls_min_i64(-7, 3), -7);
        assert_eq!(hls_max_i64(-7, 3), 3);
        assert_eq!(hls_clz_u64(0b0001_0100, 8), 3);
        assert_eq!(hls_popcount_u64(0b1011_0101, 8), 5);
        let (sin0, cos0) = hls_cordic_sincos_q14(0);
        assert!(sin0.abs() <= 16, "sin(0) in Q14 ~ 0, got {sin0}");
        assert!((cos0 - 16384).abs() <= 16, "cos(0) in Q14 ~ 16384, got {cos0}");
        let (sin_pi2, cos_pi2) = hls_cordic_sincos_q14(25736); // pi/2 in Q14
        assert!((sin_pi2 - 16384).abs() <= 32, "sin(pi/2) in Q14 ~ 16384, got {sin_pi2}");
        assert!(cos_pi2.abs() <= 32, "cos(pi/2) in Q14 ~ 0, got {cos_pi2}");

        // 3. End-to-end C++ HLS synthesis with ap_fixed, .range(), hls::stream, and hls::math
        let src = r#"
void dsp_math_stream_kernel(
    hls::stream<ap_fixed<16, 8, AP_RND, AP_SAT>> &in_s,
    hls::stream<ap_fixed<16, 8, AP_RND, AP_SAT>> &out_s,
    ap_int<16> coeff
) {
  #pragma HLS PIPELINE II=1
  ap_fixed<16, 8, AP_RND, AP_SAT> x = in_s.read();
  ap_uint<8> low_byte = x.range(7, 0);
  ap_int<16> mag = hls::abs(x);
  ap_int<16> root = hls::sqrt(mag);
  ap_int<16> clamped = hls::min(root, coeff);
  ap_fixed<16, 8, AP_RND, AP_SAT> res = clamped * coeff + low_byte;
  out_s.write_nb(res);
}
"#;
        let compiled = hls_compile(src).expect("compile dsp_math_stream_kernel");
        assert_eq!(compiled.schedule.achieved_ii, 1);
        assert!(compiled.sv.contains("in_s_TDATA"));
        assert!(compiled.sv.contains("out_s_TDATA"));
        assert!(
            compiled
                .design
                .cells
                .iter()
                .any(|c| matches!(c.kind, CellKind::Mac27 | CellKind::Dsp48e1 { .. }))
        );
    }

    #[test]
    fn hls_dependence_directive_adjusts_rec_ii_and_distance() {
        // 1. Without #pragma HLS DEPENDENCE: loop reading and writing `hist[i]` with multiply has RecII = 4
        let conservative_src = r#"
void hist_update(int hist[16], int scale) {
  HIST_LOOP: for (int i = 1; i < 16; i++) {
    #pragma HLS PIPELINE II=1
    hist[i] = hist[i - 1] * scale + 1;
  }
}
"#;
        let r_cons = hls_compile(conservative_src).expect("conservative hist_update");
        assert_eq!(r_cons.schedule.rec_ii, 4, "Default distance=1 array recurrence with mul has RecII=4");
        assert_eq!(r_cons.schedule.achieved_ii, 4);

        // 2. With `#pragma HLS DEPENDENCE variable=hist inter true distance=2`: RecII = ceil(4 / 2) = 2
        let dist2_src = r#"
void hist_update(int hist[16], int scale) {
  HIST_LOOP: for (int i = 2; i < 16; i++) {
    #pragma HLS PIPELINE II=1
    #pragma HLS DEPENDENCE variable=hist inter true RAW distance=2
    hist[i] = hist[i - 2] * scale + 1;
  }
}
"#;
        let r_dist2 = hls_compile(dist2_src).expect("distance=2 hist_update");
        assert_eq!(r_dist2.schedule.rec_ii, 2, "distance=2 halves RecII from 4 to 2");
        assert_eq!(r_dist2.schedule.achieved_ii, 2);

        // 3. With `#pragma HLS DEPENDENCE variable=hist inter false`: false dependence removes recurrence -> RecII = 1, II = 1
        let false_dep_src = r#"
void hist_update(int hist[16], int scale) {
  HIST_LOOP: for (int i = 0; i < 16; i++) {
    #pragma HLS PIPELINE II=1
    #pragma HLS DEPENDENCE variable=hist inter false
    hist[i] = hist[i] * scale + 1;
  }
}
"#;
        let r_false = hls_compile(false_dep_src).expect("inter false hist_update");
        assert_eq!(r_false.schedule.rec_ii, 1, "inter false must eliminate array RecII");
        assert_eq!(r_false.schedule.achieved_ii, 1);
    }

    #[test]
    fn hls_m_axi_burst_coalescing_and_4kb_boundary_splitting() {
        // 1. 600 words (4 bytes each = 2400 bytes) starting at 0x0800 (2048):
        //    - Only 2048 bytes (512 words) remain before the 0x1000 (4096) 4KB boundary!
        //    - Burst 0: 256 beats at 0x0800..0x0C00 (AWLEN = 255)
        //    - Burst 1: 256 beats at 0x0C00..0x1000 (AWLEN = 255, hits 4KB boundary at 0x1000)
        //    - Burst 2: remaining 88 beats at 0x1000..0x1160 (AWLEN = 87)
        let bursts = coalesce_m_axi_bursts("gmem_in", "gmem0", AxiBurstKind::Read, 0x0800, 600, 4, 256);
        assert_eq!(bursts.len(), 3);
        assert_eq!(bursts[0].beats, 256);
        assert_eq!(bursts[0].axlen, 255);
        assert_eq!(bursts[0].start_byte_addr, 0x0800);
        assert!(bursts[0].prefetch_enabled);
        assert_eq!(bursts[1].beats, 256);
        assert_eq!(bursts[1].start_byte_addr, 0x0C00);
        assert_eq!(bursts[2].beats, 88);
        assert_eq!(bursts[2].axlen, 87);
        assert_eq!(bursts[2].start_byte_addr, 0x1000);

        // 2. Verify m_axi burst ports in synthesized RTL
        let src = r#"
void axi_burst_copy(int *in_dram, int *out_dram) {
  #pragma HLS INTERFACE m_axi port=in_dram bundle=gmem0
  #pragma HLS INTERFACE m_axi port=out_dram bundle=gmem1
  BURST_LOOP: for (int i = 0; i < 32; i++) {
    #pragma HLS PIPELINE II=1
    *out_dram = *in_dram + 5;
  }
}
"#;
        let hls = hls_compile(src).expect("compile axi_burst_copy");
        assert!(hls.sv.contains("m_axi_gmem0_ARLEN"));
        assert!(hls.sv.contains("m_axi_gmem0_RLAST"));
        assert!(hls.sv.contains("m_axi_gmem1_AWLEN"));
        assert!(hls.sv.contains("m_axi_gmem1_WLAST"));
        let kernel_bursts = extract_kernel_m_axi_bursts(&hls, 0);
        assert_eq!(kernel_bursts.len(), 2);
        assert_eq!(kernel_bursts[0].beats, 32);
        assert_eq!(kernel_bursts[0].axlen, 31);
    }

    #[test]
    fn hls_c_rtl_cosim_testbench_and_verification() {
        let src = r#"
void fir_mac_cosim(int a[4], int b[4], int bias, int *out) {
  #pragma HLS PIPELINE II=1
  #pragma HLS ARRAY_PARTITION variable=a complete
  #pragma HLS ARRAY_PARTITION variable=b complete
  int acc = bias;
  for (int i = 0; i < 4; i++) {
    acc += a[i] * b[i];
  }
  *out = acc;
}
"#;
        let v0 = CosimTestVector::new()
            .with_array("a", vec![1, 2, 3, 4])
            .with_array("b", vec![10, 20, 30, 40])
            .with_scalar("bias", 5);
        let v1 = CosimTestVector::new()
            .with_array("a", vec![2, 0, -1, 3])
            .with_array("b", vec![4, 5, 6, 7])
            .with_scalar("bias", 10);

        let report = run_cosim_verification(src, &[v0.clone(), v1]).expect("cosim pass");
        assert_eq!(report.status, CosimStatus::Pass);
        assert_eq!(report.vectors_tested, 2);
        assert_eq!(report.vectors_passed, 2);
        // 1*10 + 2*20 + 3*30 + 4*40 + 5 = 305
        assert_eq!(report.vector_results[0].c_outputs["out"], vec![305]);
        assert_eq!(report.vector_results[0].rtl_outputs["out"], vec![305]);
        // 2*4 + 0*5 + (-1)*6 + 3*7 + 10 = 8 - 6 + 21 + 10 = 33
        assert_eq!(report.vector_results[1].c_outputs["out"], vec![33]);
        assert!(report.testbench_sv.contains("module fir_mac_cosim_cosim_tb;"));
        assert!(report.testbench_sv.contains("Verify out == 305"));
        assert!(report.report_text.contains("PASS"));

        // Fault injection must cause CosimStatus::Fail
        let mut v_fault = v0;
        v_fault.inject_rtl_fault_delta = 1;
        let fail_report = run_cosim_verification(src, &[v_fault]).expect("cosim fault run");
        assert_eq!(fail_report.status, CosimStatus::Fail);
        assert_eq!(fail_report.vectors_passed, 0);
        assert!(fail_report.report_text.contains("FAIL"));
    }

    #[test]
    fn hls_dataflow_task_network_pipo_vs_fifo_and_deadlock_free_sizing() {
        let src = r#"
void producer_seq(int in_a[16], int seq_ch[16], int rand_buf[16]) {
  for (int i = 0; i < 16; i++) {
    #pragma HLS PIPELINE II=1
    seq_ch[i] = in_a[i] + 1;
    rand_buf[i] = in_a[i] * 2;
  }
}

void consumer_unbalanced(int seq_ch[16], int rand_buf[16], int out_b[16]) {
  for (int i = 0; i < 16; i++) {
    #pragma HLS PIPELINE II=4
    out_b[i] = seq_ch[i] + rand_buf[15 - i];
  }
}

void dataflow_top(int in_a[16], int out_b[16]) {
  #pragma HLS DATAFLOW
  int seq_ch[16];
  int rand_buf[16];
  producer_seq(in_a, seq_ch, rand_buf);
  consumer_unbalanced(seq_ch, rand_buf, out_b);
}
"#;
        let df = analyze_dataflow_network(src).expect("analyze_dataflow_network");
        assert_eq!(df.tasks.len(), 2);
        assert_eq!(df.channels.len(), 2);
        // `seq_ch` is accessed sequentially (`seq_ch[i]`) -> StreamFifo
        // With producer II=1 and slower consumer II=4 over 16 items, deadlock-free FIFO depth > 12
        let seq_spec = df.channels.iter().find(|c| c.name == "seq_ch").unwrap();
        assert!(seq_spec.sequential_access);
        assert!(matches!(
            seq_spec.storage,
            DataflowChannelStorage::StreamFifo { min_depth } if min_depth >= 12
        ));
        // `rand_buf` is accessed out-of-order (`rand_buf[15 - i]`) -> 2-bank PingPongRam (PIPO)
        let rand_spec = df.channels.iter().find(|c| c.name == "rand_buf").unwrap();
        assert!(!rand_spec.sequential_access);
        assert_eq!(
            rand_spec.storage,
            DataflowChannelStorage::PingPongRam {
                banks: 2,
                bank_depth: 16
            }
        );
        assert!(df.synchronizer_sv.contains("ap_channel_done_rand_buf"));
        assert!(df.synchronizer_sv.contains("start_for_consumer_unbalanced_din"));
    }

    #[test]
    fn hls_2d_dsp48_systolic_array_pe_grid_synthesis() {
        let cfg = SystolicArrayConfig {
            name: "systolic_3x3".into(),
            rows: 3,
            cols: 3,
            inner_k: 4,
            data_width: 16,
            mode: SystolicDataflowMode::WeightStationary,
        };
        let grid = generate_systolic_pe_grid(&cfg).expect("generate 3x3 systolic grid");
        assert_eq!(grid.pe_grid.len(), 9);
        assert_eq!(grid.total_dsp48, 9);
        assert_eq!(grid.ii_cycles, 1);
        let dsp_cells = grid
            .design
            .cells
            .iter()
            .filter(|c| matches!(c.kind, CellKind::Mac27 | CellKind::Dsp48e1 { .. }))
            .count();
        assert_eq!(dsp_cells, 9, "3x3 systolic PE grid must synthesize 9 DSP48E1 cells");
        // Verify vertical PCIN/BCOUT cascade wiring on row > 0
        let pe_2_1 = grid.pe_grid.iter().find(|p| p.row == 2 && p.col == 1).unwrap();
        assert_eq!(pe_2_1.b_in_net, "pe_bcout_1_1");
        assert_eq!(pe_2_1.pcin_net.as_deref(), Some("pe_pcout_1_1"));

        let a = vec![
            vec![1, 2, 3, 4],
            vec![2, 1, 0, 1],
            vec![0, 3, 2, 1],
        ];
        let b = vec![
            vec![1, 0, 2],
            vec![0, 1, 1],
            vec![2, 1, 0],
            vec![1, 2, 1],
        ];
        let c = grid.execute_matmul(&a, &b);
        assert_eq!(c[0], vec![11, 13, 8]);
        assert_eq!(c[1], vec![3, 3, 6]);
        assert_eq!(c[2], vec![5, 7, 4]);
    }

    #[test]
    fn hls_axilite_csr_map_and_bare_metal_c_driver_emission() {
        let src = r#"
int conv1d_csr(int *in_ptr, int *out_ptr, int coeff, int bias) {
  #pragma HLS INTERFACE s_axilite port=return bundle=control
  #pragma HLS INTERFACE m_axi port=in_ptr bundle=gmem0
  #pragma HLS INTERFACE m_axi port=out_ptr bundle=gmem0
  #pragma HLS INTERFACE s_axilite port=coeff bundle=control
  #pragma HLS INTERFACE s_axilite port=bias bundle=control
  *out_ptr = (*in_ptr) * coeff + bias;
  return *out_ptr;
}
"#;
        let hls = hls_compile(src).expect("compile conv1d_csr");
        let (csr_map, c_header) = emit_axilite_c_driver(&hls);
        assert_eq!(csr_map.registers[0].offset, 0x00);
        assert_eq!(csr_map.registers[0].name, "AP_CTRL");
        assert_eq!(csr_map.registers[1].offset, 0x04);
        assert_eq!(csr_map.registers[1].name, "GIE");
        assert_eq!(csr_map.registers[2].offset, 0x08);
        assert_eq!(csr_map.registers[2].name, "IER");
        assert_eq!(csr_map.registers[3].offset, 0x0C);
        assert_eq!(csr_map.registers[3].name, "ISR");
        assert!(c_header.contains("#define XCONV1D_CSR_CONTROL_ADDR_AP_CTRL 0x00"));
        assert!(c_header.contains("#define XCONV1D_CSR_CONTROL_ADDR_GIE 0x04"));
        assert!(c_header.contains("#define XCONV1D_CSR_CONTROL_ADDR_IER 0x08"));
        assert!(c_header.contains("#define XCONV1D_CSR_CONTROL_ADDR_ISR 0x0c"));
        assert!(c_header.contains("#define XCONV1D_CSR_CONTROL_ADDR_COEFF_DATA"));
        assert!(c_header.contains("#define XCONV1D_CSR_CONTROL_ADDR_BIAS_DATA"));
        assert!(c_header.contains("Xconv1d_csr_Start"));
        assert!(c_header.contains("Xconv1d_csr_IsDone"));
    }
}

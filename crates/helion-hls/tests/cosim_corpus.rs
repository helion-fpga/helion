//! Every kernel in the HLS test corpus through real C/RTL co-simulation
//! (synthesized netlist in helion-sim vs the C reference).
//!
//! Supported kernels must PASS; kernels whose interfaces the FSMD emitter
//! does not implement (AXI-Stream, m_axi, s_axilite, DATAFLOW, math
//! intrinsics) must be refused with a reason instead of "passing".

use helion_hls::{CosimStatus, CosimTestVector, HlsConfig, parse_tcl_config, run_rtl_cosim_with};

fn v() -> CosimTestVector {
    CosimTestVector::new()
}

fn seq(n: i64, f: impl Fn(i64) -> i64) -> Vec<i64> {
    (0..n).map(f).collect()
}

struct Case {
    name: &'static str,
    src: &'static str,
    tcl: Option<&'static str>,
    vectors: Vec<CosimTestVector>,
    /// `None` = must pass; `Some(substr)` = must be refused mentioning it.
    refused: Option<&'static str>,
}

fn corpus() -> Vec<Case> {
    vec![
        Case { name: "addab", src: "int addab(int a, int b) { return a + b; }", tcl: None, vectors: vec![v().with_scalar("a", 5).with_scalar("b", 7), v().with_scalar("a", -20).with_scalar("b", 4)], refused: None },
        Case { name: "k4_xor_mask", src: "int k4(int a, int b) { return (a ^ b) & 255; }", tcl: None, vectors: vec![v().with_scalar("a", 6).with_scalar("b", 300)], refused: None },
        Case { name: "mac1", src: "int mac1(int a, int b) { return a * b; }", tcl: None, vectors: vec![v().with_scalar("a", 6).with_scalar("b", -7)], refused: None },
        Case {
            name: "clamp",
            src: "int clampv(int x, int lo, int hi) { if (x < lo) return lo; if (x > hi) return hi; return x; }",
            tcl: None,
            vectors: vec![
                v().with_scalar("x", 50).with_scalar("lo", 0).with_scalar("hi", 10),
                v().with_scalar("x", -5).with_scalar("lo", 0).with_scalar("hi", 10),
                v().with_scalar("x", -20).with_scalar("lo", -30).with_scalar("hi", -10),
            ],
            refused: None,
        },
        Case {
            name: "fir_mac_cosim",
            src: "void fir_mac_cosim(int a[4], int b[4], int bias, int *out) {\n  #pragma HLS PIPELINE II=1\n  #pragma HLS ARRAY_PARTITION variable=a complete\n  #pragma HLS ARRAY_PARTITION variable=b complete\n  int acc = bias;\n  for (int i = 0; i < 4; i++) {\n    acc += a[i] * b[i];\n  }\n  *out = acc;\n}\n",
            tcl: None,
            vectors: vec![
                v().with_array("a", vec![1, 2, 3, 4]).with_array("b", vec![10, 20, 30, 40]).with_scalar("bias", 5),
                v().with_array("a", vec![-3, 7, 0, 9]).with_array("b", vec![2, -5, 11, 1]).with_scalar("bias", -1),
            ],
            refused: None,
        },
        Case {
            name: "fir_filter",
            src: "#define N_TAPS 4\nvoid fir_filter(int *y, int x) {\n  #pragma HLS INTERFACE ap_ctrl_hs port=return\n  #pragma HLS INTERFACE ap_vld port=y\n  #pragma HLS PIPELINE II=1\n  static int shift_reg[N_TAPS] = {0, 0, 0, 0};\n  #pragma HLS ARRAY_PARTITION variable=shift_reg type=complete dim=1\n  const int coeff[N_TAPS] = {3, 7, 11, 5};\n  #pragma HLS ARRAY_PARTITION variable=coeff type=complete dim=1\n\n  int acc = 0;\n  SHIFT_LOOP: for (int i = 3; i >= 1; i--) {\n    shift_reg[i] = shift_reg[i - 1];\n  }\n  shift_reg[0] = x;\n\n  MAC_LOOP: for (int i = 0; i < 4; i++) {\n    acc += shift_reg[i] * coeff[i];\n  }\n  *y = acc;\n}\n",
            tcl: None,
            vectors: vec![v().with_scalar("x", 5), v().with_scalar("x", -9)],
            refused: None,
        },
        Case {
            name: "sum_rolled_loop",
            src: "int sum8(int a[8]) {\n  int s = 0;\n  for (int i = 0; i < 8; i++) {\n    s += a[i];\n  }\n  return s;\n}\n",
            tcl: None,
            vectors: vec![v().with_array("a", seq(8, |i| i * 3 - 7))],
            refused: None,
        },
        Case {
            name: "matmul_unpartitioned",
            src: "void matmul(int A[4][4], int B[4][4], int C[4][4]) {\n  ROW: for (int i = 0; i < 2; i++) {\n    COL: for (int j = 0; j < 2; j++) {\n      #pragma HLS PIPELINE II=1\n      int sum = 0;\n      DOT: for (int k = 0; k < 4; k++) {\n        sum += A[i][k] * B[k][j];\n      }\n      C[i][j] = sum;\n    }\n  }\n}\n",
            tcl: None,
            vectors: vec![v().with_array("A", seq(16, |i| i + 1)).with_array("B", seq(16, |i| 2 - i))],
            refused: None,
        },
        Case {
            name: "matmul_partitioned",
            src: "void matmul(int A[4][4], int B[4][4], int C[4][4]) {\n  #pragma HLS ARRAY_PARTITION variable=A type=cyclic factor=4 dim=2\n  #pragma HLS ARRAY_PARTITION variable=B type=block factor=4 dim=1\n  ROW: for (int i = 0; i < 2; i++) {\n    COL: for (int j = 0; j < 2; j++) {\n      #pragma HLS PIPELINE II=1\n      int sum = 0;\n      DOT: for (int k = 0; k < 4; k++) {\n        sum += A[i][k] * B[k][j];\n      }\n      C[i][j] = sum;\n    }\n  }\n}\n",
            tcl: None,
            vectors: vec![v().with_array("A", seq(16, |i| i + 1)).with_array("B", seq(16, |i| 2 - i))],
            refused: None,
        },
        Case {
            name: "dot4_allocation_limit",
            src: "void dot4(int a[4], int b[4], int *out) {\n  #pragma HLS ARRAY_PARTITION variable=a complete\n  #pragma HLS ARRAY_PARTITION variable=b complete\n  #pragma HLS ALLOCATION instances=mul limit=2 operation\n  #pragma HLS PIPELINE II=1\n  int acc = 0;\n  LOOP: for (int i = 0; i < 4; i++) {\n    acc += a[i] * b[i];\n  }\n  *out = acc;\n}\n",
            tcl: None,
            vectors: vec![v().with_array("a", vec![1, 2, 3, 4]).with_array("b", vec![4, 3, 2, 1])],
            refused: None,
        },
        Case {
            name: "vec_mac_tcl_ap_ctrl_none",
            src: "void vec_mac(int a[8], int b[8], int *out) {\n  int acc = 0;\n  MAC_LOOP: for (int i = 0; i < 8; i++) {\n    acc += a[i] * b[i];\n  }\n  *out = acc;\n}\n",
            tcl: Some("create_clock -period 8.0\nset_top vec_mac\nset_directive_interface -mode ap_ctrl_none \"vec_mac\" return\nset_directive_array_partition -type cyclic -factor 4 -dim 1 \"vec_mac\" a\nset_directive_array_partition -type block -factor 4 -dim 1 \"vec_mac\" b\nset_directive_unroll -factor 2 \"vec_mac/MAC_LOOP\"\nset_directive_pipeline -II 1 \"vec_mac/MAC_LOOP\"\n"),
            vectors: vec![v().with_array("a", seq(8, |i| i + 1)).with_array("b", seq(8, |i| 3 - i))],
            refused: None,
        },
        Case {
            name: "iir_filter",
            src: "void iir_filter(int x[8], int y[8], int alpha) {\n  int state = 0;\n  IIR_LOOP: for (int i = 0; i < 8; i++) {\n    #pragma HLS PIPELINE II=1\n    state = state * alpha + x[i];\n    y[i] = state;\n  }\n}\n",
            tcl: None,
            vectors: vec![v().with_array("x", seq(8, |i| i - 2)).with_scalar("alpha", 2)],
            refused: None,
        },
        Case {
            name: "hist_update_inout",
            src: "void hist_update(int hist[16], int scale) {\n  HIST_LOOP: for (int i = 1; i < 16; i++) {\n    #pragma HLS PIPELINE II=1\n    hist[i] = hist[i - 1] * scale + 1;\n  }\n}\n",
            tcl: None,
            vectors: vec![v().with_array("hist", seq(16, |i| i)).with_scalar("scale", 2)],
            refused: None,
        },
        Case {
            name: "chained_alu",
            src: "void chained_alu(int a, int b, int c, int d, int *y) {\n  #pragma HLS INTERFACE ap_ctrl_hs port=return\n  int t1 = a + b;\n  int t2 = t1 ^ c;\n  int t3 = t2 + d;\n  int t4 = t3 * a;\n  *y = t4 + b;\n}\n",
            tcl: None,
            vectors: vec![v().with_scalar("a", 3).with_scalar("b", -4).with_scalar("c", 9).with_scalar("d", 100)],
            refused: None,
        },
        Case {
            name: "div_mod_min_max_abs",
            src: "int dm(int a, int b) {\n  int q = a / b;\n  int r = a % b;\n  return q * 100 + r + max(a, b) - min(a, b) + abs(b);\n}\n",
            tcl: None,
            vectors: vec![v().with_scalar("a", -47).with_scalar("b", 5), v().with_scalar("a", 47).with_scalar("b", -6), v().with_scalar("a", 9).with_scalar("b", 0)],
            refused: None,
        },
        Case {
            name: "while_shift",
            src: "int bits_of(int x) {\n  int n = 0;\n  while (x != 0) {\n    x = x >> 1;\n    n = n + 1;\n  }\n  return n;\n}\n",
            tcl: None,
            vectors: vec![v().with_scalar("x", 37), v().with_scalar("x", 1)],
            refused: None,
        },
        Case {
            name: "stream_accel",
            src: "void stream_accel(hls::stream<int> &in_s, hls::stream<int> &out_s, int *dram_ptr, int *fifo_out, int gain, int bias) {\n  #pragma HLS INTERFACE s_axilite port=return bundle=control\n  #pragma HLS INTERFACE axis port=in_s\n  #pragma HLS INTERFACE axis port=out_s\n  #pragma HLS INTERFACE m_axi port=dram_ptr bundle=gmem0 depth=64\n  #pragma HLS INTERFACE ap_fifo port=fifo_out\n  int x = in_s.read();\n  int y = x * gain + bias + *dram_ptr;\n  *fifo_out = y;\n  out_s.write(y);\n}\n",
            tcl: None,
            vectors: vec![v().with_scalar("gain", 2).with_scalar("bias", 1)],
            refused: Some("interface shell"),
        },
        Case {
            name: "axi_burst_copy",
            src: "void axi_burst_copy(int *in_dram, int *out_dram) {\n  #pragma HLS INTERFACE m_axi port=in_dram bundle=gmem0\n  #pragma HLS INTERFACE m_axi port=out_dram bundle=gmem1\n  BURST_LOOP: for (int i = 0; i < 32; i++) {\n    #pragma HLS PIPELINE II=1\n    *out_dram = *in_dram + 5;\n  }\n}\n",
            tcl: None,
            vectors: vec![v()],
            refused: Some("interface shell"),
        },
        Case {
            name: "top_dataflow",
            src: "int scale_elem(int x, int factor) {\n  #pragma HLS INLINE\n  return x * factor;\n}\nvoid stage_load(int in_arr[4], int buf[4], int k) {\n  for (int i = 0; i < 4; i++) {\n    buf[i] = scale_elem(in_arr[i], k);\n  }\n}\nvoid stage_compute(int buf[4], int out_arr[4]) {\n  for (int i = 0; i < 4; i++) {\n    out_arr[i] = buf[i] + 1;\n  }\n}\nvoid top_dataflow(int in_arr[4], int out_arr[4], int k) {\n  #pragma HLS DATAFLOW\n  int mid_buf[4];\n  stage_load(in_arr, mid_buf, k);\n  stage_compute(mid_buf, out_arr);\n}\n",
            tcl: None,
            vectors: vec![v().with_array("in_arr", vec![1, 2, 3, 4]).with_scalar("k", 3)],
            refused: Some("interface shell"),
        },
    ]
}

#[test]
fn every_corpus_kernel_through_rtl_cosim() {
    let mut table = Vec::new();
    let mut bad = Vec::new();
    for c in corpus() {
        let cfg = match c.tcl {
            Some(t) => parse_tcl_config(t).expect("tcl"),
            None => HlsConfig::default(),
        };
        let t0 = std::time::Instant::now();
        let r = run_rtl_cosim_with(c.src, &cfg, &c.vectors);
        eprintln!("corpus {} done in {:?}", c.name, t0.elapsed());
        let line = match (&r, c.refused) {
            (Ok(rep), None) => {
                let ok = rep.status == CosimStatus::Pass;
                if !ok {
                    bad.push(format!("{}: {}", c.name, rep.report_text));
                }
                format!("{:<26} {} ({}/{} vectors, latency {}..{})", c.name, if ok { "PASS" } else { "FAIL" }, rep.vectors_passed, rep.vectors_tested, rep.min_latency_cycles, rep.max_latency_cycles)
            }
            (Err(e), Some(want)) => {
                if !e.contains(want) {
                    bad.push(format!("{}: refused for the wrong reason: {e}", c.name));
                }
                format!("{:<26} REFUSED ({})", c.name, e.split(';').next().unwrap_or(e))
            }
            (Ok(rep), Some(_)) => {
                bad.push(format!("{}: expected a refusal, got {:?}", c.name, rep.status));
                format!("{:<26} UNEXPECTED {:?}", c.name, rep.status)
            }
            (Err(e), None) => {
                bad.push(format!("{}: {e}", c.name));
                format!("{:<26} ERROR {e}", c.name)
            }
        };
        table.push(line);
    }
    eprintln!("C/RTL co-simulation corpus:\n{}", table.join("\n"));
    assert!(bad.is_empty(), "{}", bad.join("\n\n"));
}

use helion_ir::{resolve, CellKind};
use helion_sv::synth_sv;

#[test]
fn ug901_use_dsp_yes_no_logic_simd() {
    // 1. `use_dsp = "yes"` on a standalone adder forces a DSP48E1 with USE_MULT="NONE".
    let sv_yes = r#"
module dsp_add (
    input  wire        clk,
    input  wire [15:0] a,
    input  wire [15:0] b,
    (* use_dsp = "yes" *)
    output reg  [15:0] sum
);
    always @(posedge clk) begin
        sum <= a + b;
    end
endmodule
"#;
    let d_yes = synth_sv(sv_yes, "dsp_add.v").expect("synth dsp_add");
    let dsp = d_yes
        .cells
        .iter()
        .find(|c| matches!(c.kind, CellKind::Dsp48e1 { .. }))
        .expect("use_dsp=yes must infer Dsp48e1 for adder");
    assert_eq!(dsp.attrs.get("USE_MULT"), Some("NONE"));
    assert_eq!(dsp.attrs.get("USE_DSP"), Some("YES"));
    assert_eq!(dsp.attrs.get("PREG"), Some("1"));

    // 2. `use_dsp = "no"` on a multiplier inhibits DSP48E1 and maps to fabric LUT6s.
    let sv_no = r#"
module mul_no_dsp (
    input  wire [3:0] a,
    input  wire [3:0] b,
    (* use_dsp = "no" *)
    output wire [7:0] prod
);
    assign prod = a * b;
endmodule
"#;
    let d_no = synth_sv(sv_no, "mul_no_dsp.v").expect("synth mul_no_dsp");
    assert!(
        !d_no
            .cells
            .iter()
            .any(|c| matches!(c.kind, CellKind::Dsp48e1 { .. } | CellKind::Mac27)),
        "use_dsp=no must inhibit Dsp48e1"
    );
    assert_eq!(d_no.attrs.get("USE_DSP"), Some("NO"));
    let luts = d_no
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Lut6 { .. }))
        .count();
    assert!(luts >= 4, "use_dsp=no multiplier must emit fabric LUT6s, got {luts}");

    // 3. `use_dsp = "simd"` infers SIMD DSP48E1 (TWO24 / FOUR12).
    let sv_simd = r#"
module simd_add (
    input  wire        clk,
    input  wire [23:0] a,
    input  wire [23:0] b,
    (* use_dsp = "simd" *)
    output reg  [23:0] y
);
    always @(posedge clk) begin
        y <= a + b;
    end
endmodule
"#;
    let d_simd = synth_sv(sv_simd, "simd_add.v").expect("synth simd_add");
    let simd_dsp = d_simd
        .cells
        .iter()
        .find(|c| matches!(c.kind, CellKind::Dsp48e1 { .. }))
        .expect("use_dsp=simd must infer Dsp48e1");
    assert_eq!(simd_dsp.attrs.get("USE_SIMD"), Some("TWO24"));
    assert_eq!(simd_dsp.attrs.get("USE_DSP"), Some("SIMD"));
}

#[test]
fn ug901_dsp_mreg_preg_pipeline_and_pcout_pcin_cascade() {
    // 1. MREG + PREG pipeline absorption: `m_reg <= a * b; p_reg <= m_reg + c`
    let sv_pipe = r#"
module dsp_pipelined (
    input  wire        clk,
    input  wire [15:0] a,
    input  wire [15:0] b,
    input  wire [31:0] c,
    output reg  [31:0] p_reg
);
    reg [15:0] a_reg, b_reg;
    reg [31:0] m_reg;
    always @(posedge clk) begin
        a_reg <= a;
        b_reg <= b;
        m_reg <= a_reg * b_reg;
        p_reg <= m_reg + c;
    end
endmodule
"#;
    let d_pipe = synth_sv(sv_pipe, "dsp_pipelined.v").expect("synth dsp_pipelined");
    let dsps: Vec<_> = d_pipe
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Dsp48e1 { .. }))
        .collect();
    assert_eq!(dsps.len(), 1, "pipelined MREG+PREG MAC must fold into 1 DSP48E1");
    assert_eq!(dsps[0].attrs.get("MREG"), Some("1"));
    assert_eq!(dsps[0].attrs.get("PREG"), Some("1"));
    assert_eq!(dsps[0].attrs.get("AREG"), Some("1"));
    assert_eq!(dsps[0].attrs.get("BREG"), Some("1"));

    // 2. PCOUT -> PCIN cascade across chained DSPs: `p0 = a0 * b0; p1 = a1 * b1 + p0`
    let sv_casc = r#"
module dsp_cascade (
    input  wire        clk,
    input  wire [15:0] a0,
    input  wire [15:0] b0,
    input  wire [15:0] a1,
    input  wire [15:0] b1,
    output reg  [31:0] p1
);
    reg [31:0] p0;
    always @(posedge clk) begin
        p0 <= a0 * b0;
        p1 <= a1 * b1 + p0;
    end
endmodule
"#;
    let d_casc = synth_sv(sv_casc, "dsp_cascade.v").expect("synth dsp_cascade");
    assert_eq!(d_casc.attrs.get("DSP_CASCADE"), Some("1"));
    let casc_dsps: Vec<_> = d_casc
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Dsp48e1 { .. }))
        .collect();
    assert_eq!(casc_dsps.len(), 2, "2-tap MAC chain must emit 2 cascaded DSP48E1s");
    assert!(
        casc_dsps.iter().any(|c| matches!(c.kind, CellKind::Dsp48e1 { opmode: 0b0010101, .. })),
        "downstream cascaded DSP must use OPMODE=0b0010101 (Z=PCIN, X:Y=M)"
    );
    let pins = d_casc.pin_index();
    let pcout = pins
        .net_on(resolve(casc_dsps[0].name), "PCOUT")
        .expect("upstream DSP must drive PCOUT");
    let pcin = pins
        .net_on(resolve(casc_dsps[1].name), "PCIN")
        .expect("downstream DSP must read PCIN");
    assert_eq!(pcout, pcin, "PCOUT must wire directly to downstream PCIN");
}

#[test]
fn ug901_ram_style_and_rom_style_modes() {
    // 1. `ram_style = "distributed"` inhibits Bram18e1 even on >=512-bit RAM.
    let sv_dist = r#"
module dist_ram (
    input  wire       clk,
    input  wire       we,
    input  wire [5:0] addr,
    input  wire [7:0] din,
    output reg  [7:0] dout
);
    (* ram_style = "distributed" *)
    reg [7:0] mem [0:63];
    always @(posedge clk) begin
        if (we) mem[addr] <= din;
        dout <= mem[addr];
    end
endmodule
"#;
    let d_dist = synth_sv(sv_dist, "dist_ram.v").expect("synth dist_ram");
    assert_eq!(d_dist.attrs.get("RAM_STYLE"), Some("DISTRIBUTED"));
    assert!(
        !d_dist
            .cells
            .iter()
            .any(|c| matches!(c.kind, CellKind::Bram18 | CellKind::Bram18e1 { .. })),
        "ram_style=distributed must not emit Bram18e1"
    );

    // 2. `ram_style = "registers"` synthesizes HFFs instead of Bram18e1.
    let sv_regs = r#"
module reg_ram (
    input  wire       clk,
    input  wire       we,
    input  wire [2:0] addr,
    input  wire [7:0] din,
    output wire [7:0] dout
);
    (* ram_style = "registers" *)
    reg [7:0] mem [0:7];
    always @(posedge clk) begin
        if (we) mem[addr] <= din;
    end
    assign dout = mem[addr];
endmodule
"#;
    let d_regs = synth_sv(sv_regs, "reg_ram.v").expect("synth reg_ram");
    assert_eq!(d_regs.attrs.get("RAM_STYLE"), Some("REGISTERS"));
    let ffs = d_regs
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    assert!(ffs >= 64, "ram_style=registers 8x8 must emit >=64 HFFs, got {ffs}");

    // 3. `ram_style = "ultra"` emits ULTRA-tagged BRAM.
    let sv_ultra = r#"
module ultra_ram (
    input  wire        clk,
    input  wire        we,
    input  wire [7:0]  addr,
    input  wire [31:0] din,
    output reg  [31:0] dout
);
    (* ram_style = "ultra" *)
    reg [31:0] mem [0:255];
    always @(posedge clk) begin
        if (we) mem[addr] <= din;
        dout <= mem[addr];
    end
endmodule
"#;
    let d_ultra = synth_sv(sv_ultra, "ultra_ram.v").expect("synth ultra_ram");
    assert_eq!(d_ultra.attrs.get("RAM_STYLE"), Some("ULTRA"));
    let uram = d_ultra
        .cells
        .iter()
        .find(|c| matches!(c.kind, CellKind::Bram18e1 { .. }))
        .expect("ram_style=ultra must emit Bram18e1");
    assert_eq!(uram.attrs.get("RAM_STYLE"), Some("ULTRA"));

    // 4. `rom_style = "block"` on a synchronous case ROM infers Bram18e1 with INIT.
    let sv_rom = r#"
module case_rom (
    input  wire       clk,
    input  wire [1:0] addr,
    (* rom_style = "block" *)
    output reg  [7:0] dout
);
    always @(posedge clk) begin
        case (addr)
            2'd0: dout <= 8'hA5;
            2'd1: dout <= 8'h5A;
            2'd2: dout <= 8'hF0;
            default: dout <= 8'h0F;
        endcase
    end
endmodule
"#;
    let d_rom = synth_sv(sv_rom, "case_rom.v").expect("synth case_rom");
    assert_eq!(d_rom.attrs.get("ROM_STYLE"), Some("BLOCK"));
    let brom = d_rom
        .cells
        .iter()
        .find(|c| matches!(c.kind, CellKind::Bram18e1 { .. }))
        .expect("rom_style=block case ROM must emit Bram18e1");
    let init = brom.attrs.get("INIT").unwrap_or("");
    assert!(init.contains("a5") && init.contains("5a"), "ROM INIT must contain a5,5a, got {init}");
}

#[test]
fn ug901_bram_byte_write_enable_and_asymmetric_aspect_ratio() {
    // 1. 4-lane byte-write enable RAM (`if (we[i]) mem[addr][i*8 +: 8] <= din[i*8 +: 8]`).
    let sv_bwe = r#"
module byte_we_ram (
    input  wire        clk,
    input  wire [3:0]  we,
    input  wire [7:0]  addr,
    input  wire [31:0] din,
    output reg  [31:0] dout
);
    reg [31:0] mem [0:255];
    integer i;
    always @(posedge clk) begin
        for (i = 0; i < 4; i = i + 1) begin
            if (we[i])
                mem[addr][i*8 +: 8] <= din[i*8 +: 8];
        end
        dout <= mem[addr];
    end
endmodule
"#;
    let d_bwe = synth_sv(sv_bwe, "byte_we_ram.v").expect("synth byte_we_ram");
    assert_eq!(d_bwe.attrs.get("BYTE_WRITE_ENABLE"), Some("1"));
    let bram = d_bwe
        .cells
        .iter()
        .find(|c| matches!(c.kind, CellKind::Bram18e1 { .. }))
        .expect("byte-write RAM must infer Bram18e1");
    assert_eq!(bram.attrs.get("BYTE_WRITE_WIDTH"), Some("8"));
    assert_eq!(bram.attrs.get("WE_WIDTH"), Some("4"));
    let pins = d_bwe.pin_index();
    let bname = resolve(bram.name);
    for lane in 0..4 {
        assert!(
            pins.net_on(bname, &format!("WEBWE[{lane}]")).is_some(),
            "expected WEBWE[{lane}] connected on byte-write Bram18e1"
        );
    }

    // 2. Asymmetric aspect ratio RAM: 8-bit write, 16-bit dual-word read.
    let sv_asym = r#"
module asym_ram (
    input  wire        clk,
    input  wire        we,
    input  wire [7:0]  waddr,
    input  wire [7:0]  din,
    input  wire [6:0]  raddr,
    output reg  [15:0] dout
);
    (* ram_style = "block" *)
    reg [7:0] mem [0:255];
    always @(posedge clk) begin
        if (we)
            mem[waddr] <= din;
        dout <= {mem[{raddr, 1'b1}], mem[{raddr, 1'b0}]};
    end
endmodule
"#;
    let d_asym = synth_sv(sv_asym, "asym_ram.v").expect("synth asym_ram");
    assert_eq!(d_asym.attrs.get("ASYMMETRIC_BRAM"), Some("1"));
    let abram = d_asym
        .cells
        .iter()
        .find(|c| matches!(c.kind, CellKind::Bram18e1 { .. }))
        .expect("asymmetric RAM must infer Bram18e1");
    match abram.kind {
        CellKind::Bram18e1 {
            read_width_a,
            write_width_a,
            ..
        } => {
            assert_eq!(write_width_a, 9, "8-bit write maps to 9-bit BRAM write width");
            assert_eq!(read_width_a, 18, "16-bit read maps to 18-bit BRAM read width");
        }
        _ => unreachable!(),
    }
}

#[test]
fn ug901_shreg_extract_and_srl_style_srl16e_srlc32e() {
    // 1. 16-stage dynamic shift register -> SRL16E
    let sv_srl16 = r#"
module dyn_srl16 (
    input  wire       clk,
    input  wire       ce,
    input  wire       din,
    input  wire [3:0] addr,
    output wire       dout
);
    (* shreg_extract = "yes", srl_style = "srl" *)
    reg [15:0] sr;
    always @(posedge clk) begin
        if (ce)
            sr <= {sr[14:0], din};
    end
    assign dout = sr[addr];
endmodule
"#;
    let d_srl16 = synth_sv(sv_srl16, "dyn_srl16.v").expect("synth dyn_srl16");
    assert_eq!(d_srl16.attrs.get("SHREG_EXTRACT"), Some("YES"));
    assert_eq!(d_srl16.attrs.get("SRL_TYPE"), Some("SRL16E"));
    let srl_cell = d_srl16
        .cells
        .iter()
        .find(|c| c.attrs.get("SRL_TYPE") == Some("SRL16E"))
        .expect("must emit SRL16E cell");
    let pins = d_srl16.pin_index();
    let cname = resolve(srl_cell.name);
    assert!(pins.net_on(cname, "CLK").is_some());
    assert!(pins.net_on(cname, "CE").is_some());
    assert!(pins.net_on(cname, "D").is_some());
    assert!(pins.net_on(cname, "Q").is_some());
    assert!(pins.net_on(cname, "A3").is_some());

    // 2. 32-stage shift register with `srl_style = "reg_srl_reg"` -> head HFF + SRLC32E + tail HFF
    let sv_srl32 = r#"
module dyn_srl32 (
    input  wire       clk,
    input  wire       ce,
    input  wire       din,
    input  wire [4:0] addr,
    output wire       dout
);
    (* shreg_extract = "yes", srl_style = "reg_srl_reg" *)
    reg [31:0] sr;
    always @(posedge clk) begin
        if (ce)
            sr <= {sr[30:0], din};
    end
    assign dout = sr[addr];
endmodule
"#;
    let d_srl32 = synth_sv(sv_srl32, "dyn_srl32.v").expect("synth dyn_srl32");
    assert_eq!(d_srl32.attrs.get("SRL_STYLE"), Some("REG_SRL_REG"));
    assert!(
        d_srl32
            .cells
            .iter()
            .any(|c| c.attrs.get("SRL_TYPE") == Some("SRLC32E")),
        "30-stage inner SRL must emit SRLC32E"
    );
    let ffs = d_srl32
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    assert_eq!(ffs, 2, "reg_srl_reg must emit 1 head HFF + 1 tail HFF, got {ffs}");

    // 3. `shreg_extract = "no"` on unpacked shift register -> HFF chain (no SRL)
    let sv_no_srl = r#"
module shreg_no (
    input  wire       clk,
    input  wire [3:0] din,
    output wire [3:0] dout
);
    (* shreg_extract = "no" *)
    reg [3:0] sr [0:3];
    integer i;
    always @(posedge clk) begin
        sr[0] <= din;
        for (i = 1; i < 4; i = i + 1)
            sr[i] <= sr[i-1];
    end
    assign dout = sr[3];
endmodule
"#;
    let d_no_srl = synth_sv(sv_no_srl, "shreg_no.v").expect("synth shreg_no");
    assert_eq!(d_no_srl.attrs.get("SHREG_EXTRACT"), Some("NO"));
    assert!(
        !d_no_srl
            .cells
            .iter()
            .any(|c| c.attrs.get("SRL_TYPE").is_some()),
        "shreg_extract=no must not emit SRL cells"
    );
    let no_srl_ffs = d_no_srl
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    assert_eq!(no_srl_ffs, 16, "4 stages x 4 bits must emit 16 HFFs");
}

#[test]
fn ug901_fsm_encoding_and_safe_state_recovery() {
    // 4-state FSM encoded with `fsm_encoding = "one_hot"` and `fsm_safe_state = "reset_state"`
    let sv_onehot = r#"
module fsm_onehot (
    input  wire clk,
    input  wire rst,
    input  wire go,
    output wire busy
);
    typedef enum logic [1:0] {
        S_IDLE = 2'd0,
        S_FETCH = 2'd1,
        S_EXEC = 2'd2,
        S_DONE = 2'd3
    } state_t;

    (* fsm_encoding = "one_hot", fsm_safe_state = "reset_state" *)
    state_t state_q;

    always @(posedge clk) begin
        if (rst) begin
            state_q <= S_IDLE;
        end else begin
            case (state_q)
                S_IDLE:  state_q <= go ? S_FETCH : S_IDLE;
                S_FETCH: state_q <= S_EXEC;
                S_EXEC:  state_q <= S_DONE;
                S_DONE:  state_q <= S_IDLE;
                default: state_q <= S_IDLE;
            endcase
        end
    end

    assign busy = (state_q == S_FETCH) || (state_q == S_EXEC);
endmodule
"#;
    let d_oh = synth_sv(sv_onehot, "fsm_onehot.v").expect("synth fsm_onehot");
    assert_eq!(d_oh.attrs.get("FSM_ENCODING"), Some("ONE_HOT"));
    assert_eq!(d_oh.attrs.get("FSM_SAFE_STATE"), Some("RESET_STATE"));
    let ffs: Vec<_> = d_oh
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .collect();
    assert_eq!(
        ffs.len(),
        4,
        "4-state one_hot FSM must widen state_q from 2 bits to 4 HFFs, got {}",
        ffs.len()
    );
    assert!(
        ffs.iter().all(|c| c.attrs.get("FSM_ENCODING") == Some("ONE_HOT")
            && c.attrs.get("FSM_SAFE_STATE") == Some("RESET_STATE")),
        "state HFFs must carry FSM_ENCODING and FSM_SAFE_STATE attributes"
    );

    // Gray-encoded FSM
    let sv_gray = r#"
module fsm_gray (
    input  wire clk,
    input  wire rst,
    input  wire step_en,
    output wire done
);
    localparam [1:0] S0 = 2'd0, S1 = 2'd1, S2 = 2'd2, S3 = 2'd3;
    (* fsm_encoding = "gray", fsm_safe_state = "reset_state" *)
    reg [1:0] state;

    always @(posedge clk) begin
        if (rst)
            state <= S0;
        else if (step_en) begin
            case (state)
                S0: state <= S1;
                S1: state <= S2;
                S2: state <= S3;
                S3: state <= S0;
            endcase
        end
    end
    assign done = (state == S3);
endmodule
"#;
    let d_gray = synth_sv(sv_gray, "fsm_gray.v").expect("synth fsm_gray");
    assert_eq!(d_gray.attrs.get("FSM_ENCODING"), Some("GRAY"));
    assert_eq!(d_gray.attrs.get("FSM_SAFE_STATE"), Some("RESET_STATE"));
    let gray_ffs = d_gray
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    assert_eq!(gray_ffs, 2, "4-state Gray FSM uses 2 HFFs");
}

#[test]
fn ug901_srlc32e_q31_cascade_johnson_fsm_and_module_use_dsp_logic() {
    // 1. 64-stage shift register -> 2 cascaded SRLC32E cells wired via Q31 -> D.
    let sv_srl64 = r#"
module srl64_cascade (
    input  wire       clk,
    input  wire       ce,
    input  wire       din,
    input  wire [5:0] addr,
    output wire       dout
);
    (* shreg_extract = "yes", srl_style = "srl" *)
    reg [63:0] sr;
    always @(posedge clk) begin
        if (ce)
            sr <= {sr[62:0], din};
    end
    assign dout = sr[addr];
endmodule
"#;
    let d_srl64 = synth_sv(sv_srl64, "srl64_cascade.v").expect("synth srl64_cascade");
    let srl32s: Vec<_> = d_srl64
        .cells
        .iter()
        .filter(|c| c.attrs.get("SRL_TYPE") == Some("SRLC32E"))
        .collect();
    assert_eq!(srl32s.len(), 2, "64-stage SRL must cascade 2 SRLC32E cells");
    let pins = d_srl64.pin_index();
    let q31 = pins
        .net_on(resolve(srl32s[0].name), "Q31")
        .expect("first SRLC32E must drive Q31");
    let d_next = pins
        .net_on(resolve(srl32s[1].name), "D")
        .expect("second SRLC32E must read D");
    assert_eq!(q31, d_next, "first SRLC32E Q31 must connect to second SRLC32E D");

    // 2. 6-state FSM with `fsm_encoding = "johnson"` -> widens to 3 Johnson bits.
    let sv_johnson = r#"
module fsm_johnson (
    input  wire clk,
    input  wire rst,
    output wire active
);
    localparam [2:0] S0 = 3'd0, S1 = 3'd1, S2 = 3'd2, S3 = 3'd3, S4 = 3'd4, S5 = 3'd5;
    (* fsm_encoding = "johnson" *)
    reg [2:0] state;
    always @(posedge clk) begin
        if (rst) state <= S0;
        else begin
            case (state)
                S0: state <= S1;
                S1: state <= S2;
                S2: state <= S3;
                S3: state <= S4;
                S4: state <= S5;
                default: state <= S0;
            endcase
        end
    end
    assign active = (state == S3);
endmodule
"#;
    let d_j = synth_sv(sv_johnson, "fsm_johnson.v").expect("synth fsm_johnson");
    assert_eq!(d_j.attrs.get("FSM_ENCODING"), Some("JOHNSON"));
    let j_ffs = d_j
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    assert_eq!(j_ffs, 3, "6-state Johnson FSM uses 3 HFFs");

    // 3. Module-level `(* use_dsp = "logic" *)` before `module`
    let sv_logic = r#"
(* use_dsp = "logic" *)
module mul_logic_mod (
    input  wire        clk,
    input  wire [3:0]  a,
    input  wire [3:0]  b,
    output reg  [7:0]  p
);
    always @(posedge clk) begin
        p <= a * b;
    end
endmodule
"#;
    let d_logic = synth_sv(sv_logic, "mul_logic_mod.v").expect("synth mul_logic_mod");
    assert_eq!(d_logic.attrs.get("USE_DSP"), Some("LOGIC"));
    assert!(
        !d_logic
            .cells
            .iter()
            .any(|c| matches!(c.kind, CellKind::Dsp48e1 { .. } | CellKind::Mac27)),
        "module-level use_dsp=logic must inhibit Dsp48e1"
    );
    let l_ffs = d_logic
        .cells
        .iter()
        .filter(|c| matches!(c.kind, CellKind::Hff))
        .count();
    assert_eq!(l_ffs, 8, "clocked 4x4 use_dsp=logic multiplier must emit 8 output HFFs");
}

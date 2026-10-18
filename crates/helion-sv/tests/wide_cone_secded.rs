use std::path::PathBuf;

use helion_ir::{resolve, Design};
use helion_sv::{synth_sv, synth_sv_path};

fn is_driver_pin(pin: &str) -> bool {
    matches!(
        pin,
        "O" | "Q"
            | "PAD"
            | "O0"
            | "O1"
            | "O2"
            | "O3"
            | "CO0"
            | "CO1"
            | "CO2"
            | "CO3"
            | "DOADO"
            | "DOBDO"
            | "P"
    )
}

fn net_has_driver(d: &Design, net_name: &str) -> bool {
    d.net(net_name)
        .is_some_and(|n| n.endpoints.iter().any(|e| is_driver_pin(resolve(e.pin))))
}

fn net_is_connected(d: &Design, net_name: &str) -> bool {
    d.net(net_name).is_some_and(|n| !n.endpoints.is_empty())
}

fn extract_sv_module<'a>(src: &'a str, mod_name: &str) -> &'a str {
    let needle = format!("module {mod_name}");
    let start = src
        .find(&needle)
        .unwrap_or_else(|| panic!("module {mod_name} not found"));
    let rest = &src[start..];
    let end = rest
        .find("endmodule")
        .unwrap_or_else(|| panic!("endmodule for {mod_name} not found"));
    &rest[..end + "endmodule".len()]
}

#[test]
fn explicit_width_cast_prim_secded_enc_lowers_all_bits() {
    let ibex_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/ysyx_ibex.sv");
    let src = std::fs::read_to_string(&ibex_path).expect("read ysyx_ibex.sv");

    for (mod_name, in_w, out_w) in [
        ("prim_secded_64_57_enc", 57usize, 64usize),
        ("prim_secded_inv_64_57_enc", 57usize, 64usize),
        ("prim_secded_22_16_enc", 16usize, 22usize),
    ] {
        let mod_src = extract_sv_module(&src, mod_name);
        let d = synth_sv(mod_src, mod_name).unwrap_or_else(|e| panic!("synth {mod_name}: {e}"));
        assert_ne!(
            d.attrs.get("WIDE_CONE"),
            Some("1"),
            "{mod_name} must not set WIDE_CONE"
        );
        assert_ne!(
            d.attrs.get("ASSIGN_NOT_LOWERED"),
            Some("1"),
            "{mod_name} must not have unlowered assigns"
        );
        for i in 0..in_w {
            let port_net = format!("data_i_{i}");
            assert!(
                net_is_connected(&d, &port_net),
                "{mod_name}: input port {port_net} must be connected (no F6_UNCONNECTED_PORT)"
            );
        }
        assert!(
            net_has_driver(&d, "data_o"),
            "{mod_name}: output port data_o must have a driver (no F1_UNDRIVEN_NET)"
        );
        let pad_drivers = d
            .net("data_o")
            .map(|n| n.endpoints.iter().filter(|e| resolve(e.pin) == "PAD").count())
            .unwrap_or(0);
        assert_eq!(
            pad_drivers, out_w,
            "{mod_name}: output port data_o must have {out_w} IobOut PAD drivers"
        );
        for i in in_w..out_w {
            let port_net = format!("data_o_{i}");
            assert!(
                net_has_driver(&d, &port_net),
                "{mod_name}: parity bit {port_net} must have a LUT driver"
            );
        }
    }
}

#[test]
fn wide_cone_prim_secded_dec_xor_parity_trees_decompose() {
    let ibex_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/ysyx_ibex.sv");
    let src = std::fs::read_to_string(&ibex_path).expect("read ysyx_ibex.sv");

    for (mod_name, in_w, out_w, syn_w) in [
        ("prim_secded_64_57_dec", 64usize, 57usize, 7usize),
        ("prim_secded_inv_64_57_dec", 64usize, 57usize, 7usize),
        ("prim_secded_72_64_dec", 72usize, 64usize, 8usize),
        ("prim_secded_inv_72_64_dec", 72usize, 64usize, 8usize),
        ("prim_secded_22_16_dec", 22usize, 16usize, 6usize),
    ] {
        let mod_src = extract_sv_module(&src, mod_name);
        let d = synth_sv(mod_src, mod_name).unwrap_or_else(|e| panic!("synth {mod_name}: {e}"));
        assert_ne!(
            d.attrs.get("WIDE_CONE"),
            Some("1"),
            "{mod_name} must decompose >6-PI syndrome XOR cones without WIDE_CONE"
        );
        assert_ne!(
            d.attrs.get("ASSIGN_NOT_LOWERED"),
            Some("1"),
            "{mod_name} must not have ASSIGN_NOT_LOWERED"
        );
        for i in 0..in_w {
            let port_net = format!("data_i_{i}");
            assert!(
                net_is_connected(&d, &port_net),
                "{mod_name}: input port {port_net} must be connected"
            );
        }
        for i in 0..out_w {
            let port_net = format!("data_o_{i}");
            assert!(
                net_has_driver(&d, &port_net),
                "{mod_name}: output port {port_net} must be driven"
            );
        }
        for i in 0..syn_w {
            let port_net = format!("syndrome_o_{i}");
            assert!(
                net_has_driver(&d, &port_net),
                "{mod_name}: syndrome port {port_net} must be driven"
            );
        }
        for i in 0..2 {
            let port_net = format!("err_o_{i}");
            assert!(
                net_has_driver(&d, &port_net),
                "{mod_name}: error port {port_net} must be driven"
            );
        }
    }
}

#[test]
fn gen_alu_datapath_widths_6_to_32_lower_without_wide_cone() {
    for w in [6usize, 8, 12, 16, 32] {
        let hi = w - 1;
        let sv = format!(
            r#"
module gen_alu_datapath (
    input  logic [{hi}:0] a,
    input  logic [{hi}:0] b,
    input  logic          cin,
    input  logic [2:0]    op,
    output logic [{hi}:0] y
);
    always_comb begin
        y = '0;
        case (op)
            3'd0: y = a + b + cin;
            3'd1: y = a - b - cin;
            3'd2: y = a & b;
            3'd3: y = a | b;
            3'd4: y = a ^ b;
            3'd5: y = a << 1;
            3'd6: y = a >> 1;
            default: y = a;
        endcase
    end
endmodule
"#
        );
        let d = synth_sv(&sv, "gen_alu_datapath")
            .unwrap_or_else(|e| panic!("gen_alu_datapath W={w}: {e}"));
        assert_ne!(
            d.attrs.get("WIDE_CONE"),
            Some("1"),
            "gen_alu_datapath W={w} must not set WIDE_CONE"
        );
        assert_ne!(
            d.attrs.get("FLATTEN_CAP"),
            Some("1"),
            "gen_alu_datapath W={w} must not set FLATTEN_CAP"
        );
        assert_ne!(
            d.attrs.get("ASSIGN_NOT_LOWERED"),
            Some("1"),
            "gen_alu_datapath W={w} must not set ASSIGN_NOT_LOWERED"
        );
        for i in 0..w {
            assert!(
                net_has_driver(&d, &format!("y_{i}")),
                "gen_alu_datapath W={w}: y_{i} must have a driver"
            );
            assert!(
                net_is_connected(&d, &format!("a_{i}")),
                "gen_alu_datapath W={w}: a_{i} must be connected"
            );
            assert!(
                net_is_connected(&d, &format!("b_{i}")),
                "gen_alu_datapath W={w}: b_{i} must be connected"
            );
        }
        assert!(
            net_is_connected(&d, "cin"),
            "gen_alu_datapath W={w}: cin must be connected"
        );
    }
}

#[test]
fn nested_if_case_sha256_w_mem_and_sha256_top_lower_all_ports() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let w_mem_path = root.join("corpus-pass/sha256/rtl/sha256_w_mem.v");
    let d_wmem = synth_sv_path(&w_mem_path).expect("synth sha256_w_mem.v");
    assert_ne!(
        d_wmem.attrs.get("WIDE_CONE"),
        Some("1"),
        "sha256_w_mem must not set WIDE_CONE"
    );
    for i in 0..32 {
        assert!(
            net_has_driver(&d_wmem, &format!("w_new_{i}")),
            "sha256_w_mem: w_new_{i} must have a driver from Carry4 ripple adder"
        );
        assert!(
            net_has_driver(&d_wmem, &format!("d0_{i}")),
            "sha256_w_mem: d0_{i} (local reg decl in always @*) must have a driver"
        );
        assert!(
            net_has_driver(&d_wmem, &format!("d1_{i}")),
            "sha256_w_mem: d1_{i} (local reg decl in always @*) must have a driver"
        );
    }

    let nested_if_case_sv = r#"
module w_mem_nested_if_case (
    input  wire        clk,
    input  wire        w_mem_we,
    input  wire [1:0]  w_ctr_reg,
    input  wire [7:0]  w_new,
    output reg  [7:0]  w_mem00_reg,
    output reg  [7:0]  w_mem01_reg
);
    reg [7:0] w_mem00_new;
    reg [7:0] w_mem01_new;
    always @* begin : w_mem_update
        w_mem00_new = 8'h00;
        w_mem01_new = 8'h00;
        if (w_mem_we) begin
            case (w_ctr_reg)
                2'd0: w_mem00_new = w_new;
                2'd1: w_mem01_new = w_new;
                default: begin
                end
            endcase
        end
    end
    always @(posedge clk) begin
        if (w_mem_we) begin
            w_mem00_reg <= w_mem00_new;
            w_mem01_reg <= w_mem01_new;
        end
    end
endmodule
"#;
    let d_nested = synth_sv(nested_if_case_sv, "w_mem_nested_if_case")
        .expect("synth w_mem_nested_if_case");
    for i in 0..8 {
        assert!(
            net_has_driver(&d_nested, &format!("w_mem00_new_{i}")),
            "w_mem_nested_if_case: w_mem00_new_{i} must be driven"
        );
        assert!(
            net_has_driver(&d_nested, &format!("w_mem01_new_{i}")),
            "w_mem_nested_if_case: w_mem01_new_{i} must be driven"
        );
        assert!(
            net_is_connected(&d_nested, &format!("w_new_{i}")),
            "w_mem_nested_if_case: w_new_{i} must be connected"
        );
    }
    let sha_all_path = root.join("corpus-pass/sha256/rtl/sha256_all.v");
    let d_sha = synth_sv_path(&sha_all_path).expect("synth sha256_all.v");
    assert_ne!(
        d_sha.attrs.get("WIDE_CONE"),
        Some("1"),
        "sha256 top must not set WIDE_CONE"
    );
    for sig in ["init_new", "next_new", "mode_new", "mode_we", "error"] {
        assert!(
            net_has_driver(&d_sha, sig),
            "sha256 top: {sig} must have a driver"
        );
    }
    for i in 0..32 {
        assert!(
            net_has_driver(&d_sha, &format!("core_t1_{i}")),
            "sha256 top: core_t1_{i} (5-input Carry4 adder) must have a driver"
        );
        assert!(
            net_has_driver(&d_sha, &format!("core_a_reg_{i}")),
            "sha256 top: core_a_reg_{i} must have a driver"
        );
    }
}

#[test]
fn picorv32_mem_la_read_wide_cone_lowers_with_driver() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let pico_path = root.join("third_party/picorv32/picorv32.v");
    let d = synth_sv_path(&pico_path).expect("synth picorv32.v");
    assert_ne!(
        d.attrs.get("WIDE_CONE"),
        Some("1"),
        "picorv32 must not set WIDE_CONE on mem_la_read or cycle counters"
    );
    assert!(
        net_has_driver(&d, "mem_la_read"),
        "picorv32: mem_la_read must have a driver"
    );
    assert!(
        net_has_driver(&d, "mem_la_write"),
        "picorv32: mem_la_write must have a driver"
    );
}

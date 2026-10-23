mod common;
use helion_sv::synth_sv;
#[test]
fn probe(){
    for expr in ["8'sd-5", "8'sd5", "-8'sd5", "8'd-5"] {
        let src = format!("module t(input logic [7:0] a, output logic [7:0] y); assign y = a + {expr}; endmodule");
        let d = synth_sv(&src,"t.sv").unwrap();
        println!("{expr:9} cells={}", d.cells.len());
    }
}

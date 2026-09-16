// Helion-original 4-bit accumulator core. Capstone of the example spine.
// ISA (2-bit op + 2-bit imm): 00 NOP, 01 LOAD, 10 ADD, 11 XOR.
// Permissive in-tree lab (Apache-2.0 OR MIT). Not PicoRV32 / Ibex / SERV.
// Helion-legal. No AXI-as-product.
module hcore (
    input  logic clk,
    output logic led
);
    logic [1:0] pc;
    logic [3:0] acc;
    logic [3:0] instr;
    logic [1:0] op;
    logic [1:0] imm;

    // Four-instruction ROM: LOAD 1, ADD 2, XOR 1, NOP.
    assign instr =
        (pc == 2'd0) ? 4'b0101 :
        (pc == 2'd1) ? 4'b1010 :
        (pc == 2'd2) ? 4'b1101 :
                       4'b0000;
    assign op  = instr[3:2];
    assign imm = instr[1:0];

    always_ff @(posedge clk) begin
        pc <= pc + 1;
        if (op == 2'b01) acc <= {2'b00, imm};
        if (op == 2'b10) acc <= acc + {2'b00, imm};
        if (op == 2'b11) acc <= acc ^ {2'b00, imm};
    end
    assign led = acc[0];
endmodule

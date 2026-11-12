module top (input logic clk, input logic we, input logic [3:0] wa, input logic [3:0] ra, input logic [7:0] wd, output logic [7:0] rd, output logic [7:0] rq);
    logic [7:0] mem [0:15];
    always_ff @(posedge clk) begin
        if (we) mem[wa] <= wd;
        rq <= mem[ra];
    end
    assign rd = mem[ra];
endmodule

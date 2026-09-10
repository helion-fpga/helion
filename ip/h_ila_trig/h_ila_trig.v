// Helion ILA match-unit / trigger FSM (catalog). Not Xilinx AXI / Vivado ILA IP.
// Match LUT INIT encodes Immediate / Rising / Falling on (probe, prev).
// Fabric-resident FSM: Idle → Armed → Fired → Done around a capture window.
module h_ila_trig (
    input  wire clk,
    input  wire arm,
    input  wire probe,
    input  wire [1:0] match_kind, // 0=immediate 1=rising 2=falling
    output wire match_hit,
    output reg  triggered,
    output reg  done
);
    reg prev;
    wire rise = probe & ~prev;
    wire fall = ~probe & prev;
    assign match_hit = (match_kind == 2'd0) ? 1'b1 :
                       (match_kind == 2'd1) ? rise :
                       (match_kind == 2'd2) ? fall : 1'b0;
    always @(posedge clk) begin
        prev <= probe;
        if (!arm) begin
            triggered <= 1'b0;
            done <= 1'b0;
        end else if (!triggered && match_hit) begin
            triggered <= 1'b1;
        end
    end
endmodule

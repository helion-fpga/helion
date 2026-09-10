// Helion-MM add/sub with sticky carry/borrow (not AXI).
// Addr map (word via mm_addr[3:2]): 0=op_a (r/w), 1=op_b (r/w),
// 2=ctrl (w: [0]=do_add pulse, [1]=do_sub pulse, [2]=clear sticky),
// 3=status (r: [0]=carry sticky, [1]=borrow sticky, [2]=busy).
// On add: result <= a+b, sticky carry if cout. On sub: result <= a-b,
// sticky borrow if a<b. Real FF+LUT fabric for catalog proof.
module h_addsub_mm (
    input  wire        clk,
    input  wire        resetn,
    input  wire        mm_valid,
    input  wire        mm_write,
    input  wire [7:0]  mm_addr,
    input  wire [31:0] mm_wdata,
    output reg  [31:0] mm_rdata,
    output wire        mm_ready,
    output reg  [31:0] result,
    output wire        carry,   // sticky unsigned carry
    output wire        borrow   // sticky unsigned borrow
);
    reg [31:0] a_r;
    reg [31:0] b_r;
    reg        do_add;
    reg        do_sub;
    reg        cy_r;
    reg        br_r;

    assign mm_ready = 1'b1;
    assign carry    = cy_r;
    assign borrow   = br_r;

    wire [1:0] sel = mm_addr[3:2];
    wire [32:0] sum33 = {1'b0, a_r} + {1'b0, b_r};
    wire [32:0] dif33 = {1'b0, a_r} - {1'b0, b_r};
    wire a_lt_b = (a_r < b_r);

    always @(posedge clk) begin
        if (!resetn) begin
            a_r      <= 32'h0;
            b_r      <= 32'h0;
            result   <= 32'h0;
            do_add   <= 1'b0;
            do_sub   <= 1'b0;
            cy_r     <= 1'b0;
            br_r     <= 1'b0;
            mm_rdata <= 32'h0;
        end else begin
            if (mm_valid && mm_write) begin
                case (sel)
                    2'b00: a_r <= mm_wdata;
                    2'b01: b_r <= mm_wdata;
                    2'b10: begin
                        if (mm_wdata[2]) begin
                            cy_r <= 1'b0;
                            br_r <= 1'b0;
                        end
                        if (mm_wdata[0])
                            do_add <= 1'b1;
                        if (mm_wdata[1])
                            do_sub <= 1'b1;
                    end
                    default: ;
                endcase
            end

            if (mm_valid && !mm_write) begin
                case (sel)
                    2'b00: mm_rdata <= a_r;
                    2'b01: mm_rdata <= b_r;
                    2'b10: mm_rdata <= {29'h0, (do_add | do_sub), br_r, cy_r};
                    2'b11: mm_rdata <= result;
                endcase
            end

            // Consume add — 33b sum for carry sticky (LUTs)
            if (do_add) begin
                result <= sum33[31:0];
                if (sum33[32])
                    cy_r <= 1'b1;
                do_add <= 1'b0;
            end else if (do_sub) begin
                result <= dif33[31:0];
                if (a_lt_b)
                    br_r <= 1'b1;
                do_sub <= 1'b0;
            end
        end
    end
endmodule

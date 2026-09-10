// Helion-MM loadable accumulator — clear/add via MM regs (not AXI).
// Addr map (word via mm_addr[3:2]): 0=accum (r/w load), 1=addend (w),
// 2=ctrl (w: [0]=add pulse sticky until consumed, [1]=clear), 3=status (r).
// On add: accum <= accum + addend. Real FF+LUT fabric for catalog proof.
module h_accum_mm (
    input  wire        clk,
    input  wire        resetn,
    input  wire        mm_valid,
    input  wire        mm_write,
    input  wire [7:0]  mm_addr,
    input  wire [31:0] mm_wdata,
    output reg  [31:0] mm_rdata,
    output wire        mm_ready,
    output reg  [31:0] accum,
    output wire        overflow  // sticky unsigned wrap detect
);
    reg [31:0] addend_r;
    reg        do_add;
    reg        ov_r;

    assign mm_ready = 1'b1;
    assign overflow = ov_r;

    wire [1:0] sel = mm_addr[3:2];
    wire [32:0] sum33 = {1'b0, accum} + {1'b0, addend_r};

    always @(posedge clk) begin
        if (!resetn) begin
            accum    <= 32'h0;
            addend_r <= 32'h0;
            do_add   <= 1'b0;
            ov_r     <= 1'b0;
            mm_rdata <= 32'h0;
        end else begin
            if (mm_valid && mm_write) begin
                case (sel)
                    2'b00: accum <= mm_wdata;          // load accum
                    2'b01: addend_r <= mm_wdata;       // set addend
                    2'b10: begin
                        if (mm_wdata[1]) begin         // clear
                            accum <= 32'h0;
                            ov_r  <= 1'b0;
                        end
                        if (mm_wdata[0])
                            do_add <= 1'b1;            // request add
                    end
                    default: ;
                endcase
            end

            if (mm_valid && !mm_write) begin
                case (sel)
                    2'b00: mm_rdata <= accum;
                    2'b01: mm_rdata <= addend_r;
                    2'b10: mm_rdata <= {30'h0, ov_r, do_add};
                    2'b11: mm_rdata <= {31'h0, ov_r};
                endcase
            end

            // Consume add request — 33b sum for wrap detect (LUTs)
            if (do_add) begin
                accum  <= sum33[31:0];
                if (sum33[32])
                    ov_r <= 1'b1;
                do_add <= 1'b0;
            end
        end
    end
endmodule

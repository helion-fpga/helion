// Helion-MM capture timer — free-run counter + sticky edge capture (not AXI).
// Addr map (word via mm_addr[3:2]): 0=count (r), 1=capture (r),
// 2=ctrl (w: [0]=enable free-run, [1]=clear count, [2]=clear sticky),
// 3=status (r: [0]=sticky capture valid, [1]=enable).
// External cap_edge (level sampled) rising edge latches count into capture
// and sets sticky until cleared. Real FF+LUT fabric.
module h_timer_cap (
    input  wire        clk,
    input  wire        resetn,
    input  wire        mm_valid,
    input  wire        mm_write,
    input  wire [7:0]  mm_addr,
    input  wire [31:0] mm_wdata,
    output reg  [31:0] mm_rdata,
    output wire        mm_ready,
    input  wire        cap_edge,   // capture input (rising edge sticky)
    output reg  [15:0] count,
    output reg  [15:0] capture,
    output reg         sticky,     // sticky capture-valid
    output wire        irq         // level while sticky
);
    reg        en_r;
    reg        prev_edge;
    reg        have_prev;

    assign mm_ready = 1'b1;
    assign irq      = sticky;

    wire [1:0] sel = mm_addr[3:2];
    wire rise = have_prev && !prev_edge && cap_edge;

    always @(posedge clk) begin
        if (!resetn) begin
            count     <= 16'h0;
            capture   <= 16'h0;
            sticky    <= 1'b0;
            en_r      <= 1'b0;
            prev_edge <= 1'b0;
            have_prev <= 1'b0;
            mm_rdata  <= 32'h0;
        end else begin
            if (mm_valid && mm_write) begin
                case (sel)
                    2'b10: begin
                        en_r <= mm_wdata[0];
                        if (mm_wdata[1])
                            count <= 16'h0;
                        if (mm_wdata[2])
                            sticky <= 1'b0;
                    end
                    default: ;
                endcase
            end

            if (mm_valid && !mm_write) begin
                case (sel)
                    2'b00: mm_rdata <= {16'h0, count};
                    2'b01: mm_rdata <= {16'h0, capture};
                    2'b10: mm_rdata <= {30'h0, sticky, en_r};
                    2'b11: mm_rdata <= {30'h0, en_r, sticky};
                endcase
            end

            // Free-run counter when enabled
            if (en_r)
                count <= count + 16'h1;

            // Rising-edge capture → sticky until clear
            if (rise) begin
                capture <= count;
                sticky  <= 1'b1;
            end
            prev_edge <= cap_edge;
            have_prev <= 1'b1;
        end
    end
endmodule

// Helion-ST quadrature decoder — A/B phase → count up/down (not AXI).
// Samples enc_a/enc_b on st_valid; Gray-transition decode; sticky direction.
module h_quad_enc (
    input  wire        clk,
    input  wire        resetn,
    // Helion-ST sample path
    input  wire        st_valid,
    output wire        st_ready,
    input  wire        enc_a,
    input  wire        enc_b,
    input  wire        clear,       // level; clears count
    output reg  [15:0] count,
    output reg         dir_up,      // 1=last step was up
    output wire        step_pulse,
    output wire        st_out_valid
);
    reg [1:0] prev_ab;
    reg       have_prev;
    reg       step_r;
    reg       out_v;

    assign st_ready     = 1'b1;
    assign step_pulse   = step_r;
    assign st_out_valid = out_v;

    wire [1:0] cur_ab = {enc_a, enc_b};

    always @(posedge clk) begin
        if (!resetn) begin
            prev_ab   <= 2'b00;
            have_prev <= 1'b0;
            count     <= 16'h0;
            dir_up    <= 1'b1;
            step_r    <= 1'b0;
            out_v     <= 1'b0;
        end else begin
            step_r <= 1'b0;
            out_v  <= 1'b0;

            if (clear) begin
                count     <= 16'h0;
                have_prev <= 1'b0;
                prev_ab   <= 2'b00;
                out_v     <= 1'b1;
            end else if (st_valid) begin
                if (have_prev && (cur_ab != prev_ab)) begin
                    // Standard quadrature: 00→01→11→10→00 = up
                    //                     00→10→11→01→00 = down
                    case ({prev_ab, cur_ab})
                        4'b0001, 4'b0111, 4'b1110, 4'b1000: begin
                            count  <= count + 16'h1;
                            dir_up <= 1'b1;
                            step_r <= 1'b1;
                            out_v  <= 1'b1;
                        end
                        4'b0010, 4'b1011, 4'b1101, 4'b0100: begin
                            count  <= count - 16'h1;
                            dir_up <= 1'b0;
                            step_r <= 1'b1;
                            out_v  <= 1'b1;
                        end
                        default: ;
                    endcase
                end
                prev_ab   <= cur_ab;
                have_prev <= 1'b1;
            end
        end
    end
endmodule

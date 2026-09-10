// Helion-ST 4-request round-robin arbiter — grant + sticky mask (not AXI).
// Samples req[3:0] on st_valid; on enable computes one-hot grant with sticky
// rotate mask so the next grant prefers the requestor after the last winner.
// Real FF+LUT fabric for catalog proof.
module h_arb_rr (
    input  wire       clk,
    input  wire       resetn,
    // Helion-ST request path
    input  wire       st_valid,
    output wire       st_ready,
    input  wire [3:0] req,        // 4 requestors
    input  wire       enable,     // compute & register grant
    input  wire       clear_mask, // reset sticky rotate mask
    output reg  [3:0] grant,      // one-hot grant
    output reg  [3:0] mask,       // sticky rotate mask (after last grant)
    output wire       st_out_valid
);
    reg [3:0] req_r;
    reg       out_v;

    assign st_ready     = 1'b1;
    assign st_out_valid = out_v;

    // Double-priority round-robin:
    //   masked = req & ~mask  (requestors still "ahead" of last grant)
    // Prefer masked if any; else fall back to raw req.
    wire [3:0] masked = req_r & ~mask;
    wire       use_masked = |masked;
    wire [3:0] cand = use_masked ? masked : req_r;

    // Fixed-priority encode among candidates (LSB wins when tied)
    wire [3:0] g_comb =
          cand[0] ? 4'b0001 :
          cand[1] ? 4'b0010 :
          cand[2] ? 4'b0100 :
          cand[3] ? 4'b1000 :
                    4'b0000;

    // Next sticky mask: block winners at/below grant; wrap clears on full cycle
    // After granting bit i, mask = {1..1 above i, 0 at/below i} inverted form:
    // classic: mask_next = {grant[2:0], 1'b0} | {3'b0, grant[3]} wait —
    // Use: bits strictly above the grant stay unmasked next; grant and below masked.
    wire [3:0] mask_next =
          g_comb[0] ? 4'b1110 :
          g_comb[1] ? 4'b1100 :
          g_comb[2] ? 4'b1000 :
          g_comb[3] ? 4'b0000 :
                      mask;

    always @(posedge clk) begin
        if (!resetn) begin
            req_r <= 4'h0;
            grant <= 4'h0;
            mask  <= 4'h0;
            out_v <= 1'b0;
        end else begin
            out_v <= 1'b0;
            if (clear_mask)
                mask <= 4'h0;
            if (st_valid)
                req_r <= req;
            else if (enable) begin
                grant <= g_comb;
                if (|g_comb)
                    mask <= mask_next;
                out_v <= 1'b1;
            end
        end
    end
endmodule

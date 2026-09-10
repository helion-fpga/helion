// Helion-ST token-ring / rotating grant among 4 requesters (not AXI).
// Distinct from h_arb_rr sticky-mask priority: an explicit one-hot token
// circulates; grant asserts only when token lands on an active requestor.
// Real FF+LUT fabric for catalog proof.
module h_rr_token (
    input  wire       clk,
    input  wire       resetn,
    // Helion-ST request path
    input  wire       st_valid,
    output wire       st_ready,
    input  wire [3:0] req,         // 4 requestors
    input  wire       enable,      // advance token / evaluate grant
    input  wire       load_token,  // force token from st path
    input  wire [3:0] token_in,    // one-hot token load value
    output reg  [3:0] token,       // rotating one-hot token
    output reg  [3:0] grant,       // one-hot grant (token & req)
    output wire       st_out_valid
);
    reg [3:0] req_r;
    reg       out_v;

    assign st_ready     = 1'b1;
    assign st_out_valid = out_v;

    // Next token: rotate left one position (wrap bit3 → bit0)
    wire [3:0] token_rot = {token[2:0], token[3]};

    always @(posedge clk) begin
        if (!resetn) begin
            req_r <= 4'h0;
            token <= 4'b0001;
            grant <= 4'h0;
            out_v <= 1'b0;
        end else begin
            out_v <= 1'b0;
            if (st_valid)
                req_r <= req;
            if (load_token && (|token_in))
                token <= token_in;
            else if (enable) begin
                // Grant only if current token holder is requesting
                grant <= token & req_r;
                // Pass token every enable tick (free-running ring)
                token <= token_rot;
                out_v <= 1'b1;
            end
        end
    end
endmodule

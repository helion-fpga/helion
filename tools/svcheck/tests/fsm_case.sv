module top (input logic clk, input logic rst_n, input logic go, input logic [1:0] sel, output logic [2:0] state, output logic done, output logic [3:0] cnt);
    typedef enum logic [2:0] {IDLE, LOAD, RUN, WAIT, DONE} st_t;
    st_t s, ns;
    logic [3:0] c;
    always_comb begin
        ns = s;
        case (s)
            IDLE: if (go) ns = LOAD;
            LOAD: ns = RUN;
            RUN:  if (c == 4'd9) ns = (sel == 2'd3) ? WAIT : DONE;
            WAIT: if (!go) ns = DONE;
            DONE: ns = IDLE;
            default: ns = IDLE;
        endcase
    end
    always_ff @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin s <= IDLE; c <= '0; end
        else begin
            s <= ns;
            if (s == LOAD) c <= {2'b0, sel};
            else if (s == RUN) c <= c + 4'd1;
        end
    end
    assign state = s;
    assign done = (s == DONE);
    assign cnt = c;
endmodule

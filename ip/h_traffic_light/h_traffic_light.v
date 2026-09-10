// Helion-ST simple FSM traffic light — registered states + timers (not AXI).
// States: IDLE(0) RED(1) GREEN(2) YELLOW(3). Programmable dwell ticks via ST.
// Outputs: red/yellow/green lamps + state code. Real FF+LUT fabric.
module h_traffic_light (
    input  wire       clk,
    input  wire       resetn,
    // Helion-ST config path
    input  wire       st_valid,
    input  wire [7:0] st_data,
    output wire       st_ready,
    input  wire [1:0] sel,       // 0=red_ticks 1=green_ticks 2=yellow_ticks 3=ctrl
    input  wire       enable,    // run FSM when high
    output reg        lamp_red,
    output reg        lamp_yellow,
    output reg        lamp_green,
    output reg  [1:0] state,     // 0=IDLE 1=RED 2=GREEN 3=YELLOW
    output wire [7:0] timer_out,
    output wire       st_out_valid
);
    localparam S_IDLE = 2'd0;
    localparam S_RED  = 2'd1;
    localparam S_GRN  = 2'd2;
    localparam S_YEL  = 2'd3;

    reg [7:0] red_ticks;
    reg [7:0] grn_ticks;
    reg [7:0] yel_ticks;
    reg [7:0] timer_r;
    reg       en_r;
    reg       out_v;

    assign st_ready     = 1'b1;
    assign timer_out    = timer_r;
    assign st_out_valid = out_v;

    wire run = en_r | enable;

    always @(posedge clk) begin
        if (!resetn) begin
            red_ticks   <= 8'h10;
            grn_ticks   <= 8'h10;
            yel_ticks   <= 8'h04;
            timer_r     <= 8'h00;
            en_r        <= 1'b0;
            state       <= S_IDLE;
            lamp_red    <= 1'b0;
            lamp_yellow <= 1'b0;
            lamp_green  <= 1'b0;
            out_v       <= 1'b0;
        end else begin
            out_v <= 1'b0;
            if (st_valid) begin
                case (sel)
                    2'd0: red_ticks <= (st_data == 8'h0) ? 8'h01 : st_data;
                    2'd1: grn_ticks <= (st_data == 8'h0) ? 8'h01 : st_data;
                    2'd2: yel_ticks <= (st_data == 8'h0) ? 8'h01 : st_data;
                    default: begin
                        en_r <= st_data[0];
                        if (st_data[1]) begin
                            state   <= S_RED;
                            timer_r <= red_ticks;
                        end
                    end
                endcase
                out_v <= 1'b1;
            end else if (run) begin
                case (state)
                    S_IDLE: begin
                        state       <= S_RED;
                        timer_r     <= red_ticks;
                        lamp_red    <= 1'b1;
                        lamp_yellow <= 1'b0;
                        lamp_green  <= 1'b0;
                    end
                    S_RED: begin
                        lamp_red    <= 1'b1;
                        lamp_yellow <= 1'b0;
                        lamp_green  <= 1'b0;
                        if (timer_r == 8'h0) begin
                            state   <= S_GRN;
                            timer_r <= grn_ticks;
                        end else
                            timer_r <= timer_r - 8'h1;
                    end
                    S_GRN: begin
                        lamp_red    <= 1'b0;
                        lamp_yellow <= 1'b0;
                        lamp_green  <= 1'b1;
                        if (timer_r == 8'h0) begin
                            state   <= S_YEL;
                            timer_r <= yel_ticks;
                        end else
                            timer_r <= timer_r - 8'h1;
                    end
                    default: begin // S_YEL
                        lamp_red    <= 1'b0;
                        lamp_yellow <= 1'b1;
                        lamp_green  <= 1'b0;
                        if (timer_r == 8'h0) begin
                            state   <= S_RED;
                            timer_r <= red_ticks;
                        end else
                            timer_r <= timer_r - 8'h1;
                    end
                endcase
                out_v <= 1'b1;
            end else begin
                lamp_red    <= 1'b0;
                lamp_yellow <= 1'b0;
                lamp_green  <= 1'b0;
            end
        end
    end
endmodule

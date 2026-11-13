// PicoRV32 running a program on the ZedBoard.
//
// The core executes five instructions from a ROM: it counts in register x1
// and stores the count to a 32-bit output port on every pass of the loop.
// The eight LEDs show an eight-bit window of that port; the slide switches
// SW4..SW0 give the window's lowest bit. With the switches at 20 the LEDs
// count at a few hertz; lower settings count faster.
// BTNC (rst) restarts the program.
//
// The processor runs at 50 MHz: a flop halves the board's 100 MHz clock, and
// the halved clock reaches the logic through a global clock buffer. Every
// register except that flop is on the halved clock, so no signal crosses
// between the two.
//
//     00:  addi x1, x0, 0       x1 = 0
//     04:  lui  x2, 0x10000     x2 = 0x1000_0000, the output port
//     08:  addi x1, x1, 1       loop: x1 += 1
//     0c:  sw   x1, 0(x2)             gpio = x1
//     10:  jal  x0, 08                goto loop
//
// Read with third_party/picorv32/picorv32.v.
module zedboard_pico_soc (
    input  logic       clk,
    input  logic       rst,
    input  logic [4:0] sw,
    output logic [7:0] led
);
    logic        resetn;
    logic        mem_valid, mem_ready;
    logic [31:0] mem_addr, mem_wdata, mem_rdata;
    logic [3:0]  mem_wstrb;

    // 50 MHz processor clock.
    logic clk_half = 1'b0;
    always_ff @(posedge clk) clk_half <= ~clk_half;

    // Two flops bring the button into the clock domain.
    logic [1:0] rst_sync;
    always_ff @(posedge clk_half) rst_sync <= {rst_sync[0], rst};
    assign resetn = ~rst_sync[1];

    // Program ROM, addressed by word.
    logic [31:0] rom;
    always_comb begin
        case (mem_addr[4:2])
            3'd0:    rom = 32'h0000_0093;
            3'd1:    rom = 32'h1000_0137;
            3'd2:    rom = 32'h0010_8093;
            3'd3:    rom = 32'h0011_2023;
            3'd4:    rom = 32'hff9f_f06f;
            default: rom = 32'h0000_0013;  // nop
        endcase
    end

    // One-cycle memory: every access is answered on the next clock edge.
    logic [31:0] gpio;
    always_ff @(posedge clk_half) begin
        if (!resetn) begin
            mem_ready <= 1'b0;
            mem_rdata <= 32'h0;
            gpio      <= 32'h0;
        end else begin
            mem_ready <= mem_valid & ~mem_ready;
            mem_rdata <= mem_addr[28] ? gpio : rom;
            if (mem_valid & ~mem_ready & mem_addr[28]) begin
                if (mem_wstrb[0]) gpio[7:0]   <= mem_wdata[7:0];
                if (mem_wstrb[1]) gpio[15:8]  <= mem_wdata[15:8];
                if (mem_wstrb[2]) gpio[23:16] <= mem_wdata[23:16];
                if (mem_wstrb[3]) gpio[31:24] <= mem_wdata[31:24];
            end
        end
    end

    logic [38:0] window;
    assign window = {7'b0, gpio} >> sw;
    assign led = window[7:0];

    picorv32 #(
        .ENABLE_COUNTERS      (0),
        .ENABLE_COUNTERS64    (0),
        .ENABLE_REGS_16_31    (1),
        .ENABLE_REGS_DUALPORT (1),
        .TWO_STAGE_SHIFT      (1),
        .BARREL_SHIFTER       (0),
        .TWO_CYCLE_COMPARE    (0),
        .TWO_CYCLE_ALU        (0),
        .COMPRESSED_ISA       (0),
        .CATCH_MISALIGN       (1),
        .CATCH_ILLINSN        (1),
        .ENABLE_PCPI          (0),
        .ENABLE_MUL           (0),
        .ENABLE_FAST_MUL      (0),
        .ENABLE_DIV           (0),
        .ENABLE_IRQ           (0),
        .ENABLE_TRACE         (0)
    ) u_cpu (
        .clk          (clk_half),
        .resetn       (resetn),
        .trap         (),
        .mem_valid    (mem_valid),
        .mem_instr    (),
        .mem_ready    (mem_ready),
        .mem_addr     (mem_addr),
        .mem_wdata    (mem_wdata),
        .mem_wstrb    (mem_wstrb),
        .mem_rdata    (mem_rdata),
        .mem_la_read  (),
        .mem_la_write (),
        .mem_la_addr  (),
        .mem_la_wdata (),
        .mem_la_wstrb (),
        .pcpi_valid   (),
        .pcpi_insn    (),
        .pcpi_rs1     (),
        .pcpi_rs2     (),
        .pcpi_wr      (1'b0),
        .pcpi_rd      (32'h0),
        .pcpi_wait    (1'b0),
        .pcpi_ready   (1'b0),
        .irq          (32'h0),
        .eoi          (),
        .trace_valid  (),
        .trace_data   ()
    );
endmodule

// FM-HEL-CONT: pin-reduced wrap around public PicoRV32 (RV32I mid core).
// Board pins are clk + led only. LED is FF-driven (honest IOB).
// Mem / PCPI / IRQ / trace ports tied for fabric bring-up.
// Instantiates picorv32 — not a counter clone. Soft-hold.
// Part: HL10T-C32-1.

module pico_pin_wrap (
    input  logic clk,
    output logic led
);
    // Heartbeat + board LED; mark_debug so Mark/Probe/ILA can capture a pico net.
    (* mark_debug = "true" *) logic [7:0] hb;
    always_ff @(posedge clk) begin
        hb  <= hb + 8'd1;
        led <= hb[7];
    end

    logic        resetn;
    logic        trap;
    logic        mem_valid;
    logic        mem_instr;
    logic        mem_ready;
    logic [31:0] mem_addr;
    logic [31:0] mem_wdata;
    logic [ 3:0] mem_wstrb;
    logic [31:0] mem_rdata;
    logic        mem_la_read;
    logic        mem_la_write;
    logic [31:0] mem_la_addr;
    logic [31:0] mem_la_wdata;
    logic [ 3:0] mem_la_wstrb;
    logic        pcpi_valid;
    logic [31:0] pcpi_insn;
    logic [31:0] pcpi_rs1;
    logic [31:0] pcpi_rs2;
    logic        pcpi_wr;
    logic [31:0] pcpi_rd;
    logic        pcpi_wait;
    logic        pcpi_ready;
    logic [31:0] irq;
    logic [31:0] eoi;
    logic        trace_valid;
    logic [35:0] trace_data;

    // No async reset on the wrap (IMUX-safe heartbeat). Core out of reset.
    assign resetn = 1'b1;
    // Idle mem: NOP ADDI x0,x0,0; never ready (honest tied fabric).
    assign mem_ready = 1'b0;
    assign mem_rdata = 32'h0000_0013;
    // PCPI / IRQ unused (parameters disable mul/div/irq/pcpi).
    assign pcpi_wr = 1'b0;
    assign pcpi_rd = 32'h0;
    assign pcpi_wait = 1'b0;
    assign pcpi_ready = 1'b0;
    assign irq = 32'h0;

    picorv32 #(
        .ENABLE_COUNTERS (0),
        .ENABLE_COUNTERS64 (0),
        .ENABLE_REGS_16_31 (1),
        .ENABLE_REGS_DUALPORT (1),
        .LATCHED_MEM_RDATA (0),
        .TWO_STAGE_SHIFT (1),
        .BARREL_SHIFTER (0),
        .TWO_CYCLE_COMPARE (0),
        .TWO_CYCLE_ALU (0),
        .COMPRESSED_ISA (0),
        .CATCH_MISALIGN (0),
        .CATCH_ILLINSN (0),
        .ENABLE_PCPI (0),
        .ENABLE_MUL (0),
        .ENABLE_FAST_MUL (0),
        .ENABLE_DIV (0),
        .ENABLE_IRQ (0),
        .ENABLE_IRQ_QREGS (0),
        .ENABLE_IRQ_TIMER (0),
        .ENABLE_TRACE (0),
        .REGS_INIT_ZERO (0)
    ) u_pico (
        .clk (clk),
        .resetn (resetn),
        .trap (trap),
        .mem_valid (mem_valid),
        .mem_instr (mem_instr),
        .mem_ready (mem_ready),
        .mem_addr (mem_addr),
        .mem_wdata (mem_wdata),
        .mem_wstrb (mem_wstrb),
        .mem_rdata (mem_rdata),
        .mem_la_read (mem_la_read),
        .mem_la_write (mem_la_write),
        .mem_la_addr (mem_la_addr),
        .mem_la_wdata (mem_la_wdata),
        .mem_la_wstrb (mem_la_wstrb),
        .pcpi_valid (pcpi_valid),
        .pcpi_insn (pcpi_insn),
        .pcpi_rs1 (pcpi_rs1),
        .pcpi_rs2 (pcpi_rs2),
        .pcpi_wr (pcpi_wr),
        .pcpi_rd (pcpi_rd),
        .pcpi_wait (pcpi_wait),
        .pcpi_ready (pcpi_ready),
        .irq (irq),
        .eoi (eoi),
        .trace_valid (trace_valid),
        .trace_data (trace_data)
    );
endmodule

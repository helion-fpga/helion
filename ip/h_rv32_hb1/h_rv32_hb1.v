// Helion HB1: PicoRV32 wrap on Helion-MM. Not Zynq PS. Not an original core.
// PicoRV32 is ISC (Claire Wolf / YosysHQ). See third_party/picorv32/LICENSE.
// Configurable through the IP customization dialog (Vivado CONFIG.*): each
// parameter passes straight through to PicoRV32; defaults keep the original
// HB1 configuration.
module h_rv32_hb1 #(
    parameter [0:0]  ENABLE_COUNTERS = 0,
    parameter [0:0]  BARREL_SHIFTER  = 0,
    parameter [0:0]  COMPRESSED_ISA  = 0,
    parameter [0:0]  CATCH_MISALIGN  = 0,
    parameter [0:0]  CATCH_ILLINSN   = 0,
    parameter [0:0]  ENABLE_MUL      = 0,
    parameter [0:0]  ENABLE_DIV      = 0,
    parameter [31:0] PROGADDR_RESET  = 32'h00000000,
    parameter [31:0] STACKADDR       = 32'hffffffff
) (
    input  wire        clk,
    input  wire        resetn,
    output wire        mm_valid,
    output wire        mm_write,
    output wire [31:0] mm_addr,
    output wire [31:0] mm_wdata,
    output wire [3:0]  mm_wstrb,
    input  wire        mm_ready,
    input  wire [31:0] mm_rdata
);
    // Official PicoRV32 native mem interface mapped to Helion-MM.
    picorv32 #(
        .ENABLE_COUNTERS(ENABLE_COUNTERS),
        .BARREL_SHIFTER(BARREL_SHIFTER),
        .COMPRESSED_ISA(COMPRESSED_ISA),
        .CATCH_MISALIGN(CATCH_MISALIGN),
        .CATCH_ILLINSN(CATCH_ILLINSN),
        .ENABLE_MUL(ENABLE_MUL),
        .ENABLE_DIV(ENABLE_DIV),
        .PROGADDR_RESET(PROGADDR_RESET),
        .STACKADDR(STACKADDR)
    ) u_pico (
        .clk(clk),
        .resetn(resetn),
        .mem_valid(mm_valid),
        .mem_instr(),
        .mem_ready(mm_ready),
        .mem_addr(mm_addr),
        .mem_wdata(mm_wdata),
        .mem_wstrb(mm_wstrb),
        .mem_rdata(mm_rdata)
    );
    assign mm_write = |mm_wstrb;
endmodule

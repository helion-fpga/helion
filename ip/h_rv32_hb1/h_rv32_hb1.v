// Helion HB1: PicoRV32 wrap on Helion-MM. Not Zynq PS. Not an original core.
// PicoRV32 is ISC (Claire Wolf / YosysHQ). See third_party/picorv32/LICENSE.
module h_rv32_hb1 (
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
        .ENABLE_COUNTERS(0),
        .COMPRESSED_ISA(0),
        .CATCH_MISALIGN(0),
        .CATCH_ILLINSN(0)
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

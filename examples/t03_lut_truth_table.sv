// t03_lut_truth_table: combinational LUT logic, no clock, no inputs.
// Isolates: LUT6 INIT programming. Each LED is a fixed function of constants,
// so a wrong INIT shows as the wrong LED pattern and nothing else can explain
// it. Constants fold, so the interesting part is that each output is driven by
// its OWN LUT rather than one shared net.
// Pass: LD0,LD2,LD4,LD6 on; LD1,LD3,LD5,LD7 off.
module t03_lut_truth_table (
    output logic [7:0] led
);
    logic [1:0] a;
    assign a = 2'b10;
    assign led[0] = a[1] & ~a[0];      // 1
    assign led[1] = a[0] & ~a[1];      // 0
    assign led[2] = a[1] | a[0];       // 1
    assign led[3] = ~(a[1] | a[0]);    // 0
    assign led[4] = a[1] ^ a[0];       // 1
    assign led[5] = ~(a[1] ^ a[0]);    // 0
    assign led[6] = a[1];              // 1
    assign led[7] = a[0];              // 0
endmodule

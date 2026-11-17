"""Generate the CMT sweep designs Vivado builds for tests/cmt_sweep.py.

Usage: python3 tests/cmt_sweep_gen.py <dir>   (writes <dir>/rtl/*.sv, *.xdc)
Then on the USF cluster, with ref_build.tcl (~/helion-zed-builds/vivado_jobs/)
next to tests/cmt_sweep_jobs.sh: bash cmt_sweep_jobs.sh sweep_<name> ...

* sweep_<bandwidth>_<m>: four MMCME2_ADV and four PLLE2_ADV, feedback
  multipliers m..m+3, D = 1 (lock/filter tables).
* sweep_divs: two MMCMs x 7 and two PLLs x 6 outputs with assorted divides,
  duties and phases (counter encoding).
* sweep_pwr, sweep_pwr2..4: MMCMs differing only in which outputs
  are used and their divides/phases (POWER_REG).
"""
import sys, os
out = sys.argv[1]
CLK = ["W12","W11","V10","W8","V12","W10","V9","V8"]          # Pmod JB (bank 13)
LED = ["T22","T21","U22","U21","V22","W22","U19","U14"]
xdc = []
for i,p in enumerate(CLK): xdc.append(f"set_property -dict {{PACKAGE_PIN {p} IOSTANDARD LVCMOS33}} [get_ports {{ci[{i}]}}]")
for i,p in enumerate(LED): xdc.append(f"set_property -dict {{PACKAGE_PIN {p} IOSTANDARD LVCMOS33}} [get_ports {{led[{i}]}}]")
xdc.append("set_property CLOCK_DEDICATED_ROUTE FALSE [get_nets -hier -filter {NAME =~ *ci_IBUF*}]")
names = []
def table_design(name, bw, ms):
    # 4 MMCMs and 4 PLLs, multiplier ms[i] each, D=1.
    v = [f"module {name}(input logic [7:0] ci, output logic [7:0] led);"]
    clocks = []
    for i in range(8):
        m = ms[i % 4]
        if m is None:
            v.append(f"  assign led[{i}] = 1'b0;"); continue
        if i < 4:
            f = max(700.0/m, 11.0); prim = "MMCME2_ADV"; mult = f".CLKFBOUT_MULT_F({m}.0)"
        else:
            f = max(840.0/m, 19.5); prim = "PLLE2_ADV"; mult = f".CLKFBOUT_MULT({m})"
        per = round(1000.0/f, 3)
        clocks.append(f"create_clock -period {per:.3f} [get_ports {{ci[{i}]}}]")
        v.append(f"  logic b{i}, fb{i};")
        v.append(f"  BUFG ub{i}(.I(ci[{i}]), .O(b{i}));")
        v.append(f"  {prim} #(.BANDWIDTH(\"{bw}\"), .CLKIN1_PERIOD({per:.3f}), .DIVCLK_DIVIDE(1), {mult}, .CLKOUT0_DIVIDE{'_F' if i<4 else ''}({'2.0' if i<4 else '2'}))")
        v.append(f"    u{i}(.CLKIN1(b{i}), .CLKIN2(1'b0), .CLKINSEL(1'b1), .CLKFBIN(fb{i}), .CLKFBOUT(fb{i}), .LOCKED(led[{i}]), .RST(1'b0), .PWRDWN(1'b0),")
        v.append(f"     .DADDR(7'd0), .DCLK(1'b0), .DEN(1'b0), .DI(16'd0), .DWE(1'b0){', .PSCLK(1\'b0), .PSEN(1\'b0), .PSINCDEC(1\'b0)' if i<4 else ''});")
    v.append("endmodule")
    open(f"{out}/rtl/{name}.sv","w").write("\n".join(v)+"\n")
    open(f"{out}/rtl/{name}.xdc","w").write("\n".join(xdc+clocks)+"\n")
    names.append(name)
os.makedirs(f"{out}/rtl", exist_ok=True)
ms = list(range(2,65))
for bw in ["OPTIMIZED","HIGH","LOW"]:
    for g in range(0, len(ms), 4):
        grp = ms[g:g+4] + [None]*(4-len(ms[g:g+4]))
        table_design(f"sweep_{bw.lower()}_{ms[g]}", bw, grp)

# Divider/phase/duty design: 2 MMCMs x 7 outputs, 2 PLLs x 6 outputs, all on BUFGs.
cmts = [
  ("MMCME2_ADV", 10, 1, [(1,0.5,0.0),(2,0.5,22.5),(3,0.5,345.0),(4,0.25,90.0),(5,0.5,0.0),(7,0.5,180.0),(128,0.5,0.0)]),
  ("MMCME2_ADV", 30, 3, [(6,0.5,0.0),(9,0.5,0.0),(33,0.5,0.0),(64,0.5,0.0),(100,0.5,0.0),(8,0.75,0.0),(13,0.5,90.0)]),
  ("PLLE2_ADV", 10, 1, [(1,0.5,0.0),(2,0.5,0.0),(3,0.5,0.0),(4,0.5,45.0),(5,0.5,0.0),(6,0.5,0.0)]),
  ("PLLE2_ADV", 27, 3, [(7,0.5,0.0),(10,0.5,0.0),(16,0.25,0.0),(31,0.5,0.0),(64,0.5,0.0),(128,0.5,0.0)]),
]
v = ["module sweep_divs(input logic clk, output logic [7:0] led);", "  logic cb; BUFG ubin(.I(clk), .O(cb));"]
tog = []
for c,(prim,m,d,outs) in enumerate(cmts):
    mmcm = prim.startswith("MMCM")
    ps = [f".BANDWIDTH(\"OPTIMIZED\")", ".CLKIN1_PERIOD(10.0)", f".DIVCLK_DIVIDE({d})",
          f".CLKFBOUT_MULT{'_F' if mmcm else ''}({m}{'.0' if mmcm else ''})"]
    pins = []
    for k,(dv,du,ph) in enumerate(outs):
        ps.append(f".CLKOUT{k}_DIVIDE{'_F' if (mmcm and k==0) else ''}({dv}{'.0' if (mmcm and k==0) else ''})")
        ps.append(f".CLKOUT{k}_DUTY_CYCLE({du})"); ps.append(f".CLKOUT{k}_PHASE({ph})")
        v.append(f"  logic o{c}_{k}, g{c}_{k}; logic t{c}_{k} = 1'b0;")
        v.append(f"  BUFG ug{c}_{k}(.I(o{c}_{k}), .O(g{c}_{k}));")
        v.append(f"  always_ff @(posedge g{c}_{k}) t{c}_{k} <= ~t{c}_{k};")
        pins.append(f".CLKOUT{k}(o{c}_{k})"); tog.append(f"t{c}_{k}")
    v.append(f"  logic fb{c};")
    v.append(f"  {prim} #({', '.join(ps)})")
    extra = ", .PSCLK(1'b0), .PSEN(1'b0), .PSINCDEC(1'b0)" if mmcm else ""
    v.append(f"    uc{c}(.CLKIN1(cb), .CLKIN2(1'b0), .CLKINSEL(1'b1), .CLKFBIN(fb{c}), .CLKFBOUT(fb{c}), .LOCKED(led[{c}]), .RST(1'b0), .PWRDWN(1'b0),")
    v.append(f"     .DADDR(7'd0), .DCLK(1'b0), .DEN(1'b0), .DI(16'd0), .DWE(1'b0){extra}, {', '.join(pins)});")
v.append(f"  assign led[4] = {' ^ '.join(tog)};")
v.append("  assign led[7:5] = 3'b0;")
v.append("endmodule")
open(f"{out}/rtl/sweep_divs.sv","w").write("\n".join(v)+"\n")
dx = [f"set_property -dict {{PACKAGE_PIN Y9 IOSTANDARD LVCMOS33}} [get_ports clk]", "create_clock -period 10.000 [get_ports clk]"]
dx += [l for l in xdc if "led" in l]
open(f"{out}/rtl/sweep_divs.xdc","w").write("\n".join(dx)+"\n")
names.append("sweep_divs"); names.append("sweep_pwr")
print(" ".join(names + ["sweep_pwr2", "sweep_pwr3", "sweep_pwr4"]))

# POWER_REG probe: same M=10/D=1 CMTs, differing only in output use and phase.
cfgs = [("none", None), ("p0", (8, 0.0)), ("pmux", (8, 22.5)), ("pdelay", (8, 45.0))]
v = ["module sweep_pwr(input logic clk, output logic [7:0] led);", "  logic cb; BUFG ubin(.I(clk), .O(cb));"]
tog = []
for c in range(8):
    mmcm = c < 4
    tag, oc = cfgs[c % 4]
    prim = "MMCME2_ADV" if mmcm else "PLLE2_ADV"
    ps = [".BANDWIDTH(\"OPTIMIZED\")", ".CLKIN1_PERIOD(10.0)", ".DIVCLK_DIVIDE(1)", f".CLKFBOUT_MULT{'_F' if mmcm else ''}(10{'.0' if mmcm else ''})"]
    pins = ""
    if oc:
        dv, ph = oc
        ps.append(f".CLKOUT0_DIVIDE{'_F' if mmcm else ''}({dv}{'.0' if mmcm else ''})")
        ps.append(f".CLKOUT0_PHASE({ph})")
        v.append(f"  logic o{c}, g{c}; logic t{c} = 1'b0; BUFG ug{c}(.I(o{c}), .O(g{c})); always_ff @(posedge g{c}) t{c} <= ~t{c};")
        pins = f", .CLKOUT0(o{c})"; tog.append(f"t{c}")
    extra = ", .PSCLK(1'b0), .PSEN(1'b0), .PSINCDEC(1'b0)" if mmcm else ""
    v.append(f"  logic fb{c}, lk{c};")
    v.append(f"  {prim} #({', '.join(ps)}) uc{c}(.CLKIN1(cb), .CLKIN2(1'b0), .CLKINSEL(1'b1), .CLKFBIN(fb{c}), .CLKFBOUT(fb{c}), .LOCKED(lk{c}), .RST(1'b0), .PWRDWN(1'b0),")
    v.append(f"     .DADDR(7'd0), .DCLK(1'b0), .DEN(1'b0), .DI(16'd0), .DWE(1'b0){extra}{pins});")
v.append(f"  assign led[0] = {' ^ '.join(tog)};")
v.append(f"  assign led[7:1] = {{{', '.join(f'lk{c}' for c in range(1,8))}}};")
v.append("endmodule")
open(f"{out}/rtl/sweep_pwr.sv","w").write("\n".join(v)+"\n")
open(f"{out}/rtl/sweep_pwr.xdc","w").write("\n".join(dx)+"\n")

# ---- POWER_REG probe 2 ----
sets = [[1], [0, 1], [2], [6]]
v = ["module sweep_pwr2(input logic clk, output logic [7:0] led);", "  logic cb; BUFG ubin(.I(clk), .O(cb));"]
tog = []; lks = []
for c in range(4):
    ps = [".BANDWIDTH(\"OPTIMIZED\")", ".CLKIN1_PERIOD(10.0)", ".DIVCLK_DIVIDE(1)", ".CLKFBOUT_MULT_F(10.0)"]
    pins = []
    for k in sets[c]:
        ps.append(f".CLKOUT{k}_DIVIDE{'_F' if k == 0 else ''}({'20.0' if k == 0 else 20})")
        v.append(f"  logic o{c}_{k}, g{c}_{k}; logic t{c}_{k} = 1'b0; BUFG ug{c}_{k}(.I(o{c}_{k}), .O(g{c}_{k})); always_ff @(posedge g{c}_{k}) t{c}_{k} <= ~t{c}_{k};")
        pins.append(f".CLKOUT{k}(o{c}_{k})"); tog.append(f"t{c}_{k}")
    v.append(f"  logic fb{c}, lk{c};"); lks.append(f"lk{c}")
    v.append(f"  MMCME2_ADV #({', '.join(ps)}) uc{c}(.CLKIN1(cb), .CLKIN2(1'b0), .CLKINSEL(1'b1), .CLKFBIN(fb{c}), .CLKFBOUT(fb{c}), .LOCKED(lk{c}), .RST(1'b0), .PWRDWN(1'b0),")
    v.append(f"     .DADDR(7'd0), .DCLK(1'b0), .DEN(1'b0), .DI(16'd0), .DWE(1'b0), .PSCLK(1'b0), .PSEN(1'b0), .PSINCDEC(1'b0), {', '.join(pins)});")
v.append(f"  assign led[0] = {' ^ '.join(tog)};")
v.append(f"  assign led[4:1] = {{{', '.join(lks)}}};")
v.append("  assign led[7:5] = 3'b0;")
v.append("endmodule")
open(f"{out}/rtl/sweep_pwr2.sv", "w").write("\n".join(v) + "\n")
open(f"{out}/rtl/sweep_pwr2.xdc", "w").write(open(f"{out}/rtl/sweep_pwr.xdc").read())

# ---- POWER_REG probe 3 ----
sets = [[(0,20,0.0),(1,8,0.0)], [(0,20,0.0),(1,20,0.0),(2,20,0.0)], [(1,8,0.0),(2,40,0.0)], [(0,20,0.0),(1,20,90.0)]]
v = ["module sweep_pwr3(input logic clk, output logic [7:0] led);", "  logic cb; BUFG ubin(.I(clk), .O(cb));"]
tog = []; lks = []
for c in range(4):
    ps = [".BANDWIDTH(\"OPTIMIZED\")", ".CLKIN1_PERIOD(10.0)", ".DIVCLK_DIVIDE(1)", ".CLKFBOUT_MULT_F(10.0)"]
    pins = []
    for k, dv, ph in sets[c]:
        ps.append(f".CLKOUT{k}_DIVIDE{'_F' if k == 0 else ''}({str(dv)+'.0' if k == 0 else dv})"); ps.append(f".CLKOUT{k}_PHASE({ph})")
        v.append(f"  logic o{c}_{k}, g{c}_{k}; logic t{c}_{k} = 1'b0; BUFG ug{c}_{k}(.I(o{c}_{k}), .O(g{c}_{k})); always_ff @(posedge g{c}_{k}) t{c}_{k} <= ~t{c}_{k};")
        pins.append(f".CLKOUT{k}(o{c}_{k})"); tog.append(f"t{c}_{k}")
    v.append(f"  logic fb{c}, lk{c};"); lks.append(f"lk{c}")
    v.append(f"  MMCME2_ADV #({', '.join(ps)}) uc{c}(.CLKIN1(cb), .CLKIN2(1'b0), .CLKINSEL(1'b1), .CLKFBIN(fb{c}), .CLKFBOUT(fb{c}), .LOCKED(lk{c}), .RST(1'b0), .PWRDWN(1'b0),")
    v.append(f"     .DADDR(7'd0), .DCLK(1'b0), .DEN(1'b0), .DI(16'd0), .DWE(1'b0), .PSCLK(1'b0), .PSEN(1'b0), .PSINCDEC(1'b0), {', '.join(pins)});")
v.append(f"  assign led[0] = {' ^ '.join(tog)};")
v.append(f"  assign led[4:1] = {{{', '.join(lks)}}};")
v.append("  assign led[7:5] = 3'b0;")
v.append("endmodule")
open(f"{out}/rtl/sweep_pwr3.sv", "w").write("\n".join(v) + "\n")
open(f"{out}/rtl/sweep_pwr3.xdc", "w").write(open(f"{out}/rtl/sweep_pwr.xdc").read())

# ---- POWER_REG probe 4: phase interpolator use per output ----
sets = [[(1, 8, 22.5)], [(0, 8, 11.25)], [(0, 8, 33.75)], [(1, 20, 2.25)]]
v = ["module sweep_pwr4(input logic clk, output logic [7:0] led);", "  logic cb; BUFG ubin(.I(clk), .O(cb));"]
tog = []; lks = []
for c in range(4):
    ps = [".BANDWIDTH(\"OPTIMIZED\")", ".CLKIN1_PERIOD(10.0)", ".DIVCLK_DIVIDE(1)", ".CLKFBOUT_MULT_F(10.0)"]
    pins = []
    for k, dv, ph in sets[c]:
        ps.append(f".CLKOUT{k}_DIVIDE{'_F' if k == 0 else ''}({str(dv)+'.0' if k == 0 else dv})"); ps.append(f".CLKOUT{k}_PHASE({ph})")
        v.append(f"  logic o{c}_{k}, g{c}_{k}; logic t{c}_{k} = 1'b0; BUFG ug{c}_{k}(.I(o{c}_{k}), .O(g{c}_{k})); always_ff @(posedge g{c}_{k}) t{c}_{k} <= ~t{c}_{k};")
        pins.append(f".CLKOUT{k}(o{c}_{k})"); tog.append(f"t{c}_{k}")
    v.append(f"  logic fb{c}, lk{c};"); lks.append(f"lk{c}")
    v.append(f"  MMCME2_ADV #({', '.join(ps)}) uc{c}(.CLKIN1(cb), .CLKIN2(1'b0), .CLKINSEL(1'b1), .CLKFBIN(fb{c}), .CLKFBOUT(fb{c}), .LOCKED(lk{c}), .RST(1'b0), .PWRDWN(1'b0),")
    v.append(f"     .DADDR(7'd0), .DCLK(1'b0), .DEN(1'b0), .DI(16'd0), .DWE(1'b0), .PSCLK(1'b0), .PSEN(1'b0), .PSINCDEC(1'b0), {', '.join(pins)});")
v.append(f"  assign led[0] = {' ^ '.join(tog)};")
v.append(f"  assign led[4:1] = {{{', '.join(lks)}}};")
v.append("  assign led[7:5] = 3'b0;")
v.append("endmodule")
open(f"{out}/rtl/sweep_pwr4.sv", "w").write("\n".join(v) + "\n")
open(f"{out}/rtl/sweep_pwr4.xdc", "w").write(open(f"{out}/rtl/sweep_pwr.xdc").read())


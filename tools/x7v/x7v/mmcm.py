"""MMCM and PLL: decode MMCME2_ADV / PLLE2_ADV configuration and model the clocks.

What the model rests on (no Helion code):

* Project X-Ray's CMT_TOP_{L,R}_LOWER_B (MMCME2_ADV) and CMT_TOP_{L,R}_UPPER_T
  (PLLE2_ADV) databases. Each output counter (DIVCLK, CLKFBOUT, CLKOUT0..6,
  PLL: CLKOUT0..5) is two DRP-style registers: `<C>_CLKOUT1_{HIGH_TIME,
  LOW_TIME, PHASE_MUX, OUTPUT_ENABLE}` and `<C>_CLKOUT2_{DELAY_TIME, EDGE,
  NO_COUNT, FRAC, FRAC_EN, FRAC_WF_R, MX}`. The MMCM's CLKOUT5/CLKOUT6 second
  registers are named `..._CLKOUT2_FRACTIONAL_*` (they also hold CLKOUT0's and
  CLKFBOUT's fractional-divide extras). DIVCLK has only HIGH/LOW/EDGE/NO_COUNT.
* UG472 (7 Series Clocking Resources) for behaviour: f_VCO = f_CLKIN1 * M / D,
  f_CLKOUTn = f_VCO / O_n, phase in steps of 1/8 of a VCO period, LOCKED once
  the loop has settled; the feedback loop must be closed (CLKFBOUT to CLKFBIN).
* The counter register meaning (Xilinx XAPP888, MMCM and PLL Dynamic
  Reconfiguration): NO_COUNT set bypasses the counter (divide 1); otherwise
  divide = HIGH_TIME + LOW_TIME (a 6-bit 0 means 64), the output is high for
  HIGH_TIME VCO periods plus half a period when EDGE is set, and the first
  edge is DELAY_TIME + PHASE_MUX/8 VCO periods late.
* Vivado 2017.3 bitstreams of generated designs (`tests/cmt_sweep.py`) for the
  lock and loop-filter words (`LKTABLE`, `TABLE`, `FILTREG*`, `LOCKREG*`,
  `POWER_REG`): their values are lookup tables in Vivado, not functions of
  anything UG472 states, so x7v holds Vivado's measured values
  (`cmt_tables.json`, per primitive, BANDWIDTH and feedback multiplier;
  `expected_power` for POWER_REG) and reports any other value as an error.

The netlist model (`x7v_cmt`) measures the CLKIN1 period, asserts LOCKED
after LOCK_CYCLES input clocks, and from that CLKIN1 edge drives each enabled
output with period O * T_VCO, high time from HIGH/LOW/EDGE and the phase
delay above, all `INSERTION` ns late (a fixed stand-in for buffer delay, so
generated edges never coincide with the co-simulation's sample instants).
The RTL side gets the same model through primitive shims (`RTL_PRIMS`) that
turn UG953 attributes into those numbers, so co-simulation passes only when
the bitstream's counters mean what the RTL's attributes ask for and the
clock routing reaches the same flops.

Reported as errors rather than guessed: fractional dividers, CLKFBOUT phase,
spread spectrum, dynamic phase shift and DRP use (PSEN/DEN driven), a
feedback loop that is not closed (CLKFBIN not driven by this CMT's CLKFBOUT,
directly or through one BUFG), a clock input selected through CLKINSEL other
than CLKIN1, MX not 00, reading an output whose counter is not enabled or
an unmodelled output (CLKOUTnB, CLKFBOUTB, DO, DRDY, PSDONE, *STOPPED), and
lock/filter words not in the measured table.
"""
from __future__ import annotations

import json
import os
from dataclasses import dataclass, field

KINDS = ("MMCME2_ADV", "PLLE2_ADV")
N_OUT = {"MMCME2_ADV": 7, "PLLE2_ADV": 6}
INSERTION_NS = 0.05
LOCK_CYCLES = 16
# Lock and loop-filter words: name -> width.
STATIC = {"LKTABLE": 40, "TABLE": 10, "FILTREG1_RESERVED": 12, "FILTREG2_RESERVED": 10,
          "LOCKREG1_RESERVED": 6, "LOCKREG2_RESERVED": 1, "LOCKREG3_RESERVED": 1}
TABLES_PATH = os.path.join(os.path.dirname(__file__), "cmt_tables.json")


def _vec(F: set, name: str, n: int, used: set) -> int:
    v = 0
    for i in range(n):
        k = f"{name}[{i}]"
        if k in F:
            v |= 1 << i
            used.add(k)
    return v


def _flag(F: set, name: str, used: set) -> bool:
    for k in (name, f"{name}[0]"):
        if k in F:
            used.add(k)
            return True
    return False


@dataclass
class Counter:
    name: str
    high: int = 1
    low: int = 1
    no_count: bool = False
    edge: bool = False
    phase_mux: int = 0
    delay: int = 0
    enabled: bool = False
    frac_en: bool = False
    frac: int = 0
    mx: int = 0

    @property
    def divide(self) -> int:
        """Integer divide (fractional dividers are refused)."""
        if self.no_count:
            return 1
        return (self.high or 64) + (self.low or 64)

    @property
    def high_half(self) -> int:
        """High time in half VCO periods."""
        if self.no_count:
            return 1
        return 2 * (self.high or 64) + int(self.edge)

    @property
    def phase8(self) -> int:
        """First-edge delay in eighths of a VCO period."""
        return 8 * self.delay + self.phase_mux


@dataclass
class Cmt:
    kind: str
    counters: dict                  # name -> Counter
    comp: str                       # MMCM: ZHOLD / Z_ZHOLD / none
    static: dict                    # STATIC name -> value
    power: int
    flags: set                      # IN_USE, ZINV_*, INV_CLKINSEL, STARTUP_WAIT, SS_EN
    unexplained: list = field(default_factory=list)
    problems: list = field(default_factory=list)

    @property
    def m(self) -> int:
        return self.counters["CLKFBOUT"].divide

    @property
    def d(self) -> int:
        return self.counters["DIVCLK"].divide

    def outputs(self):
        return [self.counters[f"CLKOUT{i}"] for i in range(N_OUT[self.kind])]


def decode_cmt(kind: str, F: set) -> Cmt:
    used: set = set()
    cs = {}
    names = ["DIVCLK", "CLKFBOUT"] + [f"CLKOUT{i}" for i in range(N_OUT[kind])]
    problems = []
    for n in names:
        c = Counter(n)
        if n == "DIVCLK":
            c.high = _vec(F, "DIVCLK_DIVCLK_HIGH_TIME", 6, used)
            c.low = _vec(F, "DIVCLK_DIVCLK_LOW_TIME", 6, used)
            c.edge = _flag(F, "DIVCLK_DIVCLK_EDGE", used)
            c.no_count = _flag(F, "DIVCLK_DIVCLK_NO_COUNT", used)
            if _vec(F, "DIVCLK_DIVCLK_RESERVED", 2, used):
                problems.append("DIVCLK_RESERVED set")
            cs[n] = c
            continue
        r1, r2 = f"{n}_CLKOUT1_", f"{n}_CLKOUT2_"
        if kind == "MMCME2_ADV" and n in ("CLKOUT5", "CLKOUT6"):
            r2 = f"{n}_CLKOUT2_FRACTIONAL_"
        c.high = _vec(F, r1 + "HIGH_TIME", 6, used)
        c.low = _vec(F, r1 + "LOW_TIME", 6, used)
        c.phase_mux = _vec(F, r1 + "PHASE_MUX", 3, used)
        c.enabled = _flag(F, r1 + "OUTPUT_ENABLE", used)
        c.delay = _vec(F, r2 + "DELAY_TIME", 6, used)
        c.edge = _flag(F, r2 + "EDGE", used)
        c.no_count = _flag(F, r2 + "NO_COUNT", used)
        c.mx = _vec(F, r2 + "MX", 2, used)
        if _flag(F, r2 + "RESERVED", used) or _vec(F, r2 + "RESERVED", 2, used):
            problems.append(f"{n}: RESERVED bit set")
        c.frac_en = _flag(F, r2 + "FRAC_EN", used)
        c.frac = _vec(F, r2 + "FRAC", 3, used)
        for extra in ("FRAC_WF_R", "FRAC_WF_F"):
            if _flag(F, r2 + extra, used) or _vec(F, r2 + extra, 3, used):
                problems.append(f"{n}: {extra} set (fractional divide is not modelled)")
        if c.frac_en or c.frac:
            problems.append(f"{n}: fractional divide is not modelled")
        if c.mx:
            problems.append(f"{n}: MX = {c.mx:02b} (must be 00)")
        cs[n] = c
    if kind == "MMCME2_ADV":
        # CLKOUT5's / CLKOUT6's PHASE_MUX_F is CLKOUT0's / CLKFBOUT's falling
        # edge phase: Vivado writes it equal to PHASE_MUX for an integer divide.
        for reg, owner in (("CLKOUT5", "CLKOUT0"), ("CLKOUT6", "CLKFBOUT")):
            f = _vec(F, f"{reg}_CLKOUT2_FRACTIONAL_PHASE_MUX_F", 3, used)
            if f != cs[owner].phase_mux:
                problems.append(f"{owner}: PHASE_MUX_F = {f} differs from PHASE_MUX = {cs[owner].phase_mux} (fractional divide is not modelled)")
    comp = "none"
    for k in ("COMP.ZHOLD", "COMP.Z_ZHOLD", "COMPENSATION.ZHOLD_NO_CLKIN_BUF", "COMPENSATION.Z_ZHOLD_OR_CLKIN_BUF"):
        if _flag(F, k, used):
            comp = k.split(".", 1)[1] if comp == "none" else comp + "+" + k.split(".", 1)[1]
    static = {k: _vec(F, k, w, used) for k, w in STATIC.items()}
    power = _vec(F, "POWER_REG_POWER_REG_POWER_REG", 16, used)
    flags = set()
    for k in ("IN_USE", "ZINV_PSEN", "ZINV_PSINCDEC", "ZINV_PWRDWN", "ZINV_RST", "INV_CLKINSEL",
              "STARTUP_WAIT", "SS_EN"):
        if _flag(F, k, used):
            flags.add(k)
    if "SS_EN" in flags:
        problems.append("spread spectrum (SS_EN) is not modelled")
    if cs["CLKFBOUT"].phase8:
        problems.append("CLKFBOUT phase shift is not modelled")
    return Cmt(kind, cs, comp, static, power, flags, sorted(F - used), problems)


def tile_cmts(dec) -> dict[str, Cmt]:
    """{tile: Cmt} for every CMT tile with configuration bits."""
    out = {}
    for t, td in dec.tiles.items():
        if not td.tile_type.startswith("CMT_TOP_"):
            continue
        for kind in KINDS:
            pre = kind + "."
            F = {f[len(pre):] for f in td.features if f.startswith(pre)}
            if F:
                out[t] = decode_cmt(kind, F)
    return out


def load_tables() -> dict:
    try:
        with open(TABLES_PATH) as f:
            return json.load(f)
    except FileNotFoundError:
        return {}


def table_match(c: Cmt, tables: dict | None = None) -> list[str]:
    """BANDWIDTH settings whose measured lock/filter words equal this CMT's."""
    tables = load_tables() if tables is None else tables
    hits = []
    for bw, by_m in sorted(tables.get(c.kind, {}).items()):
        want = by_m.get(str(c.m))
        if want and all(c.static[k] == want[k] for k in STATIC):
            hits.append(bw)
    return hits


def check(c: Cmt, tables: dict | None = None) -> list[str]:
    """Configuration problems of one CMT (decode level, no netlist)."""
    errs = list(c.problems)
    if c.unexplained:
        errs.append(f"unexplained features {c.unexplained[:8]}")
    if "IN_USE" not in c.flags:
        errs.append("not IN_USE")
    tables = load_tables() if tables is None else tables
    if not table_match(c, tables):
        have = " ".join(f"{k}={v:#x}" for k, v in c.static.items())
        errs.append(f"lock/filter words for M={c.m} match no Vivado-measured BANDWIDTH ({have})")
    if c.power != expected_power(c):
        errs.append(f"POWER_REG = {c.power:#06x}, Vivado writes {expected_power(c):#06x} for these phases")
    return errs


def expected_power(c: Cmt) -> int:
    """POWER_REG as Vivado writes it (measured, tests/cmt_sweep.py): an MMCM
    with some counter phase on an odd quarter VCO period (PHASE_MUX not 0 or
    4) 0x9900, any other MMCM 0x0100, a PLL 0."""
    if c.kind == "PLLE2_ADV":
        return 0
    return 0x9900 if any(o.phase_mux % 4 for o in c.counters.values()) else 0x0100


def summary(dec, clkin_mhz: float | None = None) -> list[str]:
    """One block per configured CMT (the `x7v cmt` report)."""
    lines = []
    tables = load_tables()
    for t, c in sorted(tile_cmts(dec).items()):
        bw = table_match(c, tables)
        vco = f", VCO {clkin_mhz * c.m / c.d:.3f} MHz" if clkin_mhz else ""
        lines.append(f"{t}.{c.kind}: M={c.m} D={c.d}{vco} COMP={c.comp} BANDWIDTH={'/'.join(bw) or '?'} "
                     f"POWER_REG={c.power:#06x} flags={','.join(sorted(c.flags))}")
        for o in c.outputs():
            if not o.enabled and o.no_count:
                continue
            f = f" {clkin_mhz * c.m / c.d / o.divide:.3f} MHz" if clkin_mhz else ""
            lines.append(f"  {o.name}: divide {o.divide}{f}, high {o.high_half / 2:g} of {o.divide} VCO periods, "
                         f"phase {o.phase8 * 45 / o.divide:g} deg ({o.phase8}/8 VCO){' ENABLED' if o.enabled else ''}")
        for e in check(c, tables):
            lines.append(f"  PROBLEM: {e}")
    return lines


# ---- netlist model --------------------------------------------------------------

def _packed(vals: list[int]) -> str:
    return "{" + ", ".join(f"16'd{v}" for v in reversed(vals)) + "}"


def model_cmt(x, s, nl):
    from .netlist import sig  # netlist imports this module
    kind = s.stype
    c = decode_cmt(kind, s.features)
    err = lambda m: x.errors.append(f"{s.name} ({s.tile}.{s.fprefix}): {m}")
    for e in check(c):
        err(e)
    if c.comp not in ("Z_ZHOLD", "none"):
        err(f"compensation {c.comp} not modelled (only internal feedback, COMP.Z_ZHOLD, is validated)")
    n = N_OUT[kind]

    def pin(p: str, what: str, inv: bool = False, need: bool = True) -> str:
        d = x.pin_driver(s, p)
        if d[0] not in ("site", "const") and need:
            err(f"{p} ({what}) is undriven")
        e = x.expr(d)
        return f"(~{e})" if inv else e

    def const(p: str, inv: bool = False):
        d = x.pin_driver(s, p)
        return None if d[0] != "const" else d[1] ^ int(inv)

    # Only CLKIN1 is modelled: CLKINSEL must select it (1, after inversion).
    sel = const("CLKINSEL", "INV_CLKINSEL" in c.flags)
    if sel != 1:
        err(f"CLKINSEL is {sel if sel is not None else 'not constant'}; only CLKIN1 (CLKINSEL = 1) is modelled")
    # CMT `ZINV_<pin>` set means the pin IS inverted (unlike BUFGCTRL's):
    # Vivado leaves RST, PWRDWN, PSEN and PSINCDEC on the INT tile's default
    # VCC (traced in its ZedBoard build) with these bits set, while its
    # design holds them low; CLKINSEL likewise stays on VCC with
    # INV_CLKINSEL clear, selecting CLKIN1.
    for p in ("PSEN", "DEN", "DWE") if kind == "MMCME2_ADV" else ("DEN", "DWE"):
        inv = p == "PSEN" and "ZINV_PSEN" in c.flags
        v = const(p, inv)
        if v != 0:
            err(f"{p} is {v if v is not None else 'not constant'}: dynamic phase shift / DRP is not modelled")
    # Feedback: CLKFBIN from this CMT's CLKFBOUT, directly or through one BUFG.
    fb = x.pin_driver(s, "CLKFBIN")
    ok = fb[:3] == ("site", s.name, "CLKFBOUT")
    if fb[0] == "site" and x.sites[fb[1]].stype == "BUFGCTRL":
        b = x.sites[fb[1]]
        ok = any(x.pin_driver(b, p)[:3] == ("site", s.name, "CLKFBOUT") for p in ("I0", "I1"))
    if not ok:
        err(f"feedback loop not closed: CLKFBIN is driven by {fb}, not this CMT's CLKFBOUT (it would never lock)")
    clkin = pin("CLKIN1", "clock in")
    rst = pin("RST", "reset", inv="ZINV_RST" in c.flags)
    pwrdwn = pin("PWRDWN", "power down", inv="ZINV_PWRDWN" in c.flags)
    outs = c.outputs()
    o8 = [8 * o.divide for o in outs] + [8] * (7 - n)
    hh = [o.high_half for o in outs] + [1] * (7 - n)
    ph = [o.phase8 for o in outs] + [0] * (7 - n)
    en = sum(1 << i for i, o in enumerate(outs) if o.enabled)
    names = {f"CLKOUT{i}": sig(s.name, f"CLKOUT{i}") for i in range(7)}
    model_out = {}
    for i in range(7):
        w = names[f"CLKOUT{i}"]
        if i >= n or not outs[i].enabled:
            # The model drives a dummy; the site pin reads X (an error if used).
            nl.lines.append(f"wire {w} = 1'bx;")
            x.x_outputs[w] = f"{s.name}.CLKOUT{i}: counter output not enabled"
            model_out[i] = sig(s.name, f"CLKOUT{i}_off")
            nl.lines.append(f"wire {model_out[i]};")
        else:
            nl.lines.append(f"wire {w};")
            model_out[i] = w
    for o, why in (("CLKFBOUT", None), ("LOCKED", None)):
        nl.lines.append(f"wire {sig(s.name, o)};")
    for o in (["CLKOUT0B", "CLKOUT1B", "CLKOUT2B", "CLKOUT3B", "CLKFBOUTB", "PSDONE"] if kind == "MMCME2_ADV" else []) + \
            ["DRDY", "CLKINSTOPPED", "CLKFBSTOPPED"] + [f"DO{i}" for i in range(16)]:
        w = sig(s.name, o)
        nl.lines.append(f"wire {w} = 1'bx;")
        x.x_outputs[w] = f"{s.name}.{o}: not modelled"
    params = dict(M8=8 * c.m, D=c.d, O8=_packed(o8), HH=_packed(hh), PH8=_packed(ph), EN=f"7'd{en}")
    ps = ", ".join(f".{k}({v})" for k, v in params.items())
    conn = dict(CLKIN=clkin, RST=rst, PWRDWN=pwrdwn, CLKFBOUT=sig(s.name, "CLKFBOUT"), LOCKED=sig(s.name, "LOCKED"),
                CLKOUT="{" + ",".join(model_out[i] for i in reversed(range(7))) + "}")
    cs = ", ".join(f".{k}({v})" for k, v in conn.items())
    nl.lines.append(f"// {s.name} ({s.tile}.{s.fprefix}) M={c.m} D={c.d} "
                    + " ".join(f"O{i}={o.divide}/{o.phase8}" for i, o in enumerate(outs) if o.enabled))
    nl.lines.append(f"x7v_cmt #({ps}) u_{sig(s.name, 'CMT')} ({cs});")


# ---- behavioural Verilog --------------------------------------------------------

VERILOG_MODEL = r"""
// x7v behavioural model of one MMCM/PLL, written from UG472 (not a Xilinx
// simulation model). M8: feedback multiply in eighths; D: input divide;
// per output (16-bit fields, CLKOUT0 lowest): O8 divide in eighths, HH high
// time in half VCO periods, PH8 first-edge delay in eighths of a VCO period;
// EN: enabled outputs. LOCKED rises LOCK_CYCLES CLKIN edges after RST and
// PWRDWN are low; outputs start from that edge, INSERTION ns late.
`timescale 1ns/1ps
module x7v_cmt #(parameter integer M8 = 64, parameter integer D = 1,
                 parameter [111:0] O8 = {7{16'd8}}, parameter [111:0] HH = {7{16'd1}},
                 parameter [111:0] PH8 = {7{16'd0}}, parameter [6:0] EN = 7'd0,
                 parameter integer LOCK_CYCLES = 16, parameter real INSERTION = 0.05)
  (input CLKIN, input RST, input PWRDWN, output CLKFBOUT, output reg LOCKED = 1'b0, output [6:0] CLKOUT);
  integer cnt = 0;
  realtime tprev = 0.0, tin = 0.0, tref = 0.0;
  reg run = 1'b0;
  assign CLKFBOUT = CLKIN;  // only its connection is checked (by the extractor)
  always @(posedge CLKIN or posedge RST or posedge PWRDWN) begin
    if (RST !== 1'b0 || PWRDWN !== 1'b0) begin
      cnt = 0; run = 1'b0; LOCKED <= 1'b0;
    end else begin
      if (cnt > 0) tin = $realtime - tprev;
      tprev = $realtime;
      if (cnt == LOCK_CYCLES) begin tref = $realtime; run = 1'b1; LOCKED <= 1'b1; end
      if (cnt <= LOCK_CYCLES) cnt = cnt + 1;
    end
  end
  genvar k;
  generate for (k = 0; k < 7; k = k + 1) begin : g
    localparam integer O = O8[16*k +: 16];
    localparam integer H = HH[16*k +: 16];
    localparam integer P = PH8[16*k +: 16];
    reg c = 1'b0;
    realtime tv, tn;
    assign CLKOUT[k] = c;
    if (EN[k]) begin : on
      always begin
        wait (run === 1'b1);
        tv = tin * D * 8.0 / M8;
        tn = tref + INSERTION + P * tv / 8.0;
        while (run === 1'b1) begin
          #(tn - $realtime) c = 1'b1;
          #(H * tv / 2.0) c = 1'b0;
          tn = tn + O * tv / 8.0;
        end
      end
    end
  end endgenerate
endmodule
"""

# Shims for the RTL side: Xilinx primitives (UG953 attributes) onto the same
# model. Parameters a design may set and the model ignores are declared so a
# simulator accepts them.
_COMMON = """parameter BANDWIDTH = "OPTIMIZED", parameter real CLKIN1_PERIOD = 0.0, parameter real REF_JITTER1 = 0.010,
  parameter STARTUP_WAIT = "FALSE", parameter integer DIVCLK_DIVIDE = 1, parameter real CLKFBOUT_PHASE = 0.0,"""


def _outs(n: int, mmcm: bool) -> str:
    ps = []
    for i in range(n):
        div = f"parameter real CLKOUT0_DIVIDE_F = 1.0" if (mmcm and i == 0) else f"parameter integer CLKOUT{i}_DIVIDE = 1"
        ps.append(f"{div}, parameter real CLKOUT{i}_DUTY_CYCLE = 0.5, parameter real CLKOUT{i}_PHASE = 0.0")
    return ",\n  ".join(ps)


def _shim(name: str, mmcm: bool, adv: bool) -> str:
    n = 7 if mmcm else 6
    mult = "parameter real CLKFBOUT_MULT_F = 5.0" if mmcm else "parameter integer CLKFBOUT_MULT = 5"
    extra = []
    if mmcm:
        extra.append('parameter CLKOUT4_CASCADE = "FALSE", parameter CLKFBOUT_USE_FINE_PS = "FALSE"')
        extra.append(", ".join(f'parameter CLKOUT{i}_USE_FINE_PS = "FALSE"' for i in range(7)))
        if adv:
            extra.append('parameter SS_EN = "FALSE", parameter SS_MODE = "CENTER_HIGH", parameter integer SS_MOD_PERIOD = 10000')
    if adv:
        extra.append('parameter COMPENSATION = "ZHOLD", parameter real CLKIN2_PERIOD = 0.0, parameter real REF_JITTER2 = 0.010')
        extra.append(", ".join(f"parameter [0:0] IS_{p}_INVERTED = 1'b0" for p in
                               (["CLKINSEL", "PSEN", "PSINCDEC", "PWRDWN", "RST"] if mmcm else ["CLKINSEL", "PWRDWN", "RST"])))
    div = lambda i: ("CLKOUT0_DIVIDE_F" if (mmcm and i == 0) else f"CLKOUT{i}_DIVIDE")
    o8l = [f"localparam integer O8_{i} = $rtoi({div(i)} * 8.0 + 0.5);" for i in range(n)]
    hhl = [f"localparam integer HH_{i} = $rtoi(CLKOUT{i}_DUTY_CYCLE * {div(i)} * 2.0 + 0.5);" for i in range(n)]
    phl = [f"localparam integer PH8_{i} = $rtoi((CLKOUT{i}_PHASE < 0.0 ? CLKOUT{i}_PHASE + 360.0 : CLKOUT{i}_PHASE) / 360.0 * {div(i)} * 8.0 + 0.5);"
           for i in range(n)]
    pad = 7 - n
    cat = lambda pre, fill: "{" + ", ".join([f"16'd{fill}"] * pad + [f"{pre}{i}[15:0]" for i in reversed(range(n))]) + "}"
    m8 = "$rtoi(CLKFBOUT_MULT_F * 8.0 + 0.5)" if mmcm else "CLKFBOUT_MULT * 8"
    outs = ", ".join(f"output CLKOUT{i}" for i in range(n))
    bouts = ", output CLKOUT0B, output CLKOUT1B, output CLKOUT2B, output CLKOUT3B, output CLKFBOUTB" if mmcm else ""
    ins = "input CLKIN1, input CLKFBIN, input RST, input PWRDWN"
    if adv:
        ins += ", input CLKIN2, input CLKINSEL, input [6:0] DADDR, input DCLK, input DEN, input [15:0] DI, input DWE"
        if mmcm:
            ins += ", input PSCLK, input PSEN, input PSINCDEC"
        bouts += ", output [15:0] DO, output DRDY, output CLKINSTOPPED, output CLKFBSTOPPED" + (", output PSDONE" if mmcm else "")
    rst = "(RST ^ IS_RST_INVERTED)" if adv else "RST"
    pwr = "(PWRDWN ^ IS_PWRDWN_INVERTED)" if adv else "PWRDWN"
    body = "\n  ".join(o8l + hhl + phl)
    en = "7'd" + str((1 << n) - 1)
    return f"""
module {name} #({_COMMON}
  {mult},
  {_outs(n, mmcm)}{(',' + chr(10) + '  ' + (',' + chr(10) + '  ').join(extra)) if extra else ''})
  ({ins}, output CLKFBOUT, {outs}{bouts}, output LOCKED);
  {body}
  wire [6:0] co;
  x7v_cmt #(.M8({m8}), .D(DIVCLK_DIVIDE),
    .O8({cat('O8_', 8)}), .HH({cat('HH_', 1)}), .PH8({cat('PH8_', 0)}), .EN({en}))
    u (.CLKIN(CLKIN1), .RST({rst}), .PWRDWN({pwr}), .CLKFBOUT(CLKFBOUT), .LOCKED(LOCKED), .CLKOUT(co));
  assign {{{', '.join(f'CLKOUT{i}' for i in reversed(range(n)))}}} = co[{n - 1}:0];
endmodule
"""


RTL_PRIMS = ("// x7v primitive shims for RTL simulation (see x7v/mmcm.py)\n`timescale 1ns/1ps\n"
             + "".join(_shim(nm, mm, adv) for nm, mm, adv in (("MMCME2_BASE", True, False), ("MMCME2_ADV", True, True),
                                                             ("PLLE2_BASE", False, False), ("PLLE2_ADV", False, True)))
             + "\nmodule BUFG(input I, output O); assign O = I; endmodule\n"
             + 'module BUFGCE #(parameter SIM_DEVICE = "7SERIES") (input I, input CE, output O); assign O = I & CE; endmodule\n'
             + VERILOG_MODEL)

PRIM_NAMES = ("MMCME2_BASE", "MMCME2_ADV", "PLLE2_BASE", "PLLE2_ADV", "BUFG", "BUFGCE")

"""DSP48E1: decode the configuration of every DSP slice and model it for co-simulation.

What the model rests on (no Helion code):

* Project X-Ray's DSP_L/DSP_R database. A DSP tile holds two DSP48E1 sites,
  FASM prefixes `DSP48.DSP_0` (site X0Y0, wires `DSP_0_*`) and `DSP48.DSP_1`.
  Their bits:
    - `AREG_0`, `AREG_2` (neither: AREG = 1), same for B;
      `ZAREG_2_ACASCREG_1` is stored inverted: clear means AREG = 2 with
      ACASCREG = 1, set means ACASCREG = AREG (same for B);
    - `Z<reg>REG[0]` set means that register is off (CREG, DREG, ADREG,
      MREG, PREG, CARRYINREG, CARRYINSELREG, INMODEREG, OPMODEREG, ALUMODEREG);
    - `A_INPUT[0]` / `B_INPUT[0]` set: CASCADE (ACIN/BCIN); `USE_DPORT[0]`;
    - `USE_SIMD_FOUR12_TWO24` (TWO24 or FOUR12) plus `USE_SIMD_FOUR12` (FOUR12);
    - `AUTORESET_PATDET_RESET` (RESET_MATCH), `..._RESET_NOT_MATCH`;
    - `SEL_MASK_C`, `SEL_MASK_ROUNDING_MODE1`, `..._MODE2` (MODE2 also sets
      MODE1's bit), none: MASK; `PATTERN[47:0]`, `MASK[47:0]` stored as is;
    - `ZIS_<pin>_INVERTED` set means the pin is not inverted (CLK, CARRYIN,
      ALUMODE[3:0], INMODE[4:0], OPMODE[6:0]).
  An unused slice has every bit clear; a used one always sets some (the
  ZIS_ bits of its non-inverted pins, its AREG/BREG encoding).
  USE_MULT, USE_PATTERN_DETECT and SEL_PATTERN have no bit in the database.
* UG479 (7 Series DSP48E1 Slice) for behaviour: A/B two-deep input
  pipelines (AREG = 1 is the second register, CEA2/CEB2; ACOUT/BCOUT from
  A1/B1 when AREG/BREG = 2 and ACASCREG/BCASCREG = 1), INMODE (A1/A2 and
  B1/B2 select, zero A, D enable, pre-adder subtract), 25-bit pre-adder
  D +/- A with DREG/ADREG, 25x18 signed multiply with MREG, OPMODE X/Y/Z
  muxes, ALUMODE: Z inverted by ALUMODE[0] and the result by ALUMODE[1]
  around X+Y+Z+CIN (add/sub) or the logic unit (XOR family when ALUMODE[3:2]
  = 01, AND/OR family when 11; Y = 0 or all ones picks the variant),
  CARRYINSEL, SIMD lanes, synchronous resets that win over clock enables,
  every register 0 at configuration.

Reported as errors rather than guessed: AUTORESET_PATDET (the pattern
source has no bit), pattern detector, OVERFLOW/UNDERFLOW and MULTSIGNOUT
outputs when anything reads them, MACC extend (OPMODE Z = 100), an illegal
OPMODE Z, cascade inputs that are needed but undriven, and AREG/BREG bits
that do not form a legal encoding. CARRYOUT/CARRYCASCOUT are modelled for
ALUMODE 0000 and 0011 only (the adder's own carry, which is what chains a
wide add or subtract); other modes give X.
"""
from __future__ import annotations

from dataclasses import dataclass, field

REGS = ("CREG", "DREG", "ADREG", "MREG", "PREG", "CARRYINREG", "CARRYINSELREG", "INMODEREG",
        "OPMODEREG", "ALUMODEREG")
SIMD_LANES = {"ONE48": 1, "TWO24": 2, "FOUR12": 4}


@dataclass
class Dsp:
    """One DSP48E1 as its configuration bits set it."""
    areg: int
    acascreg: int
    breg: int
    bcascreg: int
    regs: dict          # REGS -> 0/1
    a_input: str        # DIRECT / CASCADE
    b_input: str
    use_dport: bool
    use_simd: str
    autoreset: str      # NO_RESET / RESET_MATCH / RESET_NOT_MATCH
    sel_mask: str       # MASK / C / ROUNDING_MODE1 / ROUNDING_MODE2
    pattern: int
    mask: int
    inv: dict           # pin group -> IS_<pin>_INVERTED value (CLK, CARRYIN, ALUMODE, INMODE, OPMODE)
    problems: list = field(default_factory=list)


def _vec(F: set, name: str, n: int) -> int:
    return sum(1 << i for i in range(n) if f"{name}[{i}]" in F)


def _inreg(F: set, base: str, z: str, name: str, problems: list) -> tuple[int, int]:
    """(xREG, xCASCREG) from AREG_0 / AREG_2 / ZAREG_2_ACASCREG_1."""
    r0, r2 = f"{base}_0" in F, f"{base}_2" in F
    if r0 and r2:
        problems.append(f"{base}_0 and {base}_2 both set")
    reg = 0 if r0 else 2 if r2 else 1
    one_casc = z not in F
    if one_casc and reg != 2:
        problems.append(f"{z} clear (AREG/BREG = 2, cascade 1) but {name} = {reg}")
    return reg, (1 if one_casc else reg)


def decode_dsp(F: set) -> Dsp:
    """Site features (prefix stripped, e.g. `AREG_0`, `ZPREG[0]`) -> configuration."""
    problems: list[str] = []
    areg, acasc = _inreg(F, "AREG", "ZAREG_2_ACASCREG_1", "AREG", problems)
    breg, bcasc = _inreg(F, "BREG", "ZBREG_2_BCASCREG_1", "BREG", problems)
    simd = "FOUR12" if "USE_SIMD_FOUR12" in F else "TWO24" if "USE_SIMD_FOUR12_TWO24" in F else "ONE48"
    if simd == "FOUR12" and "USE_SIMD_FOUR12_TWO24" not in F:
        problems.append("USE_SIMD_FOUR12 without USE_SIMD_FOUR12_TWO24")
    ar = ("RESET_NOT_MATCH" if "AUTORESET_PATDET_RESET_NOT_MATCH" in F
          else "RESET_MATCH" if "AUTORESET_PATDET_RESET" in F else "NO_RESET")
    sm = ("ROUNDING_MODE2" if "SEL_MASK_ROUNDING_MODE2" in F else "ROUNDING_MODE1" if "SEL_MASK_ROUNDING_MODE1" in F
          else "C" if "SEL_MASK_C" in F else "MASK")
    inv = {"CLK": int("ZIS_CLK_INVERTED" not in F), "CARRYIN": int("ZIS_CARRYIN_INVERTED" not in F)}
    for g, n in (("ALUMODE", 4), ("INMODE", 5), ("OPMODE", 7)):
        inv[g] = ((1 << n) - 1) ^ _vec(F, f"ZIS_{g}_INVERTED", n)
    return Dsp(areg, acasc, breg, bcasc, {r: int(f"Z{r}[0]" not in F) for r in REGS},
               "CASCADE" if "A_INPUT[0]" in F else "DIRECT", "CASCADE" if "B_INPUT[0]" in F else "DIRECT",
               "USE_DPORT[0]" in F, simd, ar, sm, _vec(F, "PATTERN", 48), _vec(F, "MASK", 48), inv, problems)


def tile_dsps(dec) -> dict[str, dict[str, Dsp]]:
    """{DSP tile: {"DSP_0"/"DSP_1": Dsp}} for every configured DSP48E1."""
    out: dict[str, dict[str, Dsp]] = {}
    for t, td in dec.tiles.items():
        if td.tile_type not in ("DSP_L", "DSP_R"):
            continue
        for d in ("DSP_0", "DSP_1"):
            pre = f"DSP48.{d}."
            F = {f[len(pre):] for f in td.features if f.startswith(pre)}
            if F:
                out.setdefault(t, {})[d] = decode_dsp(F)
    return out


def summary(dec) -> list[str]:
    """One line per configured DSP48E1 (the `x7v dsp` report)."""
    lines = []
    for t, ds in sorted(tile_dsps(dec).items()):
        for name, d in sorted(ds.items()):
            regs = " ".join(f"{r}={v}" for r, v in d.regs.items())
            inv = " ".join(f"IS_{g}_INVERTED={v:b}" for g, v in d.inv.items() if v)
            lines.append(
                f"{t}.{name}: AREG={d.areg} ACASCREG={d.acascreg} BREG={d.breg} BCASCREG={d.bcascreg} {regs} "
                f"A_INPUT={d.a_input} B_INPUT={d.b_input} USE_DPORT={str(d.use_dport).upper()} USE_SIMD={d.use_simd} "
                f"AUTORESET_PATDET={d.autoreset} SEL_MASK={d.sel_mask} PATTERN=48'h{d.pattern:012x} MASK=48'h{d.mask:012x}"
                + (f" {inv}" if inv else "") + (f" PROBLEMS: {'; '.join(d.problems)}" if d.problems else ""))
    return lines


# ---- netlist model --------------------------------------------------------------

def model_dsp48e1(x, s, nl):
    from .netlist import sig  # netlist imports this module
    d = decode_dsp(s.features)
    tt = x.fab.tt(x.db.tilegrid[s.tile]["type"])
    err = lambda m: x.errors.append(f"{s.name} ({s.tile}.{s.fprefix}): {m}")
    if not s.features:
        err("drives logic but is not configured (every bit clear)")
    for p in d.problems:
        err(f"bad encoding: {p}")
    if d.autoreset != "NO_RESET":
        err(f"AUTORESET_PATDET = {d.autoreset} not modelled (SEL_PATTERN has no X-Ray bit)")

    def drv(pin: str):
        return x.pin_driver(s, pin)

    def bit(pin: str, inv: int = 0, what: str = "") -> str:
        dr = drv(pin)
        if dr[0] not in ("site", "const") and what:
            err(f"{pin} ({what}) is undriven")
        e = x.expr(dr)
        return f"(~{e})" if inv else e

    def bus(pin: str, n: int, inv: int = 0, what: str = "") -> str:
        return "{" + ",".join(bit(f"{pin}{i}", inv >> i & 1, what) for i in reversed(range(n))) + "}"

    def const(pin: str, n: int, inv: int = 0):
        v = 0
        for i in range(n):
            dr = drv(f"{pin}{i}")
            if dr[0] != "const":
                return None
            v |= (dr[1] ^ (inv >> i & 1)) << i
        return v

    def casc(pin: str, n: int, what: str, need: bool) -> str:
        # A cascade input comes from the slice below over dedicated wires.
        # When only a dynamic OPMODE/CARRYINSEL might select it, an input
        # with no configured slice behind it reads X instead of an error.
        pins = [f"{pin}{i}" if n > 1 else pin for i in reversed(range(n))]
        wires = [tt.sites[s.index]["site_pins"][p]["wire"] for p in pins]
        ds = [x.driver(x.fab.node(s.tile, w)) for w in wires]
        if all(dr[0] == "site" and x.sites[dr[1]].features for dr in ds):
            return "{" + ",".join(bit(p) for p in pins) + "}"
        if need:
            err(f"{what} cascade input {pin} is used but not driven by a configured DSP slice")
        return f"{{{n}{{1'bx}}}}"

    opmode = const("OPMODE", 7, d.inv["OPMODE"])
    csel = const("CARRYINSEL", 3)
    zsel = None if opmode is None else opmode >> 4
    if zsel == 4:
        err("OPMODE Z = 100 (MACC extend, MULTSIGNIN) not modelled")
    if zsel == 7:
        err("OPMODE Z = 111 is illegal")
    uses_c = opmode is None or (opmode >> 2 & 3) == 3 or zsel == 3 or d.sel_mask != "MASK"

    clkd = drv("CLK")
    clk = x.expr(clkd) if clkd[0] == "site" else "1'b0"  # an unclocked slice's registers hold 0
    if clk != "1'b0" and d.inv["CLK"]:
        clk = f"(~{clk})"
    conn = {
        "CLK": clk,
        "A": "30'b0" if d.a_input == "CASCADE" else bus("A", 30),
        "ACIN": casc("ACIN", 30, "A", True) if d.a_input == "CASCADE" else "30'b0",
        "B": "18'b0" if d.b_input == "CASCADE" else bus("B", 18),
        "BCIN": casc("BCIN", 18, "B", True) if d.b_input == "CASCADE" else "18'b0",
        "C": bus("C", 48) if uses_c else "48'b0",
        "D": bus("D", 25) if d.use_dport else "25'b0",
        "PCIN": casc("PCIN", 48, "P", zsel in (1, 5) or csel in (1, 3))
                if zsel in (None, 1, 5) or csel in (None, 1, 3) else "48'b0",
        "CARRYCASCIN": casc("CARRYCASCIN", 1, "carry", csel == 2) if csel in (None, 2) else "1'b0",
        "CARRYIN": bit("CARRYIN", d.inv["CARRYIN"], "carry in") if csel in (None, 0) else "1'b0",
        "CARRYINSEL": bus("CARRYINSEL", 3, 0, "carry-in select"),
        "OPMODE": bus("OPMODE", 7, d.inv["OPMODE"], "OPMODE"),
        "ALUMODE": bus("ALUMODE", 4, d.inv["ALUMODE"], "ALUMODE"),
        "INMODE": bus("INMODE", 5, d.inv["INMODE"], "INMODE"),
    }
    for p in ("CEA1", "CEA2", "CEB1", "CEB2", "CEC", "CED", "CEAD", "CEM", "CEP", "CECARRYIN", "CECTRL",
              "CEINMODE", "CEALUMODE"):
        conn[p] = bit(p, 0, "clock enable")
    for p in ("RSTA", "RSTB", "RSTC", "RSTD", "RSTM", "RSTP", "RSTALLCARRYIN", "RSTCTRL", "RSTINMODE",
              "RSTALUMODE"):
        conn[p] = bit(p, 0, "reset")
    for port, n in (("P", 48), ("PCOUT", 48), ("ACOUT", 30), ("BCOUT", 18), ("CARRYOUT", 4), ("CARRYCASCOUT", 1)):
        names = [sig(s.name, f"{port}{i}" if n > 1 else port) for i in reversed(range(n))]
        nl.lines.extend(f"wire {o};" for o in names)
        conn[port] = "{" + ",".join(names) + "}"
    for port, why in (("PATTERNDETECT", "the pattern detector is not modelled (SEL_PATTERN has no X-Ray bit)"),
                      ("PATTERNBDETECT", "the pattern detector is not modelled (SEL_PATTERN has no X-Ray bit)"),
                      ("OVERFLOW", "the pattern detector is not modelled (SEL_PATTERN has no X-Ray bit)"),
                      ("UNDERFLOW", "the pattern detector is not modelled (SEL_PATTERN has no X-Ray bit)"),
                      ("MULTSIGNOUT", "MACC extend is not modelled")):
        o = sig(s.name, port)
        nl.lines.append(f"wire {o} = 1'bx;")
        x.x_outputs[o] = f"{s.name}.{port}: {why}"
    params = dict(AREG=d.areg, ACASCREG=d.acascreg, BREG=d.breg, BCASCREG=d.bcascreg, **d.regs,
                  USE_DPORT=int(d.use_dport), LANES=SIMD_LANES[d.use_simd])
    ps = ", ".join(f".{k}({v})" for k, v in params.items())
    cs = ", ".join(f".{k}({v})" for k, v in conn.items())
    nl.lines.append(f"// {s.name} ({s.tile}.{s.fprefix}) USE_SIMD={d.use_simd} A_INPUT={d.a_input} B_INPUT={d.b_input}")
    nl.lines.append(f"x7v_dsp48e1 #({ps}) u_{sig(s.name, 'DSP')} ({cs});")


# ---- behavioural Verilog --------------------------------------------------------

VERILOG_MODEL = r"""
// x7v behavioural model of one DSP48E1, written from UG479 (not a Xilinx
// simulation model). Pin inversions and A_INPUT/B_INPUT are applied by the
// netlist (cascade inputs arrive on ACIN/BCIN, A/B tied 0, or the reverse).
// LANES: USE_SIMD ONE48 = 1, TWO24 = 2, FOUR12 = 4. Every register is 0 at
// configuration; resets are synchronous and win over clock enables.
module x7v_dsp48e1 #(
  parameter integer AREG = 1, ACASCREG = 1, BREG = 1, BCASCREG = 1, CREG = 1, DREG = 1, ADREG = 1,
  parameter integer MREG = 1, PREG = 1, CARRYINREG = 1, CARRYINSELREG = 1, INMODEREG = 1,
  parameter integer OPMODEREG = 1, ALUMODEREG = 1, USE_DPORT = 0, LANES = 1
)(
  input CLK,
  input [29:0] A, ACIN, input [17:0] B, BCIN, input [47:0] C, PCIN, input [24:0] D,
  input CARRYIN, CARRYCASCIN, input [2:0] CARRYINSEL, input [6:0] OPMODE, input [3:0] ALUMODE, input [4:0] INMODE,
  input CEA1, CEA2, CEB1, CEB2, CEC, CED, CEAD, CEM, CEP, CECARRYIN, CECTRL, CEINMODE, CEALUMODE,
  input RSTA, RSTB, RSTC, RSTD, RSTM, RSTP, RSTALLCARRYIN, RSTCTRL, RSTINMODE, RSTALUMODE,
  output [47:0] P, PCOUT, output [29:0] ACOUT, output [17:0] BCOUT, output [3:0] CARRYOUT, output CARRYCASCOUT
);
  localparam integer W = 48 / LANES;
  // control registers
  reg [4:0] im_r = 0; reg [6:0] om_r = 0; reg [3:0] al_r = 0; reg [2:0] cs_r = 0;
  always @(posedge CLK) if (RSTINMODE) im_r <= 0; else if (CEINMODE) im_r <= INMODE;
  always @(posedge CLK) if (RSTCTRL) begin om_r <= 0; cs_r <= 0; end
                        else if (CECTRL) begin om_r <= OPMODE; cs_r <= CARRYINSEL; end
  always @(posedge CLK) if (RSTALUMODE) al_r <= 0; else if (CEALUMODE) al_r <= ALUMODE;
  wire [4:0] im = INMODEREG ? im_r : INMODE;
  wire [6:0] om = OPMODEREG ? om_r : OPMODE;
  wire [3:0] al = ALUMODEREG ? al_r : ALUMODE;
  wire [2:0] cs = CARRYINSELREG ? cs_r : CARRYINSEL;
  // A and B: two-deep pipelines; a single register (xREG = 1) is the second one.
  wire [29:0] a_in = A | ACIN;
  wire [17:0] b_in = B | BCIN;
  reg [29:0] a1 = 0, a2 = 0; reg [17:0] b1 = 0, b2 = 0;
  wire [29:0] a1q = AREG == 2 ? a1 : a_in;
  wire [17:0] b1q = BREG == 2 ? b1 : b_in;
  always @(posedge CLK) if (RSTA) begin a1 <= 0; a2 <= 0; end else begin
    if (CEA1) a1 <= a_in;
    if (CEA2) a2 <= a1q;
  end
  always @(posedge CLK) if (RSTB) begin b1 <= 0; b2 <= 0; end else begin
    if (CEB1) b1 <= b_in;
    if (CEB2) b2 <= b1q;
  end
  wire [29:0] a2q = AREG == 0 ? a_in : a2;
  wire [17:0] b2q = BREG == 0 ? b_in : b2;
  assign ACOUT = (AREG == 2 && ACASCREG == 1) ? a1 : a2q;
  assign BCOUT = (BREG == 2 && BCASCREG == 1) ? b1 : b2q;
  // INMODE: [0] A1 not A2, [1] zero A, [2] D into the pre-adder, [3] subtract A, [4] B1 not B2.
  wire [24:0] a25 = im[1] ? 25'd0 : (im[0] ? a1q[24:0] : a2q[24:0]);
  wire [17:0] bm = im[4] ? b1q : b2q;
  reg [24:0] d_r = 0, ad_r = 0;
  always @(posedge CLK) if (RSTD) d_r <= 0; else if (CED) d_r <= D;
  wire [24:0] dq = DREG ? d_r : D;
  wire [24:0] ad = (im[2] ? dq : 25'd0) + (im[3] ? -a25 : a25);
  always @(posedge CLK) if (RSTD) ad_r <= 0; else if (CEAD) ad_r <= ad;
  wire [24:0] am = USE_DPORT ? (ADREG ? ad_r : ad) : a25;
  // 25 x 18 signed multiply; the rounding bit A[24] XNOR B[17] rides with M.
  wire signed [42:0] m = $signed(am) * $signed(bm);
  reg [42:0] m_r = 0; reg rnd_r = 0;
  always @(posedge CLK) if (RSTM) m_r <= 0; else if (CEM) m_r <= m;
  always @(posedge CLK) if (RSTALLCARRYIN) rnd_r <= 0; else if (CEM) rnd_r <= ~(am[24] ^ bm[17]);
  wire [42:0] mq = MREG ? m_r : m;
  wire rnd = MREG ? rnd_r : ~(am[24] ^ bm[17]);
  reg [47:0] c_r = 0;
  always @(posedge CLK) if (RSTC) c_r <= 0; else if (CEC) c_r <= C;
  wire [47:0] cq = CREG ? c_r : C;
  // P register (with its carries); P feedback always comes from it.
  reg [47:0] p_r = 0; reg [3:0] co_r = 0;
  // X / Y / Z multiplexers. M is one 48-bit value on X; Y adds 0 for it.
  reg [47:0] xm, ym, zm;
  always @* begin
    case (om[1:0]) 2'b00: xm = 0; 2'b01: xm = {{5{mq[42]}}, mq}; 2'b10: xm = p_r; default: xm = {a2q, b2q}; endcase
    case (om[3:2]) 2'b00: ym = 0; 2'b01: ym = 0; 2'b10: ym = {48{1'b1}}; default: ym = cq; endcase
    if ((om[1:0] == 2'b01) != (om[3:2] == 2'b01)) begin xm = {48{1'bx}}; ym = {48{1'bx}}; end
    case (om[6:4])
      3'b000: zm = 0; 3'b001: zm = PCIN; 3'b010: zm = p_r; 3'b011: zm = cq;
      3'b101: zm = {{17{PCIN[47]}}, PCIN[47:17]}; 3'b110: zm = {{17{p_r[47]}}, p_r[47:17]};
      default: zm = {48{1'bx}};  // 100 MACC extend (not modelled), 111 illegal
    endcase
  end
  // carry in
  reg cin_r = 0;
  always @(posedge CLK) if (RSTALLCARRYIN) cin_r <= 0; else if (CECARRYIN) cin_r <= CARRYIN;
  reg cin;
  always @* case (cs)
    3'd0: cin = CARRYINREG ? cin_r : CARRYIN;
    3'd1: cin = ~PCIN[47];
    3'd2: cin = CARRYCASCIN;
    3'd3: cin = PCIN[47];
    3'd4: cin = co_r[3];
    3'd5: cin = ~p_r[47];
    3'd6: cin = rnd;
    default: cin = p_r[47];
  endcase
  // ALU: ALUMODE[0] inverts Z, ALUMODE[1] inverts the result.
  wire [47:0] zi = al[0] ? ~zm : zm;
  reg [47:0] sum; reg [3:0] co; reg [49:0] t;
  integer l;
  always @* begin
    sum = 0; co = 4'bxxxx;
    for (l = 0; l < LANES; l = l + 1) begin
      t = zi[l*W +: W] + xm[l*W +: W] + ym[l*W +: W] + (l == 0 ? cin : 1'b0);
      sum[l*W +: W] = t[W-1:0];
      co[(l + 1) * 4 / LANES - 1] = t[W];
    end
  end
  wire [47:0] lg = al[3] ? (ym == 0 ? (xm & zi) : ym == {48{1'b1}} ? (xm | zi) : {48{1'bx}}) : (xm ^ ym ^ zi);
  wire [47:0] r0 = al[3:2] == 2'b10 ? {48{1'bx}} : al[2] ? lg : sum;
  wire [47:0] alu = al[1] ? ~r0 : r0;
  wire [3:0] alu_co = (al[3:2] == 2'b00 && al[1] == al[0]) ? co : 4'bxxxx;
  always @(posedge CLK) if (RSTP) begin p_r <= 0; co_r <= 0; end
                        else if (CEP) begin p_r <= alu; co_r <= alu_co; end
  assign P = PREG ? p_r : alu;
  assign PCOUT = P;
  assign CARRYOUT = PREG ? co_r : alu_co;
  assign CARRYCASCOUT = CARRYOUT[3];
endmodule
"""

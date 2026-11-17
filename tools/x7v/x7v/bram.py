"""Block RAM: decode RAMB18E1 / RAMB36E1 configuration and model it for co-simulation.

What the model rests on (no Helion code):

* Project X-Ray's BRAM_L/BRAM_R database. A BRAM tile holds two RAMB18
  halves, `RAMB18_Y0` (site FIFO18E1, wires `BRAM_FIFO18_*`) and
  `RAMB18_Y1` (site RAMB18E1, wires `BRAM_RAMB18_*`), each with its own
  configuration bits (widths, write modes, registers, INIT/SRVAL, INIT_xx
  and INITP_xx contents). A RAMB36 has no storage of its own: the tile's
  RAMB36 site pins are wired to the two halves bit-interleaved
  (`DOADO[2k]` is the lower half's `DOADO[k]`, `DOADO[2k+1]` the upper's,
  same for DI/DIP/DOP), and both halves see the same address. So x7v models
  the two halves, each exactly as its own bits configure it, which is the
  hardware whether the design called it one RAMB36E1 or two RAMB18E1s.
* UG473 (7 Series FPGAs Memory Resources) for behaviour: synchronous read,
  port widths 1/2/4/9/18 (SDP 36), WRITE_FIRST / READ_FIRST / NO_CHANGE,
  optional output register (DO_REG, REGCE, RSTREG, RSTREG_PRIORITY),
  RSTRAM resetting the output latch to SRVAL, INIT_A/B as the power-up
  output value. Memory layout: a width-w port at address a reads data bits
  [a*d + d-1 : a*d] of the 16 Kb array (d = 1, 2, 4, 8, 16, 32 data bits)
  and parity bits [a*p + p-1 : a*p] of the 2 Kb parity array (p = 0, 0, 0,
  1, 2, 4); INIT_xx holds data bits [256*xx + 255 : 256*xx], INITP_xx
  parity bits.
* X-Ray feature naming: `Z` features are stored inverted (ZINIT_A[i] set
  means INIT_A[i] = 0), `ZINV_<pin>` set means the pin is not inverted.

Reported as errors rather than guessed (none is needed for a plain RAM or
ROM): FIFO mode, ECC, RAMB36 cascade, RAMB36 1-bit width mode (the halves'
address-LSB select), per-byte write enables, WRITE_FIRST with different
read and write widths on one port, and SDP with DOA_REG != DOB_REG.
"""
from __future__ import annotations

import re
from dataclasses import dataclass

HALVES = {"FIFO18E1": ("RAMB18_Y0", "BRAM_FIFO18_"), "RAMB18E1": ("RAMB18_Y1", "BRAM_RAMB18_")}
WRITE_MODES = ("WRITE_FIRST", "READ_FIRST", "NO_CHANGE")
# Tile features a plain RAM never sets: FIFO, cascade and ECC machinery.
# (CASCOUT_ARD/BWR_ACTIVE are not among them: they drive the address buses
# to the tile above, which Vivado uses as routing; see model_ramb18.)
UNMODELLED_TILE = ("EN_SYN", "FIRST_WORD_FALL_THROUGH",
                   "RAMB36.EN_ECC_READ", "RAMB36.EN_ECC_WRITE", "RAMB36.RAM_EXTENSION_A_LOWER",
                   "RAMB36.RAM_EXTENSION_B_LOWER", "RAMB36.BRAM36_READ_WIDTH_A_1",
                   "RAMB36.BRAM36_READ_WIDTH_B_1", "RAMB36.BRAM36_WRITE_WIDTH_A_1",
                   "RAMB36.BRAM36_WRITE_WIDTH_B_1")


@dataclass
class Half:
    """One RAMB18 half as its configuration bits set it."""
    in_use: bool
    ra: int            # read width A (36 = SDP read on port A)
    rb: int
    wa: int
    wb: int            # write width B (36 = SDP write on port B)
    wmode_a: str
    wmode_b: str
    doa_reg: bool
    dob_reg: bool
    regce_pri_a: bool  # RSTREG_PRIORITY_A = REGCE
    regce_pri_b: bool
    init_a: int        # 18 bits each
    init_b: int
    srval_a: int
    srval_b: int
    data: int          # 16384 bits
    parity: int        # 2048 bits
    fifo: bool
    zinv: frozenset    # pins whose inversion is disabled

    @property
    def mode(self) -> str:
        return "SDP" if 36 in (self.ra, self.wb) else "TDP"


def _width(F: set, key: str) -> int:
    # Width 1 is the all-clear encoding, so it never shows as a feature.
    ws = [int(f.rsplit("_", 1)[1]) for f in F if f.startswith(key + "_")]
    return ws[0] if len(ws) == 1 else 1


def _vec(F: set, name: str, n: int) -> int:
    return sum(1 << i for i in range(n) if f"{name}[{i}]" in F)


def decode_half(F: set) -> Half:
    """Site features (prefix stripped, e.g. `READ_WIDTH_A_18`) -> configuration."""
    data = par = 0
    for f in F:
        m = re.match(r"^INIT(P?)_([0-9A-F]{2})\[(\d+)\]$", f)
        if m:
            bit = 256 * int(m.group(2), 16) + int(m.group(3))
            if m.group(1):
                par |= 1 << bit
            else:
                data |= 1 << bit
    wm = lambda p: next((m for m in WRITE_MODES[1:] if f"WRITE_MODE_{p}_{m}" in F), "WRITE_FIRST")
    full = (1 << 18) - 1
    return Half(
        in_use="IN_USE" in F,
        ra=36 if "SDP_READ_WIDTH_36" in F else _width(F, "READ_WIDTH_A"),
        rb=_width(F, "READ_WIDTH_B"),
        wa=_width(F, "WRITE_WIDTH_A"),
        wb=36 if "SDP_WRITE_WIDTH_36" in F else _width(F, "WRITE_WIDTH_B"),
        wmode_a=wm("A"), wmode_b=wm("B"),
        doa_reg="DOA_REG" in F, dob_reg="DOB_REG" in F,
        regce_pri_a="RSTREG_PRIORITY_A_REGCE" in F, regce_pri_b="RSTREG_PRIORITY_B_REGCE" in F,
        init_a=full ^ _vec(F, "ZINIT_A", 18), init_b=full ^ _vec(F, "ZINIT_B", 18),
        srval_a=full ^ _vec(F, "ZSRVAL_A", 18), srval_b=full ^ _vec(F, "ZSRVAL_B", 18),
        data=data, parity=par, fifo="FIFO_MODE" in F,
        zinv=frozenset(f[5:] for f in F if f.startswith("ZINV_")))


def tile_halves(dec) -> dict[str, dict[str, Half]]:
    """{BRAM tile: {"RAMB18_Y0"/"RAMB18_Y1": Half}} for every configured half."""
    out: dict[str, dict[str, Half]] = {}
    for t, td in dec.tiles.items():
        if td.tile_type not in ("BRAM_L", "BRAM_R"):
            continue
        for half in ("RAMB18_Y0", "RAMB18_Y1"):
            F = {f[len(half) + 1:] for f in td.features if f.startswith(half + ".")}
            if F:
                out.setdefault(t, {})[half] = decode_half(F)
    return out


def summary(dec) -> list[str]:
    """One line per configured BRAM half (the `x7v bram` report)."""
    lines = []
    for t, hs in sorted(tile_halves(dec).items()):
        both = len([h for h in hs.values() if h.in_use]) == 2
        for half, h in sorted(hs.items()):
            rows = len({i // 256 for i in range(16384) if h.data >> i & 1})
            lines.append(
                f"{t}.{half}{' (RAMB36 half)' if both else ''}: {'in use' if h.in_use else 'NOT IN_USE'} {h.mode} "
                f"A r{h.ra}/w{h.wa} {h.wmode_a}{' DOA_REG' if h.doa_reg else ''} "
                f"B r{h.rb}/w{h.wb} {h.wmode_b}{' DOB_REG' if h.dob_reg else ''} "
                f"INIT_A={h.init_a:05x} INIT_B={h.init_b:05x} SRVAL_A={h.srval_a:05x} SRVAL_B={h.srval_b:05x} "
                f"data rows {rows}/64, data sum32 {_sum32(h.data, 16384):08x}, parity sum32 {_sum32(h.parity, 2048):08x}")
    return lines


def _sum32(v: int, nbits: int) -> int:
    return sum((v >> (32 * i)) & 0xFFFFFFFF for i in range(nbits // 32)) & 0xFFFFFFFF


# ---- netlist models ------------------------------------------------------------

def model_ramb36(x, s, nl):
    """The RAMB36 site has no storage of its own (see module doc); only check
    that no RAMB36-only mode (ECC, cascade, 1-bit) is configured."""
    bad = sorted(f"RAMB36.{f}" for f in s.features if f"RAMB36.{f}" in UNMODELLED_TILE)
    if bad:
        x.errors.append(f"{s.name}: RAMB36 mode not modelled: {bad}")


def _net(x, d: tuple) -> tuple:
    """A slice's xMUX output set to O6 is the LUT's O6 net itself (Vivado
    feeds one write enable to WEA0..3 from both pins)."""
    if d[0] == "site" and d[2] in ("AMUX", "BMUX", "CMUX", "DMUX") and f"{d[2][0]}OUTMUX.O6" in x.sites[d[1]].features:
        return ("site", d[1], d[2][0])
    return d


def model_ramb18(x, s, nl):
    from .netlist import sig  # netlist imports this module
    half, wp = HALVES[s.stype]
    h = decode_half(s.features)
    tile_f = x.active.get(s.tile, set())
    err = lambda m: x.errors.append(f"{s.name} ({s.tile}.{half}): {m}")
    if not h.in_use:
        err(f"configured but not IN_USE: {sorted(s.features)[:6]}")
    if h.fifo:
        err("FIFO mode not modelled")
    bad = sorted(f for f in UNMODELLED_TILE if f in tile_f)
    if bad:
        err(f"FIFO/ECC/cascade/RAMB36 1-bit mode not modelled: {bad}")
    tt = x.fab.tt(x.db.tilegrid[s.tile]["type"])
    # An address bus read from the tile below (BRAM_CASCINBOT_* pips) is
    # driven only with CASCOUT_<bus>_ACTIVE set there: Vivado sets it in every
    # such case (168 of 168 in the ZedBoard BRAM fill), so its absence is an error.
    for bus, act in (("ADDRARDADDR", "CASCOUT_ARD_ACTIVE"), ("ADDRBWRADDR", "CASCOUT_BWR_ACTIVE")):
        w = next((f.split(".")[1] for f in sorted(tile_f) if f".BRAM_CASCINBOT_{bus}" in f), None)
        if w and not any(act in x.active.get(t, ()) for t, wn in x.fab.nodes[x.fab.node(s.tile, w)] if wn.startswith("BRAM_CASCOUT_")):
            m = f"{s.tile}: reads {bus} from the tile below ({w}) but no {act} drives it"
            if m not in x.errors:
                x.errors.append(m)

    def inp(pin: str, what: str, inv_pin: bool = False) -> str:
        d = x.driver(x.fab.node(s.tile, wp + pin))
        if d[0] == "site":
            x.used_sites.add(d[1])
        elif d[0] != "const":
            err(f"{pin} ({what}) is undriven")
        e = x.expr(d)
        return f"(~{e})" if inv_pin and pin not in h.zinv else e

    def clock(pin: str) -> str | None:
        # A clock pin no logic drives never ticks: the port does nothing and
        # its outputs hold INIT, which is what the device does too.
        d = x.driver(x.fab.node(s.tile, wp + pin))
        if d[0] != "site":
            return None
        x.used_sites.add(d[1])
        e = x.expr(d)
        return f"(~{e})" if pin not in h.zinv else e

    def bus(pin: str, n: int, used: int, what: str) -> str:
        return "{" + ",".join(inp(f"{pin}{i}", what) if i < used else "1'b0" for i in reversed(range(n))) + "}"

    sdp_r, sdp_w = h.ra == 36, h.wb == 36
    if sdp_r and h.doa_reg != h.dob_reg:
        err("SDP read with DOA_REG != DOB_REG not modelled")
    shift = {1: 0, 2: 1, 4: 2, 9: 3, 18: 4, 36: 5}
    ndata = {1: 1, 2: 2, 4: 4, 9: 8, 18: 16, 36: 16}
    npar = {9: 1, 18: 2, 36: 2}
    pins = {"A": ("CLKARDCLK", "ENARDEN", "RSTRAMARSTRAM", "REGCLKARDRCLK", "REGCEAREGCE", "RSTREGARSTREG", "WEA", "ADDRARDADDR", "DIADI", "DIPADIP"),
            "B": ("CLKBWRCLK", "ENBWREN", "RSTRAMB", "REGCLKB", "REGCEB", "RSTREGB", "WEBWE", "ADDRBWRADDR", "DIBDI", "DIPBDIP")}
    conn = {}
    for p in "AB":
        clk_p, en_p, rstram_p, regclk_p, regce_p, rstreg_p, we_p, addr_p, di_p, dip_p = pins[p]
        rw, ww = (h.ra, h.wa) if p == "A" else (h.rb, h.wb)
        # SDP: port A only reads (all 36 bits), port B only writes.
        reads = not (p == "B" and (sdp_r or sdp_w))
        writes = not (p == "A" and (sdp_r or sdp_w))
        c = dict(CLK="1'b0", EN="1'b0", WE="1'b0", RSTRAM="1'b0", REGCLK="1'b0", REGCE="1'b0", RSTREG="1'b0",
                 ADDR="14'b0")
        clk = clock(clk_p)
        en = inp(en_p, "enable", True) if clk else "1'b0"
        if en not in ("1'b0", "(~1'b1)"):
            c.update(CLK=clk, EN=en, RSTRAM=inp(rstram_p, "output latch reset", True))
            if writes:
                wes = [inp(f"{we_p}{i}", "write enable") for i in range(8 if sdp_w else 4)]
                if len({_net(x, x.driver(x.fab.node(s.tile, f"{wp}{we_p}{i}"))) for i in range(len(wes))}) > 1:
                    err(f"per-byte write enables not modelled ({we_p}0..{len(wes) - 1} = {wes})")
                writes = wes[0] != "1'b0"
                c["WE"] = wes[0] if writes else "1'b0"
            used_w = [w for w, on in ((rw, reads), (ww, writes)) if on]
            lo = min(shift[w] for w in used_w) if used_w else 14
            c["ADDR"] = "{" + ",".join(inp(f"{addr_p}{i}", "address") if i >= lo else "1'b0" for i in reversed(range(14))) + "}"
            wmode = h.wmode_a if p == "A" else h.wmode_b
            if reads and writes and wmode == "WRITE_FIRST" and rw != ww:
                err(f"port {p} WRITE_FIRST with read width {rw} != write width {ww} not modelled")
            if reads and (h.doa_reg if p == "A" else h.dob_reg):
                regclk = clock(regclk_p)
                if regclk:
                    c.update(REGCLK=regclk, REGCE=inp(regce_p, "output register enable"),
                             RSTREG=inp(rstreg_p, "output register reset", True))
            if writes:
                # An SDP write takes its upper 18 bits from port A's data pins.
                for q, w in ((p, ww), ("A", 36)) if sdp_w else ((p, ww),):
                    conn["DI" + q] = bus(pins[q][8], 16, ndata[w], "write data")
                    conn["DIP" + q] = bus(pins[q][9], 2, npar.get(w, 0), "write parity")
        for k, v in c.items():
            conn[k + p] = v
    for q in "AB":
        conn.setdefault("DI" + q, "16'b0")
        conn.setdefault("DIP" + q, "2'b0")
    outs = {}
    for port, wpre, n in (("DOA", "DOADO", 16), ("DOB", "DOBDO", 16), ("DOPA", "DOPADOP", 2), ("DOPB", "DOPBDOP", 2)):
        bits = []
        for i in reversed(range(n)):
            _, pin = tt.site_pin_wire[f"{wp}{wpre}{i}"]
            o = sig(s.name, pin)
            nl.lines.append(f"wire {o};")
            bits.append(o)
        outs[port] = "{" + ",".join(bits) + "}"
    wmi = {m: i for i, m in enumerate(WRITE_MODES)}
    params = dict(RA=h.ra, RB=h.rb, WA=h.wa, WB=h.wb, WMA=wmi[h.wmode_a], WMB=wmi[h.wmode_b],
                  DOA_REG=int(h.doa_reg), DOB_REG=int(h.dob_reg), PRIA=int(h.regce_pri_a), PRIB=int(h.regce_pri_b),
                  INIT_A=f"18'h{h.init_a:05x}", INIT_B=f"18'h{h.init_b:05x}",
                  SRVAL_A=f"18'h{h.srval_a:05x}", SRVAL_B=f"18'h{h.srval_b:05x}",
                  DATA=f"16384'h{h.data:04096x}", PAR=f"2048'h{h.parity:0512x}")
    ps = ", ".join(f".{k}({v})" for k, v in params.items())
    cs = ", ".join(f".{k}({v})" for k, v in {**conn, **outs}.items())
    nl.lines.append(f"// {s.name} ({s.tile}.{half}) {h.mode}")
    nl.lines.append(f"x7v_ramb18 #({ps}) u_{sig(s.name, 'RAM')} ({cs});")


# ---- behavioural Verilog --------------------------------------------------------

VERILOG_MODEL = r"""
// x7v behavioural model of one RAMB18 half, written from UG473 (not a
// Xilinx simulation model). Widths 1/2/4/9/18; RA = 36 is an SDP read on
// port A (DOB/DOPB carry the upper 18 bits), WB = 36 an SDP write on port B
// (DIB/DIPB the upper 18 bits). Write modes: 0 WRITE_FIRST, 1 READ_FIRST,
// 2 NO_CHANGE. Memory is written with nonblocking assignments, so a read on
// the other port in the same instant sees the old word.
module x7v_ramb18 #(
  parameter integer RA = 1, RB = 1, WA = 1, WB = 1, WMA = 0, WMB = 0,
  parameter integer DOA_REG = 0, DOB_REG = 0, PRIA = 0, PRIB = 0,
  parameter [17:0] INIT_A = 0, INIT_B = 0, SRVAL_A = 0, SRVAL_B = 0,
  parameter [16383:0] DATA = 0,
  parameter [2047:0] PAR = 0
)(
  input CLKA, ENA, WEA, RSTRAMA, REGCLKA, REGCEA, RSTREGA, input [13:0] ADDRA, input [15:0] DIA, input [1:0] DIPA,
  input CLKB, ENB, WEB, RSTRAMB, REGCLKB, REGCEB, RSTREGB, input [13:0] ADDRB, input [15:0] DIB, input [1:0] DIPB,
  output [15:0] DOA, output [1:0] DOPA, output [15:0] DOB, output [1:0] DOPB
);
  function integer dw(input integer w); dw = w == 36 ? 32 : w == 18 ? 16 : w == 9 ? 8 : w; endfunction
  function integer pw(input integer w); pw = w == 36 ? 4 : w == 18 ? 2 : w == 9 ? 1 : 0; endfunction
  function integer sh(input integer w); sh = w == 36 ? 5 : w == 18 ? 4 : w == 9 ? 3 : w == 4 ? 2 : w == 2 ? 1 : 0; endfunction
  reg [16383:0] mem;
  reg [2047:0] par;
  // A port word {parity[3:0], data[31:0]}; bits a width does not have are X.
  function [35:0] word(input integer w, input [31:0] d, input [3:0] p);
    integer i;
    begin
      word = {36{1'bx}};
      for (i = 0; i < dw(w); i = i + 1) word[i] = d[i];
      for (i = 0; i < pw(w); i = i + 1) word[32 + i] = p[i];
    end
  endfunction
  function [35:0] rd(input integer w, input [13:0] a);
    integer i, k;
    begin
      rd = {36{1'bx}};
      k = a >> sh(w);
      for (i = 0; i < dw(w); i = i + 1) rd[i] = mem[k * dw(w) + i];
      for (i = 0; i < pw(w); i = i + 1) rd[32 + i] = par[k * pw(w) + i];
    end
  endfunction
  // INIT/SRVAL: READ_WIDTH-wide {parity, data}; SDP 36 = port A's 18 bits low, B's high.
  function [35:0] val(input integer w, input [17:0] va, input [17:0] vb);
    val = w == 36 ? {vb[17:16], va[17:16], vb[15:0], va[15:0]} : word(w, va, va >> dw(w));
  endfunction
  wire [31:0] wdA = WB == 36 ? {DIB, DIA} : DIA;
  wire [3:0] wpA = WB == 36 ? {DIPB, DIPA} : DIPA;
  reg [35:0] la, lb, qa, qb;
  integer i, ka, kb;
  initial begin
    mem = DATA; par = PAR;
    la = val(RA, INIT_A, INIT_B); qa = la;
    lb = val(RB, INIT_B, INIT_B); qb = lb;
  end
  always @(posedge CLKA) if (ENA) begin
    if (RSTRAMA) la <= val(RA, SRVAL_A, SRVAL_B);
    else if (!(WEA && WMA == 2)) la <= (WEA && WMA == 0) ? word(WA, DIA, DIPA) : rd(RA, ADDRA);
    if (WEA) begin
      ka = ADDRA >> sh(WA);
      for (i = 0; i < dw(WA); i = i + 1) mem[ka * dw(WA) + i] <= DIA[i];
      for (i = 0; i < pw(WA); i = i + 1) par[ka * pw(WA) + i] <= DIPA[i];
    end
  end
  always @(posedge CLKB) if (ENB) begin
    // In SDP mode port B only writes; its output latch is not a read port.
    if (RA == 36 || WB == 36) ;
    else if (RSTRAMB) lb <= val(RB, SRVAL_B, SRVAL_B);
    else if (!(WEB && WMB == 2)) lb <= (WEB && WMB == 0) ? word(WB, DIB, DIPB) : rd(RB, ADDRB);
    if (WEB) begin
      kb = ADDRB >> sh(WB);
      for (i = 0; i < dw(WB); i = i + 1) mem[kb * dw(WB) + i] <= wdA[i];
      for (i = 0; i < pw(WB); i = i + 1) par[kb * pw(WB) + i] <= wpA[i];
    end
  end
  always @(posedge REGCLKA) if (RSTREGA && (PRIA == 0 || REGCEA)) qa <= val(RA, SRVAL_A, SRVAL_B); else if (REGCEA) qa <= la;
  always @(posedge REGCLKB) if (RSTREGB && (PRIB == 0 || REGCEB)) qb <= val(RB, SRVAL_B, SRVAL_B); else if (REGCEB) qb <= lb;
  wire [35:0] oa = DOA_REG ? qa : la;
  wire [35:0] ob = DOB_REG ? qb : lb;
  assign DOA = oa[15:0];
  assign DOPA = oa[33:32];
  assign DOB = RA == 36 ? oa[31:16] : ob[15:0];
  assign DOPB = RA == 36 ? oa[35:34] : ob[33:32];
endmodule
"""

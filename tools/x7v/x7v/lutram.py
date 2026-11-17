"""LUT RAM: model the SLICEM LUTs X-Ray decodes in RAM mode for co-simulation.

What the model rests on (no Helion code):

* Project X-Ray's CLBLM_L/CLBLM_R database. Per LUT of a SLICEM:
  `xLUT.RAM` (RAM mode), `xLUT.SMALL` (two 32-deep halves), `xLUT.INIT`
  (the contents at configuration), `xLUT.DI1MUX.xI` (the data comes from the
  LUT's own `xI` pin; clear: the data of the LUT above, C from D, B from C,
  A from B; the D LUT always takes `DI`). Per slice: `WA7USED`, `WA8USED`,
  `WEMUX.CE` (write enable from `CE`; clear: from `WE`), `CLKINV`.
* UG474 (7 Series CLB) for behaviour: all LUTs of a SLICEM write at the
  address on the D LUT's pins `D1..D6`, on the clock edge, while the write
  enable is 1; the read is asynchronous on each LUT's own pins `x1..x6`
  (O6 reads all six, O5 the lower half with `x6` taken as 0). A 32-deep
  (SMALL) LUT writes both halves at `D1..D5`: the O6 half from the LUT's
  `xX` pin, the O5 half from its DI1 data. `WA7` is `CX` and `WA8` is `BX`:
  with `WA7USED` the C and A LUTs take the writes with `WA7` = 1 and D and
  B the rest; with `WA8USED` the B and A LUTs take the writes with `WA8` = 1.

Validated against Vivado 2017.3 builds (xc7z020): every LUT RAM primitive
side by side (tools/svcheck/tests_hard/lutram_prims.sv, Xilinx's unisim
models as the RTL) and PicoRV32's register file (12 RAM32M); see the
README.

Reported as errors rather than guessed: SRL mode, `SMALL` or `WA7USED` /
`WA8USED` / `WEMUX.CE` in a slice with no RAM LUT, and an undriven write
address, enable, data or clock.
"""
from __future__ import annotations

LETTERS = "ABCD"


def _sig(site: str, pin: str) -> str:
    from .netlist import sig
    return sig(site, pin)


def model(x, s, nl, ram: list[str], init_of) -> None:
    """Emit the RAM LUTs `ram` of SLICEM `s`: `s__<L>` (O6) and `s__<L>_O5`."""
    F = s.features
    n = lambda pin: _sig(s.name, pin)
    L = nl.lines
    clk = x.inp(s, "CLK", "LUT RAM clock")
    nl.clock_sinks.append((s.name, clk))
    edge = "negedge" if "CLKINV" in F else "posedge"
    we = x.inp(s, "CE" if "WEMUX.CE" in F else "WE", "LUT RAM write enable")
    wa = [x.inp(s, f"D{k}", "LUT RAM write address", matters=not (k == 6 and all(f"{c}LUT.SMALL" in F for c in ram)))
          for k in range(1, 7)]
    wa7 = x.inp(s, "CX", "WA7") if "WA7USED" in F else None
    wa8 = x.inp(s, "BX", "WA8") if "WA8USED" in F else None
    # DI1 of each LUT: its own I pin, or the data of the LUT above.
    di1 = {}
    for c in "DCBA":
        if c == "D" or f"{c}LUT.DI1MUX.{c}I" in F:
            di1[c] = x.inp(s, f"{c}I", f"{c}LUT RAM data")
        else:
            di1[c] = di1["DCBA"["DCBA".index(c) - 1]]
    for c in ram:
        small = f"{c}LUT.SMALL" in F
        ra = [x.inp(s, f"{c}{k}", f"{c}LUT RAM read address", matters=not (small and k == 6)) for k in range(1, 7)]
        mem = n(c + "_MEM")
        L.append(f"reg [63:0] {mem} = 64'h{init_of(c):016x};")
        L.append(f"wire {n(c)} = {mem}[{{{','.join(reversed(ra))}}}];")
        L.append(f"wire {n(c + '_O5')} = {mem}[{{1'b0,{','.join(reversed(ra[:5]))}}}];")
        en = [we]
        if wa7 is not None:
            en.append(wa7 if c in "CA" else f"~{wa7}")
        if wa8 is not None:
            en.append(wa8 if c in "BA" else f"~{wa8}")
        cond = " & ".join(f"({e})" for e in en)
        a5 = ",".join(reversed(wa[:5]))
        if small:
            dx = x.inp(s, f"{c}X", f"{c}LUT RAM data (upper half)")
            L.append(f"always @({edge} {clk}) if ({cond}) begin {mem}[{{1'b1,{a5}}}] <= {dx}; {mem}[{{1'b0,{a5}}}] <= {di1[c]}; end")
        else:
            L.append(f"always @({edge} {clk}) if ({cond}) {mem}[{{{wa[5]},{a5}}}] <= {di1[c]};")


def check_features(F: set) -> list[str]:
    """Slice features that need a RAM LUT but the slice has none, or SRL mode."""
    bad = [f for f in F if f.endswith(".SRL") and f[1:5] == "LUT."]
    if not any(f.endswith("LUT.RAM") for f in F):
        bad += [f for f in F if f.endswith("LUT.SMALL") or f in ("WA7USED", "WA8USED", "WEMUX.CE")]
    return sorted(bad)


def ecc_exclude(db, tiles) -> dict[int, dict[int, int]]:
    """{frame: {word: mask}} of the INIT bits of every LUT in RAM mode, which
    Vivado leaves out of the frame ECC (the contents change as it runs)."""
    import re
    out: dict[int, dict[int, int]] = {}
    for t, td in tiles.items():
        rams = {(m[1], m[2]) for f in td.features for m in [re.match(r"^(SLICEM_X\d)\.([A-D])LUT\.RAM$", f)] if m}
        if not rams:
            continue
        bits = db.tilegrid[t]["bits"]["CLB_IO_CLK"]
        base, off = int(bits["baseaddr"], 16), bits["offset"]
        for name, sb in db.segbits(td.tile_type, "CLB_IO_CLK").items():
            m = re.match(r"^[A-Z_]+\.(SLICEM_X\d)\.([A-D])LUT\.INIT\[\d+\]$", name)
            if m and (m[1], m[2]) in rams:
                for b in sb:
                    w = out.setdefault(base + b.minor, {})
                    w[off + b.bit // 32] = w.get(off + b.bit // 32, 0) | 1 << (b.bit % 32)
    return out

#!/usr/bin/env python3
"""Block RAM check of x7v: positive designs must pass verify, edited ones must fail.

Helion cannot build a block RAM bitstream yet, and there is no Vivado one,
so this test makes its own: zedbit builds a small base design (clock on a
BUFG, eight switches and a button in, eight LEDs out), then this script adds
one RAMB18 half to its decoded features, routes the RAM's pins over the
X-Ray routing graph (clock from the base's clock tree, address/data/write
enable from the switch and button nets, constants from INT-tile GND_WIRE,
LEDs re-driven from the RAM's DOADO pins), encodes the frames with x7v's
encoder and rewrites the bitstream (ECC and CRC recomputed). The RTL of
each design is a plain behavioural Verilog memory written from UG473, not
from x7v's model, and `verify` co-simulates it against the bitstream.

Designs (all must PASS): rom18 (x18 ROM, non-zero INIT_A), ram18_rf (x18
READ_FIRST writes), ram9_wf_reg (x9 WRITE_FIRST with DOA_REG), sdp36 (SDP
36-bit read and write ports). Negative controls on rom18 (all must FAIL in
cosim): one INIT bit flipped, READ_WIDTH_A_18 cleared.

    python3 tests/bram.py [--work DIR]     # from tools/x7v; needs a release zedbit
"""
from __future__ import annotations

import argparse
import os
import random
import re
import subprocess
import sys
import tempfile
from collections import defaultdict, deque

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))
from x7v import bit, clockcheck, decode, encode, netlist, verify  # noqa: E402
from x7v.db import Db  # noqa: E402

REPO = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
ZEDBIT = os.environ.get("X7V_ZEDBIT", os.path.join(REPO, "target", "release", "examples", "zedbit"))
BASE_SV = """module base(input wire clk, input wire [7:0] sw, input wire we, output reg [7:0] led);
  always @(posedge clk) led <= sw ^ {8{we}};
endmodule
"""
PINS = dict(clk="Y9", we="P16", **{f"sw[{i}]": p for i, p in enumerate("F22 G22 H22 F21 H19 H18 H17 M15".split())},
            **{f"led[{i}]": p for i, p in enumerate("T22 T21 U22 U21 V22 W22 U19 U14".split())})


def xdc() -> str:
    out = []
    for port, pin in PINS.items():
        std = "LVCMOS18" if port.startswith(("sw", "we")) else "LVCMOS33"
        out.append(f"set_property -dict {{ PACKAGE_PIN {pin} IOSTANDARD {std} }} [get_ports {{ {port} }}]")
    out.append("create_clock -period 10.000 -name sys_clk [get_ports { clk }]")
    return "\n".join(out) + "\n"


class Composer:
    """Adds features and routes to a decoded bitstream."""

    def __init__(self, db: Db, bitfile: str):
        self.db, self.bitfile = db, bitfile
        bs = bit.parse(bitfile, db.part)
        self.frames = list(bs.frames)
        self.ex = netlist.Extractor(db, decode.decode(db, bs.frames))
        self.fab, self.tg = self.ex.fab, db.tilegrid
        self.active = self.ex.active
        self._fwd: dict[str, dict] = {}
        self.used: set[int] = set()
        seeds = []
        for t, fs in self.active.items():
            tt = self.fab.tt(self.tg[t]["type"])
            for f in fs:
                p = f.split(".")
                if len(p) == 2 and (p[1], p[0]) in tt.pip_pairs:
                    seeds += [self.fab.node(t, p[0]), self.fab.node(t, p[1])]
        for s in seeds:
            if s not in self.used:
                self.used |= self.tree(s)

    def fwd(self, ttype: str) -> dict:
        if ttype not in self._fwd:
            tt, seg, out = self.fab.tt(ttype), self.db.segbits(ttype), defaultdict(list)
            for dst, srcs in tt.pips_into.items():
                for src, bid in srcs:
                    feat = f"{dst}.{src}"
                    kind = tt.ppips.get(feat)
                    if kind and kind != "always":
                        continue
                    # Bidirectional pips (clock spine rebuffers) are followed
                    # along existing nets only, never used for new routes.
                    k = "bid" if bid else "always" if kind else "zero" if feat in tt.zero_pips else "pip" if f"{ttype}.{feat}" in seg else None
                    if k:
                        out[src].append((dst, k))
            self._fwd[ttype] = out
        return self._fwd[ttype]

    def tree(self, root: int) -> set[int]:
        """Nodes a net reaches from `root` through active pips and always-on connections."""
        seen, q = {root}, deque([root])
        while q:
            n = q.popleft()
            for t, w in self.fab.nodes[n]:
                act = self.active.get(t, ())
                for dst, k in self.fwd(self.tg[t]["type"]).get(w, ()):
                    if k == "always" or f"{dst}.{w}" in act or (k == "bid" and f"{w}.{dst}" in act):
                        m = self.fab.node(t, dst)
                        if m not in seen:
                            seen.add(m)
                            q.append(m)
        return seen

    def free(self, n: int) -> bool:
        if n in self.used:
            return False
        for t, w in self.fab.nodes[n]:
            tt = self.fab.tt(self.tg[t]["type"])
            if w in tt.site_pin_wire:
                return False
            for src, _ in tt.pips_into.get(w, ()):
                f = f"{w}.{src}"
                if f in self.active.get(t, ()) or (tt.ppips.get(f) == "always" and self.fab.node(t, src) in self.used):
                    return False
        return True

    def route(self, sources: set[int], target: int, radius: int = 6) -> None:
        # Search box: the target and the source nearest it, plus a margin.
        gx = lambda n: (self.tg[self.fab.nodes[n][0][0]]["grid_x"], self.tg[self.fab.nodes[n][0][0]]["grid_y"])
        tx, ty = gx(target)
        sx, sy = min((gx(n) for n in sources), key=lambda p: abs(p[0] - tx) + abs(p[1] - ty))
        x0, x1, y0, y1 = min(tx, sx) - radius, max(tx, sx) + radius, min(ty, sy) - radius, max(ty, sy) + radius
        prev = {s: None for s in sources}
        q = deque(sources)
        while q:
            n = q.popleft()
            for t, w in self.fab.nodes[n]:
                g = self.tg[t]
                if not (x0 <= g["grid_x"] <= x1 and y0 <= g["grid_y"] <= y1):
                    continue
                for dst, k in self.fwd(g["type"]).get(w, ()):
                    if k == "bid":
                        continue
                    m = self.fab.node(t, dst)
                    if m in prev or (m != target and not self.free(m)):
                        continue
                    prev[m] = (n, t, dst, w, k)
                    if m == target:
                        while prev[m] is not None:
                            m, t2, d2, s2, k2 = prev[m]
                            if k2 == "pip":
                                self.active.setdefault(t2, set()).add(f"{d2}.{s2}")
                            self.used.add(self.fab.node(t2, d2))
                        self.used |= self.tree(target)
                        return
                    q.append(m)
        if radius < 24:
            return self.route(sources, target, radius * 2)
        raise RuntimeError(f"no route to {self.fab.nodes[target][:2]}")

    def node(self, tile: str, wire: str) -> int:
        return self.fab.node(tile, wire)

    def site_pin(self, site: str, pin: str) -> int:
        s = self.ex.sites[site]
        return self.node(s.tile, self.fab.tt(self.tg[s.tile]["type"]).sites[s.index]["site_pins"][pin]["wire"])

    def pad_net(self, port: str) -> set[int]:
        """The net an input pad drives (from its ILOGIC's O)."""
        site = self.db.package_pins[PINS[port]]["site"].replace("IOB", "ILOGIC")
        return self.tree(self.site_pin(site, "O"))

    def cut_led(self, port: str) -> int:
        """Disconnect an output pad from its driver; returns the node to drive."""
        n = self.site_pin(self.db.package_pins[PINS[port]]["site"].replace("IOB", "OLOGIC"), "D1")
        for _ in range(16):
            up = None
            for t, w in self.fab.nodes[n]:
                tt = self.fab.tt(self.tg[t]["type"])
                for src, _ in tt.pips_into.get(w, ()):
                    f = f"{w}.{src}"
                    if f in self.active.get(t, ()):
                        self.active[t].discard(f)
                        return n
                    if tt.ppips.get(f) == "always":
                        up = self.node(t, src)
            if up is None:
                break
            n = up
        raise RuntimeError(f"{port}: no driver found")

    def clock_net(self) -> set[int]:
        s = next(s for s in self.ex.sites.values() if s.stype == "BUFGCTRL" and "IN_USE" in s.features)
        return self.tree(self.site_pin(s.name, "O"))

    def gnd(self, near: int, radius: int = 6) -> set[int]:
        g0 = self.tg[self.fab.nodes[near][0][0]]
        out = set()
        for t, g in self.tg.items():
            if g["type"] in ("INT_L", "INT_R") and abs(g["grid_x"] - g0["grid_x"]) <= radius and abs(g["grid_y"] - g0["grid_y"]) <= radius:
                out |= self.tree(self.node(t, "GND_WIRE"))
        return out

    def features(self) -> list[str]:
        return [f"{t}.{f}" for t, fs in self.active.items() for f in fs]

    def fix_clocks(self) -> None:
        """Add the leaf-buffer enables a new clock branch needs (clockcheck R4)."""
        dec = decode.DesignDecode({t: decode.TileDecode(t, self.tg[t]["type"], sorted(fs)) for t, fs in self.active.items()})
        for e in clockcheck.check(self.db, dec).errors:
            m = re.match(r"^R4 (\S+): .*lacks (\S+)$", e)
            if not m:
                raise RuntimeError(f"clock rule broken by the added route: {e}")
            self.active.setdefault(m.group(1), set()).add(m.group(2))

    def write(self, out: str) -> list[str]:
        return write(self.db, self.bitfile, out, self.features())


def write(db: Db, base: str, out: str, features: list[str]) -> list[str]:
    frames = encode.encode(db, features, list(bit.parse(base, db.part).frames))
    bit.rewrite(base, out, frames, db.part)
    return features


def pick_bram(c: Composer) -> str:
    """The BRAM tile nearest the base design's flops, in their clock region
    (so the new clock branch only needs a leaf buffer, not a new HROW tap)."""
    tiles = [s.tile for s in c.ex.sites.values() if s.stype.startswith("SLICE") and s.features]
    cx = sum(c.tg[t]["grid_x"] for t in tiles) / len(tiles)
    cy = sum(c.tg[t]["grid_y"] for t in tiles) / len(tiles)
    region = c.tg[tiles[0]]["clock_region"]
    return min((t for t, g in c.tg.items() if g["type"] in ("BRAM_L", "BRAM_R") and g["clock_region"] == region),
               key=lambda t: abs(c.tg[t]["grid_x"] - cx) + abs(c.tg[t]["grid_y"] - cy))


def vec(name: str, value: int, n: int) -> list[str]:
    return [f"{name}[{i}]" for i in range(n) if value >> i & 1]


def build(db: Db, base: str, out: str, cfg: dict) -> list[str]:
    """Compose one RAM design: returns its feature list (for negative edits)."""
    c = Composer(db, base)
    tile = pick_bram(c)
    half = cfg["half"]
    wp = "BRAM_FIFO18_" if half == "RAMB18_Y0" else "BRAM_RAMB18_"
    pin = lambda p: c.node(tile, wp + p)
    F = {"IN_USE", *cfg["features"], *(f"ZINV_{p}" for p in cfg["zinv"])}
    F |= set(vec("ZINIT_A", ~cfg.get("init_a", 0), 18)) | set(vec("ZINIT_B", ~cfg.get("init_b", 0), 18))
    F |= set(vec("ZSRVAL_A", ~0, 18)) | set(vec("ZSRVAL_B", ~0, 18))
    F |= {f"INIT_{i // 256:02X}[{i % 256:03d}]" for i in range(16384) if cfg["data"] >> i & 1}
    F |= {f"INITP_{i // 256:02X}[{i % 256:03d}]" for i in range(2048) if cfg["parity"] >> i & 1}
    c.active.setdefault(tile, set()).update(f"{half}.{f}" for f in F)
    clk = c.clock_net()
    for p in cfg["clocks"]:
        c.route(clk, pin(p))
    for p, port in cfg["nets"]:
        c.route(c.pad_net(port), pin(p))
    for p in cfg["gnd"]:
        c.route(c.gnd(pin(p)), pin(p))
    for p, port in cfg["leds"]:
        c.route({pin(p)} | c.tree(pin(p)), c.cut_led(port))
    c.fix_clocks()
    return c.write(out)


def rand_bits(rng, n):
    return rng.getrandbits(n)


def configs(rng) -> dict[str, dict]:
    data, par = rand_bits(rng, 16384), rand_bits(rng, 2048)
    word = lambda w, a: data >> (w * a) & ((1 << w) - 1)
    led = [f"led[{i}]" for i in range(8)]
    sw = [f"sw[{i}]" for i in range(8)]
    mem_init = lambda w, n: "".join(f"    mem[{a}] = {w}'h{word(w, a):x};\n" for a in range(n))
    out = {}
    # x18 ROM on the upper half, power-up output INIT_A.
    init_a = 0x2A5C3
    out["rom18"] = dict(
        half="RAMB18_Y1", data=data, parity=par, init_a=init_a,
        features=["READ_WIDTH_A_18", "WRITE_WIDTH_A_18"], zinv=["CLKARDCLK", "ENARDEN", "RSTRAMARSTRAM"],
        clocks=["CLKARDCLK"], nets=[(f"ADDRARDADDR{4 + i}", sw[i]) for i in range(4)],
        gnd=[f"ADDRARDADDR{i}" for i in range(8, 14)] + [f"WEA{i}" for i in range(4)] + ["RSTRAMARSTRAM"],
        leds=[(f"DOADO{i}", led[i]) for i in range(8)],
        rtl=f"""module rom18(input wire clk, input wire [7:0] sw, input wire we, output reg [7:0] led);
  reg [15:0] mem [0:15];
  initial begin
{mem_init(16, 16)}    led = 8'h{init_a & 0xFF:02x};
  end
  always @(posedge clk) led <= mem[sw[3:0]][7:0];
endmodule
""")
    # x18 READ_FIRST with writes on the lower half.
    out["ram18_rf"] = dict(
        half="RAMB18_Y0", data=data, parity=par,
        features=["READ_WIDTH_A_18", "WRITE_WIDTH_A_18", "WRITE_MODE_A_READ_FIRST"], zinv=["CLKARDCLK", "ENARDEN", "RSTRAMARSTRAM"],
        clocks=["CLKARDCLK"],
        nets=[(f"ADDRARDADDR{4 + i}", sw[i]) for i in range(4)] + [(f"WEA{i}", "we") for i in range(4)]
        + [(f"DIADI{i}", sw[4 + i % 4]) for i in range(8)],
        gnd=[f"ADDRARDADDR{i}" for i in range(8, 14)] + ["RSTRAMARSTRAM"],
        leds=[(f"DOADO{i}", led[i]) for i in range(8)],
        rtl=f"""module ram18_rf(input wire clk, input wire [7:0] sw, input wire we, output reg [7:0] led);
  reg [15:0] mem [0:15];
  initial begin
{mem_init(16, 16)}    led = 8'h00;
  end
  always @(posedge clk) begin
    led <= mem[sw[3:0]][7:0];
    if (we) mem[sw[3:0]] <= {{8'hff, sw[7:4], sw[7:4]}};
  end
endmodule
""")
    # x9 WRITE_FIRST with the output register, upper half.
    out["ram9_wf_reg"] = dict(
        half="RAMB18_Y1", data=data, parity=par,
        features=["READ_WIDTH_A_9", "WRITE_WIDTH_A_9", "DOA_REG"], zinv=["CLKARDCLK", "ENARDEN", "REGCLKARDRCLK", "RSTRAMARSTRAM", "RSTREGARSTREG"],
        clocks=["CLKARDCLK", "REGCLKARDRCLK"],
        nets=[(f"ADDRARDADDR{3 + i}", sw[i]) for i in range(4)] + [(f"WEA{i}", "we") for i in range(4)]
        + [(f"DIADI{i}", sw[(i + 4) % 8]) for i in range(8)],
        gnd=[f"ADDRARDADDR{i}" for i in range(7, 14)] + ["DIPADIP0", "RSTRAMARSTRAM", "RSTREGARSTREG"],
        leds=[(f"DOADO{i}", led[i]) for i in range(8)],
        rtl=f"""module ram9_wf_reg(input wire clk, input wire [7:0] sw, input wire we, output reg [7:0] led);
  reg [7:0] mem [0:15];
  reg [7:0] lat;
  initial begin
{mem_init(8, 16)}    lat = 8'h00; led = 8'h00;
  end
  always @(posedge clk) begin
    if (we) begin mem[sw[3:0]] <= {{sw[3:0], sw[7:4]}}; lat <= {{sw[3:0], sw[7:4]}}; end
    else lat <= mem[sw[3:0]];
    led <= lat;
  end
endmodule
""")
    # SDP: 36-bit read port A (address sw[3:0]), 36-bit write port B (address sw[7:4]).
    init_a, init_b = 0x0000A, 0x00005
    out["sdp36"] = dict(
        half="RAMB18_Y0", data=data, parity=par, init_a=init_a, init_b=init_b,
        features=["SDP_READ_WIDTH_36", "SDP_WRITE_WIDTH_36"],
        zinv=["CLKARDCLK", "CLKBWRCLK", "ENARDEN", "ENBWREN", "RSTRAMARSTRAM", "RSTRAMB"],
        clocks=["CLKARDCLK", "CLKBWRCLK"],
        nets=[(f"ADDRARDADDR{5 + i}", sw[i]) for i in range(4)] + [(f"ADDRBWRADDR{5 + i}", sw[4 + i]) for i in range(4)]
        + [(f"WEBWE{i}", "we") for i in range(8)] + [(f"DIADI{i}", sw[4 + i]) for i in range(4)]
        + [(f"DIBDI{i}", sw[i]) for i in range(4)],
        gnd=[f"ADDRARDADDR{i}" for i in range(9, 14)] + [f"ADDRBWRADDR{i}" for i in range(9, 14)] + ["RSTRAMARSTRAM", "RSTRAMB"],
        leds=[(f"DOADO{i}", led[i]) for i in range(4)] + [(f"DOBDO{i}", led[4 + i]) for i in range(4)],
        rtl=f"""module sdp36(input wire clk, input wire [7:0] sw, input wire we, output reg [7:0] led);
  reg [31:0] mem [0:15];
  reg [31:0] q;
  initial begin
{mem_init(32, 16)}    led = 8'h{(init_b & 0xF) << 4 | (init_a & 0xF):02x};
  end
  always @(posedge clk) begin
    q = mem[sw[3:0]];
    led <= {{q[19:16], q[3:0]}};
    if (we) mem[sw[7:4]] <= {{12'hfff, sw[3:0], 12'hfff, sw[7:4]}};
  end
endmodule
""")
    return out


def check(db: Db, work: str, name: str, bitfile: str, rtl: str, top: str, expect: bool, cycles: int) -> bool:
    sv = os.path.join(work, f"{name}.sv")
    open(sv, "w").write(rtl)
    rep = verify.verify(db, name, bitfile, rtl=[sv], top=top, xdc=os.path.join(work, "pins.xdc"),
                        workdir=os.path.join(work, f"x7v_{name}"), cycles=cycles)
    ok = rep.ok == expect
    if not expect:
        # A negative control must be caught by co-simulation, not by an
        # unrelated check.
        ok = ok and [c.name for c in rep.checks if not c.ok] == ["cosim"]
    print(rep.summary())
    print(f"==> {name}: {'as expected' if ok else 'UNEXPECTED'} (wanted {'PASS' if expect else 'FAIL'})\n")
    return ok


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--work", default=None)
    ap.add_argument("--cycles", type=int, default=3000)
    a = ap.parse_args()
    work = a.work or tempfile.mkdtemp(prefix="x7v_bram_")
    os.makedirs(work, exist_ok=True)
    db = Db()
    open(os.path.join(work, "pins.xdc"), "w").write(xdc())
    base = os.path.join(work, "base.bit")
    if not os.path.exists(base):
        open(os.path.join(work, "base.sv"), "w").write(BASE_SV)
        subprocess.run([ZEDBIT, "base.sv", "pins.xdc", "base.bit", "base"], cwd=work, check=True,
                       env={**os.environ, "HELION_ZED_FRONT": "v2"})
    results = []
    cfgs = configs(random.Random(7))
    for name, cfg in cfgs.items():
        out = os.path.join(work, f"{name}.bit")
        feats = build(db, base, out, cfg)
        results.append(check(db, work, name, out, cfg["rtl"], name, True, a.cycles))
        if name == "rom18":
            # Negative controls: one INIT bit of a word the ROM reads, and the read width.
            pre = next(f.split(".RAMB18_Y1.")[0] for f in feats if ".RAMB18_Y1.IN_USE" in f) + ".RAMB18_Y1."
            flip = f"{pre}INIT_00[{5 * 16 + 2:03d}]"
            init_flip = [f for f in feats if f != flip] + ([] if flip in feats else [flip])
            write(db, base, os.path.join(work, "rom18_init_flip.bit"), init_flip)
            results.append(check(db, work, "rom18_init_flip", os.path.join(work, "rom18_init_flip.bit"),
                                 cfg["rtl"].replace("module rom18", "module rom18_init_flip"), "rom18_init_flip", False, a.cycles))
            width = [f for f in feats if f != pre + "READ_WIDTH_A_18"]
            write(db, base, os.path.join(work, "rom18_width.bit"), width)
            results.append(check(db, work, "rom18_width", os.path.join(work, "rom18_width.bit"),
                                 cfg["rtl"].replace("module rom18", "module rom18_width"), "rom18_width", False, a.cycles))
    print(f"bram: {sum(results)}/{len(results)} as expected ({work})")
    return 0 if all(results) else 1


if __name__ == "__main__":
    sys.exit(main())

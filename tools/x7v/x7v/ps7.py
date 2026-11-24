"""Zynq PS7: the processing system as the edge of the design.

The PS7 site holds no PL configuration bits (the PSS tiles' pips into the
interconnect are bitless always-on pseudo-pips), so x7v does not model what
the PS does: it cuts the design there. The PS7's fabric-facing outputs
(FCLK, FCLKRESETN, the AXI masters' signals, ...) become inputs of the
extracted netlist and its inputs (AXI slaves' responses, interrupts, ...)
become outputs, each named PS7_<site pin>. Ports and directions are the
database's ps7_ports.json, normal class: the pins with fabric wires (MIO,
DDR and PS_* are the PS's own package pins, configured by the boot code;
test and debug pins are not on the cell).

Cosim gives the RTL a stub PS7 with the same ports: its outputs are
registers the testbench sets, its inputs are sampled, so both simulations
see one stimulus and every PS7 input is compared every cycle. Each M_AXI_GP
port the RTL connects is driven by an AXI3 master (`axi_tb`) that runs
against the RTL once and records what it drove; that record is the
stimulus both simulations then replay.
"""
from __future__ import annotations

import json
import os
import random
import re
import subprocess
from dataclasses import dataclass, field
from functools import lru_cache


@lru_cache(maxsize=None)
def _ports(root: str) -> tuple:
    path = os.path.join(root, "ps7_ports.json")
    if not os.path.exists(path):
        return ()
    return tuple((k, v["width"], v["direction"]) for k, v in sorted(json.load(open(path)).items()) if v["class"] == "normal")


def ports(db) -> list[tuple[str, int, str]]:
    """(port, width, input|output) of every normal-class PS7 port; [] off Zynq."""
    return list(_ports(db.root))


def site_pin(port: str, k: int, width: int) -> str:
    return port if width == 1 else f"{port}{k}"


# ---- extraction --------------------------------------------------------------

def site(x):
    """The device's PS7 site (an Extractor Site), or None."""
    for tile, t in x.db.tilegrid.items():
        if "PS7" in t.get("sites", {}).values():
            tt = x.fab.tt(t["type"])
            return x.site_at(tile, next(i for i, s in enumerate(tt.sites) if s["type"] == "PS7"))
    return None


def driven(x, s) -> bool:
    """Does fabric logic drive any PS7 input? (A design may only feed the PS.)"""
    return any(x.pin_driver(s, site_pin(p, k, w))[0] == "site"
               for p, w, d in ports(x.db) if d == "input" for k in range(w))


def model_ps7(x, s, nl):
    from .netlist import sig
    nl.lines.append(f"// {s.name} (PS7): the design boundary; its pins are the ports PS7_<pin>")
    floating = []
    for p, w, d in ports(x.db):
        for k in range(w):
            pin = site_pin(p, k, w)
            if d == "output":
                nl.ports_in[f"PS7_{pin}"] = sig(s.name, pin)
                continue
            r = x.pin_driver(s, pin)
            # An input with no driver at all (the unused ports' clocks, left
            # unrouted) floats: z, not X, so cosim can tell it from bad logic.
            if r[0] not in ("site", "const"):
                floating.append(pin)
            nl.ports_out[f"PS7_{pin}"] = x.expr(r) if r[0] in ("site", "const") else "1'bz"
    if floating:
        x.warnings.append(f"{s.name}: {len(floating)} inputs have no driver: {floating[:8]}")


# ---- cosim boundary ------------------------------------------------------------

@dataclass
class Boundary:
    inst: str                       # instance name of PS7 in the RTL's top module
    out_bits: list[str]             # PS7 outputs ("ps7.FCLKCLK[0]"): stimulus
    in_bits: list[str]              # PS7 inputs: compared every cycle
    pin_of: dict[str, str]          # bit -> netlist port PS7_<pin>
    masters: list[str]              # M_AXI_GP ports the RTL connects ("MAXIGP0")
    resets: list[str] = field(default_factory=list)   # FCLKRESETN bits (active low)


def boundary(rtl_files: list[str], top: str, plist: list) -> Boundary | None:
    """The PS7 the RTL's top module instantiates, or None (no PS7, or the
    RTL brings its own PS7 model)."""
    text = "\n".join(re.sub(r"//[^\n]*|/\*.*?\*/", "", open(f).read(), flags=re.S) for f in rtl_files)
    if not plist or not re.search(r"\bPS7\b", text) or re.search(r"\bmodule\s+PS7\b", text):
        return None
    body = re.search(rf"\bmodule\s+{top}\b(.*?)\bendmodule\b", text, re.S)
    m = body and re.search(r"\bPS7\s*(?:#\s*\(.*?\)\s*)?(\w+)\s*\(", body.group(1), re.S)
    if not m:
        raise ValueError("x7v cosim: PS7 must be instantiated in the top module")
    # The instance's connection list: up to its balanced closing parenthesis.
    depth, i = 1, m.end()
    while depth:
        depth += {"(": 1, ")": -1}.get(body.group(1)[i], 0)
        i += 1
    conns = body.group(1)[m.end():i]
    inst = m.group(1)
    b = Boundary(inst, [], [], {}, [n for n in ("MAXIGP0", "MAXIGP1")
                                    if re.search(rf"\.{n}(AW|AR)VALID\s*\(\s*[^\s)]", conns)])
    for p, w, d in plist:
        for k in range(w):
            name = f"{inst}.{p}" if w == 1 else f"{inst}.{p}[{k}]"
            b.pin_of[name] = f"PS7_{site_pin(p, k, w)}"
            (b.out_bits if d == "output" else b.in_bits).append(name)
            if p == "FCLKRESETN":
                b.resets.append(name)
    return b


def stub(plist: list) -> str:
    decl = []
    for p, w, d in plist:
        rng = "" if w == 1 else f"[{w - 1}:0] "
        decl.append(f"  output reg {rng}{p} = 0" if d == "output" else f"  input wire {rng}{p}")
    return ("// x7v: the PS7 as a boundary (outputs set by the testbench, inputs sampled)\n"
            "`timescale 1ns/1ps\nmodule PS7(\n" + ",\n".join(decl) + "\n);\nendmodule\n")


# ---- AXI3 master ------------------------------------------------------------------

# One master per M_AXI_GP port, written as per-cycle procedural code in the
# testbench (@v: variable prefix, @d: the port's path in the RTL, @P: the
# port, @n: address pool size). Writes and reads run concurrently, up to 4
# outstanding each, with mostly zero idle cycles (back to back), random
# AW/W/AR delays (W often before AW), random BREADY/RREADY, partial strobes
# and 2-4 beat bursts. Half the OKAY single-beat writes to a register named
# in --axi-rw are read back next, with no write issued until that read
# returns; a read is checked against what was written only when no write
# was in flight from its address handshake to its last beat.
#
# Protocol checks: a BID/RID must match an outstanding transaction, RLAST
# must mark the last beat, and BVALID/RVALID once raised must hold with the
# same payload until BREADY/RREADY (a response the slave overwrites or
# drops is lost on silicon). FCLK_RESET does not reset the PS interconnect,
# so a reset never forgives a transaction: when the random stimulus asks
# for one, the masters start nothing new and FCLKRESETN stays high until
# every transaction has completed (software quiescing the port), and a
# response never sent keeps it high to the end. The masters stay idle until
# the stimulus's first reset has been applied (if it has one: none with
# --no-reset), so a busy master never holds off the start-up pulse that a
# slave with no initial values needs, while FCLKRESETN is low and for
# RESET_QUIET cycles after (the slave's reset synchroniser), and start
# nothing in the last DRAIN cycles, with BREADY/RREADY high: a write or read
# still outstanding at the end never completes.
RESET_QUIET = 4
_DECL = """
// ---- AXI3 master on @P
reg @v_awvalid = 0, @v_wvalid = 0, @v_wlast = 0, @v_bready = 0, @v_arvalid = 0, @v_rready = 0;
reg [31:0] @v_awaddr = 0, @v_araddr = 0, @v_wdata = 0, @v_d0 = 0;
reg [11:0] @v_awid = 0, @v_arid = 0;
reg [3:0] @v_awlen = 0, @v_arlen = 0, @v_wstrb = 0, @v_s0 = 0;
reg @v_won = 0, @v_awpend = 0, @v_aron = 0, @v_holdw = 0, @v_rbnext = 0, @v_arrb = 0;
integer @v_wbeat = 0, @v_awdly = 0, @v_wdly = 0, @v_wpool = 0, @v_ardly = 0, @v_arpool = 0, @v_rbpool = 0;
reg [11:0] @v_obid [0:3]; integer @v_obpool [0:3]; integer @v_oblen [0:3]; reg [31:0] @v_obdata [0:3]; reg [3:0] @v_obstrb [0:3];
reg [11:0] @v_orid [0:3]; integer @v_orpool [0:3]; integer @v_orlen [0:3]; integer @v_orbeat [0:3]; integer @v_orbc [0:3];
reg @v_orchk [0:3]; reg @v_orrb [0:3];
integer @v_obn = 0, @v_orn = 0, @v_bc = 0;
reg [31:0] @v_pool [0:@n-1]; reg [31:0] @v_mask [0:@n-1]; reg [31:0] @v_shadow [0:@n-1]; reg [31:0] @v_known [0:@n-1]; integer @v_nrb [0:@n-1];
reg @v_s_awready, @v_s_wready, @v_s_bvalid, @v_s_arready, @v_s_rvalid, @v_s_rlast;
reg [11:0] @v_s_bid, @v_s_rid; reg [1:0] @v_s_bresp, @v_s_rresp; reg [31:0] @v_s_rdata;
integer @v_n_aw = 0, @v_n_w = 0, @v_n_bok = 0, @v_n_berr = 0, @v_n_ar = 0, @v_n_rok = 0, @v_n_rerr = 0;
integer @v_n_part = 0, @v_n_burst = 0, @v_n_b2b = 0, @v_n_rb = 0, @v_n_rbbad = 0, @v_n_perr = 0;
integer @v_last_aw = -9, @v_last_ar = -9, @v_awfirst = -1, @v_arfirst = -1;
reg @v_hb = 0, @v_hr = 0, @v_hrlast = 0; reg [11:0] @v_hbid = 0, @v_hrid = 0; reg [1:0] @v_hbresp = 0, @v_hrresp = 0; reg [31:0] @v_hrdata = 0;
"""

_DECIDE = """
    if (!act) begin
      @v_won = 0; @v_aron = 0; @v_obn = 0; @v_orn = 0; @v_holdw = 0; @v_rbnext = 0; @v_hb = 0; @v_hr = 0;
      for (i = 0; i < @n; i = i + 1) @v_known[i] = 0;
    end else if (!stop) begin
      if (!@v_won && !@v_holdw && @v_obn < 4 && rnd(8) != 0) begin
        @v_won = 1; @v_awpend = 1; @v_wbeat = 0;
        @v_wpool = @pick; @v_awaddr = @v_pool[@v_wpool]; @v_awid = $random(seed); @v_awfirst = -1;
        @v_awlen = rnd(8) == 0 ? 1 + rnd(3) : 0;
        @v_awdly = rnd(4) == 0 ? 1 + rnd(3) : 0;
        @v_wdly = rnd(4) == 0 ? 1 + rnd(3) : 0;
        @v_wdata = $random(seed); @v_wstrb = strb(0);
      end
      if (!@v_aron && @v_orn < 4 && rnd(8) != 0 && (!@v_rbnext || (!@v_won && @v_obn == 0))) begin
        @v_aron = 1; @v_arrb = @v_rbnext; @v_rbnext = 0;
        @v_arpool = @v_arrb ? @v_rbpool : @pick; @v_araddr = @v_pool[@v_arpool]; @v_arid = $random(seed); @v_arfirst = -1;
        @v_arlen = (!@v_arrb && rnd(8) == 0) ? 1 + rnd(3) : 0;
        @v_ardly = rnd(4) == 0 ? 1 + rnd(3) : 0;
      end
    end
    @v_awvalid = act && @v_won && @v_awpend && @v_awdly == 0;
    @v_wvalid = act && @v_won && @v_wbeat <= @v_awlen && @v_wdly == 0;
    @v_wlast = @v_wbeat == @v_awlen;
    @v_arvalid = act && @v_aron && @v_ardly == 0;
    if (@v_awvalid && @v_awfirst < 0) @v_awfirst = k;
    if (@v_arvalid && @v_arfirst < 0) @v_arfirst = k;
    @v_bready = act && (stop || rnd(4) != 0); @v_rready = act && (stop || rnd(4) != 0);
"""

_SAMPLE = """
    @v_s_awready = @dAWREADY === 1'b1; @v_s_wready = @dWREADY === 1'b1; @v_s_arready = @dARREADY === 1'b1;
    @v_s_bvalid = @dBVALID === 1'b1; @v_s_rvalid = @dRVALID === 1'b1; @v_s_rlast = @dRLAST === 1'b1;
    @v_s_bid = @dBID; @v_s_rid = @dRID; @v_s_bresp = @dBRESP; @v_s_rresp = @dRRESP; @v_s_rdata = @dRDATA;
"""

_UPDATE = """
    if (act) begin
      if (@v_hb && (!@v_s_bvalid || @v_s_bid !== @v_hbid || @v_s_bresp !== @v_hbresp)) begin
        @v_n_perr = @v_n_perr + 1;
        $fdisplay(fl, "error cycle %0d: @P BVALID dropped or B changed before BREADY (BID %h)", k, @v_hbid);
      end
      if (@v_hr && (!@v_s_rvalid || @v_s_rid !== @v_hrid || @v_s_rresp !== @v_hrresp || @v_s_rdata !== @v_hrdata || @v_s_rlast !== @v_hrlast)) begin
        @v_n_perr = @v_n_perr + 1;
        $fdisplay(fl, "error cycle %0d: @P RVALID dropped or R changed before RREADY (RID %h)", k, @v_hrid);
      end
      @v_hb = @v_s_bvalid && !@v_bready; @v_hbid = @v_s_bid; @v_hbresp = @v_s_bresp;
      @v_hr = @v_s_rvalid && !@v_rready; @v_hrid = @v_s_rid; @v_hrresp = @v_s_rresp; @v_hrdata = @v_s_rdata; @v_hrlast = @v_s_rlast;
      if (@v_awvalid && @v_s_awready) begin
        @v_awpend = 0; @v_n_aw = @v_n_aw + 1;
        if (@v_awlen != 0) @v_n_burst = @v_n_burst + 1;
        // Back to back: this AW was raised the cycle after the previous one was taken.
        if (@v_awfirst == @v_last_aw + 1) @v_n_b2b = @v_n_b2b + 1;
        @v_last_aw = k;
      end else if (@v_won && @v_awpend && @v_awdly > 0) @v_awdly = @v_awdly - 1;
      if (@v_wvalid && @v_s_wready) begin
        if (@v_wbeat == 0) begin @v_d0 = @v_wdata; @v_s0 = @v_wstrb; end
        @v_wbeat = @v_wbeat + 1; @v_n_w = @v_n_w + 1;
        @v_wdata = $random(seed); @v_wstrb = strb(0);
      end else if (@v_won && @v_wdly > 0) @v_wdly = @v_wdly - 1;
      if (@v_won && !@v_awpend && @v_wbeat > @v_awlen) begin
        @v_obid[@v_obn] = @v_awid; @v_obpool[@v_obn] = @v_wpool; @v_oblen[@v_obn] = @v_awlen;
        @v_obdata[@v_obn] = @v_d0; @v_obstrb[@v_obn] = @v_s0; @v_obn = @v_obn + 1; @v_won = 0;
      end
      if (@v_bready && @v_s_bvalid) begin
        j = -1;
        for (i = @v_obn - 1; i >= 0; i = i - 1) if (@v_obid[i] == @v_s_bid) j = i;
        if (j < 0) begin
          @v_n_perr = @v_n_perr + 1;
          $fdisplay(fl, "error cycle %0d: @P BID %h matches no outstanding write", k, @v_s_bid);
        end else begin
          p = @v_obpool[j];
          if (@v_s_bresp == 2'b00) begin
            @v_n_bok = @v_n_bok + 1;
            if (@v_oblen[j] == 0 && @v_obstrb[j] != 4'hF) @v_n_part = @v_n_part + 1;
            if (@v_oblen[j] == 0 && @v_mask[p] != 0) begin
              m = {{8{@v_obstrb[j][3]}}, {8{@v_obstrb[j][2]}}, {8{@v_obstrb[j][1]}}, {8{@v_obstrb[j][0]}}};
              @v_shadow[p] = (@v_shadow[p] & ~m) | (@v_obdata[j] & m);
              @v_known[p] = @v_known[p] | m;
              if (rnd(2) == 0) begin @v_rbnext = 1; @v_rbpool = p; @v_holdw = 1; end
            end else begin
              // A burst or a register not named in --axi-rw: what it changed is unknown.
              for (i = 0; i < @n; i = i + 1) @v_known[i] = 0;
            end
          end else @v_n_berr = @v_n_berr + 1;
          for (i = j; i < 3; i = i + 1) begin
            @v_obid[i] = @v_obid[i + 1]; @v_obpool[i] = @v_obpool[i + 1]; @v_oblen[i] = @v_oblen[i + 1];
            @v_obdata[i] = @v_obdata[i + 1]; @v_obstrb[i] = @v_obstrb[i + 1];
          end
          @v_obn = @v_obn - 1; @v_bc = @v_bc + 1;
        end
      end
      if (@v_arvalid && @v_s_arready) begin
        @v_orid[@v_orn] = @v_arid; @v_orpool[@v_orn] = @v_arpool; @v_orlen[@v_orn] = @v_arlen; @v_orbeat[@v_orn] = 0;
        @v_orbc[@v_orn] = @v_bc; @v_orchk[@v_orn] = !@v_won && @v_obn == 0; @v_orrb[@v_orn] = @v_arrb;
        @v_orn = @v_orn + 1; @v_aron = 0; @v_n_ar = @v_n_ar + 1;
        if (@v_arlen != 0) @v_n_burst = @v_n_burst + 1;
        if (@v_arfirst == @v_last_ar + 1) @v_n_b2b = @v_n_b2b + 1;
        @v_last_ar = k;
      end else if (@v_aron && @v_ardly > 0) @v_ardly = @v_ardly - 1;
      if (@v_rready && @v_s_rvalid) begin
        j = -1;
        for (i = @v_orn - 1; i >= 0; i = i - 1) if (@v_orid[i] == @v_s_rid) j = i;
        if (j < 0) begin
          @v_n_perr = @v_n_perr + 1;
          $fdisplay(fl, "error cycle %0d: @P RID %h matches no outstanding read", k, @v_s_rid);
        end else begin
          if (@v_s_rlast != (@v_orbeat[j] == @v_orlen[j])) begin
            @v_n_perr = @v_n_perr + 1;
            $fdisplay(fl, "error cycle %0d: @P RLAST %b on beat %0d of %0d", k, @v_s_rlast, @v_orbeat[j], @v_orlen[j] + 1);
          end
          if (@v_orbeat[j] == @v_orlen[j]) begin
            p = @v_orpool[j];
            if (@v_s_rresp == 2'b00) @v_n_rok = @v_n_rok + 1; else @v_n_rerr = @v_n_rerr + 1;
            m = @v_known[p] & @v_mask[p];
            if (@v_orlen[j] == 0 && @v_s_rresp == 2'b00 && @v_orchk[j] && !@v_won && @v_obn == 0 && @v_bc == @v_orbc[j] && m != 0) begin
              @v_n_rb = @v_n_rb + 1; @v_nrb[p] = @v_nrb[p] + 1;
              if (((@v_s_rdata ^ @v_shadow[p]) & m) != 0) begin
                @v_n_rbbad = @v_n_rbbad + 1;
                $fdisplay(fl, "error cycle %0d: @P read %h returned %h, written %h (bits %h)", k, @v_pool[p], @v_s_rdata, @v_shadow[p], m);
              end
            end
            if (@v_orrb[j]) @v_holdw = 0;
            for (i = j; i < 3; i = i + 1) begin
              @v_orid[i] = @v_orid[i + 1]; @v_orpool[i] = @v_orpool[i + 1]; @v_orlen[i] = @v_orlen[i + 1];
              @v_orbeat[i] = @v_orbeat[i + 1]; @v_orbc[i] = @v_orbc[i + 1]; @v_orchk[i] = @v_orchk[i + 1]; @v_orrb[i] = @v_orrb[i + 1];
            end
            @v_orn = @v_orn - 1;
          end else @v_orbeat[j] = @v_orbeat[j] + 1;
        end
      end
    end
"""

_REPORT = """
  if (@v_won || @v_aron || @v_obn != 0 || @v_orn != 0) begin
    @v_n_perr = @v_n_perr + 1;
    $fdisplay(fl, "error end: @P %0d writes and %0d reads outstanding after %0d cycles of draining (a lost or never-sent response hangs the PS)",
              @v_obn + @v_won, @v_orn + @v_aron, @drain);
  end
  $fdisplay(fl, "axi @P writes=%0d wbeats=%0d bokay=%0d berr=%0d partial=%0d reads=%0d rokay=%0d rerr=%0d bursts=%0d backtoback=%0d readbacks=%0d readbackbad=%0d protocol=%0d",
            @v_n_aw, @v_n_w, @v_n_bok, @v_n_berr, @v_n_part, @v_n_ar, @v_n_rok, @v_n_rerr, @v_n_burst, @v_n_b2b, @v_n_rb, @v_n_rbbad, @v_n_perr);
  for (i = 0; i < @n; i = i + 1) if (@v_mask[i] != 0) $fdisplay(fl, "rw @P %h readbacks=%0d", @v_pool[i], @v_nrb[i]);
"""

# Master outputs the testbench owns: port suffix -> expression (constants:
# 4-byte beats, INCR bursts, the rest 0). WID is AWID (AXI3).
_DRIVES = {"ARESETN": "rstn", "AWVALID": "@v_awvalid", "AWADDR": "@v_awaddr", "AWID": "@v_awid", "AWLEN": "@v_awlen",
           "AWSIZE": 2, "AWBURST": 1, "WVALID": "@v_wvalid", "WDATA": "@v_wdata", "WSTRB": "@v_wstrb",
           "WLAST": "@v_wlast", "WID": "@v_awid", "BREADY": "@v_bready", "ARVALID": "@v_arvalid",
           "ARADDR": "@v_araddr", "ARID": "@v_arid", "ARLEN": "@v_arlen", "ARSIZE": 2, "ARBURST": 1,
           "RREADY": "@v_rready"}
_ZERO = ("AWLOCK", "AWCACHE", "AWPROT", "AWQOS", "ARLOCK", "ARCACHE", "ARPROT", "ARQOS")


def pool(master: str, rw: dict[int, int], seed: int) -> tuple[list[int], int]:
    """Addresses a master uses, and how many lead the list: the --axi-rw
    registers in its window first, then its first eight words and four
    random words in it."""
    base = 0x4000_0000 if master == "MAXIGP0" else 0x8000_0000
    rng = random.Random(seed)
    lead = sorted(a for a in rw if base <= a < base + (1 << 30))
    out = lead + [base + 4 * i for i in range(8)] + [base + 4 * rng.randrange(1 << 28) for _ in range(4)]
    return list(dict.fromkeys(out)), len(lead)


def axi_tb(top: str, top_ports: list, b: Boundary, in_bits: list[str], clock: str, cycles: int,
           base_stim: str, stim: str, log: str, seed: int, rw: dict[int, int]) -> str:
    n = max(1, len(in_bits))
    col = {bit: n - 1 - i for i, bit in enumerate(in_bits)}
    ref = lambda bit: f"dut.{bit}" if "." in bit else bit
    drain = min(64, cycles // 4)
    decl, conn = [], []
    for p in top_ports:
        w = "" if p.width is None else f"[{p.hi}:{p.lo}] "
        decl.append(f"reg {w}{p.name} = 0;" if p.direction == "in" else f"wire {w}{p.name};")
        conn.append(f".{p.name}({p.name})")
    widths = {}
    for bit in b.out_bits + b.in_bits:
        m = re.match(rf"{b.inst}\.(\w+)(?:\[(\d+)\])?$", bit)
        widths[m.group(1)] = max(widths.get(m.group(1), 0), int(m.group(2) or 0) + 1)
    blocks = {"decl": [], "init": [], "decide": [], "override": [], "sample": [], "update": [], "report": [], "busy": []}
    for idx, P in enumerate(b.masters):
        v, d = f"m{idx}", f"dut.{b.inst}.{P}"
        addrs, n_rw = pool(P, rw, seed + idx)
        # Half the addresses picked are --axi-rw registers, so most writes get read back.
        pick = f"(rnd(2) == 0 ? rnd({n_rw}) : rnd({len(addrs)}))" if n_rw else f"rnd({len(addrs)})"
        sub = lambda t: t.replace("@pick", pick).replace("@drain", str(drain)).replace("@v", v).replace("@d", d).replace("@P", P).replace("@n", str(len(addrs)))
        blocks["decl"].append(sub(_DECL))
        for i, a in enumerate(addrs):
            blocks["init"].append(f"  {v}_pool[{i}] = 32'h{a:08x}; {v}_mask[{i}] = 32'h{rw.get(a, 0):08x}; "
                                  f"{v}_known[{i}] = 0; {v}_shadow[{i}] = 0; {v}_nrb[{i}] = 0;")
        blocks["decide"].append(sub(_DECIDE))
        blocks["sample"].append(sub(_SAMPLE))
        blocks["update"].append(sub(_UPDATE))
        blocks["report"].append(sub(_REPORT))
        blocks["busy"].append(sub("@v_won || @v_aron || @v_obn != 0 || @v_orn != 0"))
        drives = dict(_DRIVES, **{z: 0 for z in _ZERO})
        for suffix, e in drives.items():
            w = widths[P + suffix]
            for k in range(w):
                bit = f"{b.inst}.{P}{suffix}" if w == 1 else f"{b.inst}.{P}{suffix}[{k}]"
                if bit == clock or bit not in col:
                    continue
                val = f"1'b{(e >> k) & 1}" if isinstance(e, int) else (sub(e) if w == 1 else f"{sub(e)}[{k}]")
                blocks["override"].append(f"    vec[{col[bit]}] = {val};")
    blocks["override"] += [f"    vec[{col[r]}] = rstn;" for r in b.resets if r in col]
    drive = [f"    {ref(bit)} = vec[{n - 1 - i}];" for i, bit in enumerate(in_bits)]
    first_reset = any(l[in_bits.index(b.resets[0])] == "0" for l in open(base_stim).read().split())
    j = lambda key: "\n".join(blocks[key])
    return f"""`timescale 1ns/1ps
// x7v: AXI3 masters on the PS7's M_AXI_GP ports, run against the RTL; the
// stimulus they produce (with the other inputs' random walk) is written out
// for both simulations to replay.
module tb_axi;
{chr(10).join(decl)}
{top} dut({', '.join(conn)});
integer fi, fs, fl, k, rc, i, j, p;
integer seed = {seed};
integer quiet = {-1 if first_reset else RESET_QUIET};   // -1: idle until the first reset
reg rstn = 0, rq, act, drain, stop;
reg [31:0] m;
reg [{n}-1:0] vec;
function integer rnd(input integer lim);
  rnd = $unsigned($random(seed)) % lim;
endfunction
function [3:0] strb(input integer unused);
  case (rnd(4))
    0, 1: strb = 4'hF;
    2: strb = $random(seed);
    default: strb = 4'h1 << rnd(4);
  endcase
endfunction
{j("decl")}
initial begin
{j("init")}
  fi = $fopen("{base_stim}", "r");
  fs = $fopen("{stim}", "w");
  fl = $fopen("{log}", "w");
  for (k = 0; k < {cycles}; k = k + 1) begin
    rc = $fscanf(fi, "%b", vec);
    rq = vec[{col[b.resets[0]]}];
    // A reset waits for every transaction to complete (see above).
    rstn = rq || (rstn && ({' || '.join(blocks['busy']) or '0'}));
    if (!rstn) quiet = {RESET_QUIET}; else if (quiet > 0) quiet = quiet - 1;
    act = rstn && quiet == 0;
    drain = k >= {cycles - drain};
    stop = drain || !rq;
{j("decide")}
{j("override")}
    $fwrite(fs, "%b\\n", vec);
    {ref(clock)} = 1'b0;
{chr(10).join(drive)}
    #4;
{j("sample")}
    #1;
    {ref(clock)} = 1'b1;
    #4;
    #1;
{j("update")}
  end
{j("report")}
  $fclose(fs);
  $fclose(fl);
  $finish;
end
endmodule
"""


def run_axi(workdir: str, rtl_files: list[str], tb: str) -> tuple[list[str], list[str]]:
    """Run the master testbench; (summary lines, errors)."""
    tbf = os.path.join(workdir, "tb_axi.v")
    open(tbf, "w").write(tb)
    vvp = os.path.join(workdir, "axi.vvp")
    r = subprocess.run(["iverilog", "-g2012", "-o", vvp, tbf, *rtl_files], capture_output=True, text=True)
    if r.returncode == 0:
        r = subprocess.run(["vvp", "-n", vvp], capture_output=True, text=True, cwd=workdir)
    if r.returncode != 0:
        raise RuntimeError(f"AXI master run failed:\n{(r.stdout + r.stderr)[-3000:]}")
    lines = open(os.path.join(workdir, "axi.log")).read().splitlines()
    return [l for l in lines if not l.startswith("error")], [l for l in lines if l.startswith("error")]


def axi_problems(summary: list[str]) -> list[str]:
    """Reasons the AXI traffic did not prove anything (beyond logged errors)."""
    out = []
    for l in summary:
        f = dict(kv.split("=") for kv in l.split() if "=" in kv)
        if l.startswith("axi "):
            if int(f["bokay"]) + int(f["berr"]) == 0 or int(f["rokay"]) + int(f["rerr"]) == 0:
                out.append(f"{l.split()[1]}: no write or no read completed")
            if int(f["writes"]) != int(f["bokay"]) + int(f["berr"]) or int(f["reads"]) != int(f["rokay"]) + int(f["rerr"]):
                out.append(f"{l.split()[1]}: {f['writes']} writes and {f['reads']} reads, {int(f['bokay']) + int(f['berr'])} B and {int(f['rokay']) + int(f['rerr'])} last R responses")
            if int(f["readbackbad"]) or int(f["protocol"]):
                out.append(f"{l.split()[1]}: {f['readbackbad']} readback mismatches, {f['protocol']} protocol errors")
        elif l.startswith("rw ") and int(f["readbacks"]) == 0:
            out.append(f"{l.split()[1]} register {l.split()[2]}: never read back after a write")
    return out

# FFMUX / FF INIT corpus census (W-CORPUS, UltraScale+ grind).
#
# Reads the 14 decoded Vivado pair FASMs in
# ~/workspace/us09-checkers-logs/pairs/ and answers, from Vivado data
# alone (no production code is touched):
#   Q1: FFMUXA1 selection census by selection and slice letter.
#   Q2: for FFs whose D is driven by a LUT O in the SAME slice (same
#       letter, LUT INIT present), what does Vivado emit: D6 with no
#       fabric PIPs feeding the slice AX pin, BYP with fabric PIPs
#       feeding AX, or anything else.
#   Q3: FF INIT census (V0 vs V1), all slots and used-FF-only.
#
# Method for Q2: the slice AX pin is the only fabric entry to the FF D
# mux (BYP). Its wire is found through the prjuray-db tile type JSONs
# (Fabric.site_pin_wire), its node through tileconn stitching
# (Fabric.node), and active PIPs feeding any wire of that node are
# collected from the decoded FASM itself. The ultimate driver is
# resolved with Extractor.driver. A D6 FF needs no AX driver; a BYP FF
# needs one. Carry DI use of the same AX pin (CARRY8.DI<i>.<letter>X)
# is recorded separately, since it legitimately drives AX while the FF
# uses D6.
#
# Usage: python3 census.py [pair ...] [--out JSON]
# With no pairs, all 14 are processed. Writes a JSON summary and
# prints a text summary. No em dashes anywhere.
from __future__ import annotations

import json
import os
import re
import sys
from collections import Counter, defaultdict

sys.path.insert(0, os.path.expanduser(
    "~/workspace/helion-ultra-bitgen-ffmux-corpus/tools/us09v"))
sys.path.insert(0, os.path.expanduser(
    "~/workspace/helion-ultra-bitgen-ffmux-corpus/tools/us09v/us09v"))

from us09v.db import Db  # noqa: E402
from us09v.extract import Extractor, LETTERS  # noqa: E402
from us09v.fasmio import parse_fasm_file, init_value  # noqa: E402

PAIRS_DIR = os.path.expanduser("~/workspace/us09-checkers-logs/pairs")
PAIR_FILES = {
    "P-11": "P-11_resizer_ultra96.decoded.fasm",
    "P-13": "P-13_cnvW1A1-ultra96.decoded.fasm",
    "P-14": "P-14_cnvW1A2-ultra96.decoded.fasm",
    "P-15": "P-15_cnvW2A2-ultra96.decoded.fasm",
    "P-16": "P-16_lfcW1A1-ultra96.decoded.fasm",
    "P-17": "P-17_lfcW1A2-ultra96.decoded.fasm",
    "P-23": "P-23_cnv-w1a1.decoded.fasm",
    "P-28": "P-28_xv2Filter2DDilate.decoded.fasm",
    "P-40": "P-40_cnv-w1a2.decoded.fasm",
    "P-41": "P-41_cnv-w2a2.decoded.fasm",
    "P-42": "P-42_tfc-w1a1.decoded.fasm",
    "P-43": "P-43_tfc-w1a2.decoded.fasm",
    "P-44": "P-44_tfc-w2a2.decoded.fasm",
    "P-45": "P-45_unsw_nb15-mlp-w2a2.decoded.fasm",
}

FFMUX_RE = re.compile(r"^FFMUX([A-H])([12])\.(.+)$")
INIT_RE = re.compile(r"^([A-H]FF2?)\.INIT\.(V[01])$")
SRVAL_RE = re.compile(r"^([A-H]FF2?)\.SRVAL\.(V[01])$")


def normalize_pips(tiles):
    """Fold bidirectional-PIP direction suffixes into plain PIP
    features so the us09v driver code (which matches PIP.<dst>.<src>
    exactly) sees them. `PIP.D.S.FWD` means S drives D, so it becomes
    `PIP.D.S`; `PIP.D.S.REV` means D drives S, so it becomes
    `PIP.S.D`. Returns (n_fwd, n_rev) folded. The decoded FASM never
    lists one (tile, dst, src) pair in two forms (verified on P-11:
    0 overlaps), so no feature is double-counted."""
    n_fwd = n_rev = 0
    for tile, feats in tiles.items():
        add = {}
        for feat in list(feats):
            if not feat.startswith("PIP."):
                continue
            parts = feat.split(".")
            if len(parts) == 4 and parts[3] == "FWD":
                add[f"PIP.{parts[1]}.{parts[2]}"] = feats[feat]
                n_fwd += 1
            elif len(parts) == 4 and parts[3] == "REV":
                add[f"PIP.{parts[2]}.{parts[1]}"] = feats[feat]
                n_rev += 1
        feats.update(add)
    return n_fwd, n_rev


def ax_net(ext, site, letter):
    """Return (nid, pip_list, driver) for the slice AX pin, or None
    when the pin has no tile wire. pip_list holds (tile, feature)
    for every active PIP driving any wire of the AX node."""
    fab = ext.fab
    ftile = ext._fabric_tile(site)
    wire = fab.site_pin_wire(ftile, site.index, letter + "X")
    if wire is None:
        return None
    nid = fab.node(ftile, wire)
    members = fab.nodes[nid]
    pips = []
    tt_cache = {}
    for (t, w) in members:
        ttype = fab.tile_type(t)
        if ttype is None:
            continue
        tt = tt_cache.get(ttype)
        if tt is None:
            tt = fab.tt(ttype)
            tt_cache[ttype] = tt
        for (src, _bid) in tt["pips_into"].get(w, ()):
            feat = f"PIP.{w}.{src}"
            if feat in ext.tiles.get(t, {}):
                pips.append((t, feat))
    driver = ext.driver(nid)
    return nid, pips, driver


def census_pair(db, pair, path):
    tiles = parse_fasm_file(path)
    n_fwd, n_rev = normalize_pips(tiles)
    ext = Extractor(db, tiles)
    q1 = Counter()          # (selection,) and (letter, selection)
    q1_letter_sel = Counter()
    q1_num = Counter()      # (letter, num, selection)
    q3_all = Counter()      # (ff, V0/V1)
    q3_srval = Counter()
    q3_used = Counter()     # (ff, V0/V1) for FFs with an FFMUX line
    q2_rows = []            # one row per (slice, letter, num) with
                            # FFMUX present and same-letter LUT present
    examples = {"a": [], "b": [], "anomaly": []}
    n_slices = 0
    n_slicem = 0
    for name in sorted(ext.sites):
        site = ext.sites[name]
        if site.stype not in ("SLICEL", "SLICEM"):
            continue
        n_slices += 1
        if site.stype == "SLICEM":
            n_slicem += 1
        F = site.features
        for feat in F:
            m = FFMUX_RE.match(feat)
            if m:
                letter, num, sel = m.group(1), m.group(2), m.group(3)
                q1[sel] += 1
                q1_letter_sel[(letter, sel)] += 1
                q1_num[(letter, num, sel)] += 1
                # Used-FF INIT split: every FF with an FFMUX line,
                # whether or not its same-letter LUT is present.
                ff = f"{letter}FF" + ("2" if num == "2" else "")
                for v in ("V0", "V1"):
                    if f"{ff}.INIT.{v}" in F:
                        q3_used[(ff, v)] += 1
            m = INIT_RE.match(feat)
            if m:
                q3_all[(m.group(1), m.group(2))] += 1
            m = SRVAL_RE.match(feat)
            if m:
                q3_srval[(m.group(1), m.group(2))] += 1
        # Real carry use in this slice: some mux selects CY or XORIN.
        # (Refined lazily below with Extractor._carry_used, which also
        # counts CI/CI_TOP selections, DI{i}.DI{i}, and a consumed
        # COUT, i.e. a carry chain passing through.)
        carry_cache = {}
        wide_used = any(
            (f.startswith(f"FFMUX{c}") or f.startswith(f"OUTMUX{c}"))
            and f.endswith(".F7F8")
            for c in LETTERS for f in F)
        # AX net per letter, computed once and shared by both FFs.
        ax_cache = {}
        for letter in LETTERS:
            lut_init = init_value(F, letter)
            if lut_init is None:
                continue
            for num in ("1", "2"):
                sel = None
                for feat in F:
                    if feat.startswith(f"FFMUX{letter}{num}."):
                        sel = feat.split(".", 1)[1]
                        break
                if sel is None:
                    continue
                ff = f"{letter}FF" + ("2" if num == "2" else "")
                # Sibling FF (same letter, other index) selection.
                sib_num = "2" if num == "1" else "1"
                sib_sel = None
                for feat in F:
                    if feat.startswith(f"FFMUX{letter}{sib_num}."):
                        sib_sel = feat.split(".", 1)[1]
                        break
                # AX net analysis.
                if letter not in ax_cache:
                    ax_cache[letter] = ax_net(ext, site, letter)
                net = ax_cache[letter]
                if net is None:
                    row = {"pair": pair, "slice": name, "letter": letter,
                           "num": num, "sel": sel, "stype": site.stype,
                           "lut_init": f"0x{lut_init:016x}",
                           "ax": "no-wire", "pip_fed": None,
                           "driver": None, "carry_used": None,
                           "wide_used": wide_used, "sib_sel": sib_sel}
                    q2_rows.append(row)
                    continue
                nid, pips, driver = net
                pip_fed = len(pips) > 0
                if driver[0] == "site":
                    drv = f"site {driver[1]}.{driver[2]}"
                elif driver[0] == "const":
                    drv = f"const {driver[1]}"
                elif driver[0] == "primary":
                    drv = f"primary {driver[1]}"
                else:
                    drv = driver[0]
                same_lut = (driver[0] == "site" and driver[1] == name
                            and driver[2] == letter + "MUX")
                # OUTMUX selection for this letter: where the LUT O goes
                # when the FF does not take it (BYP case).
                outmux_sel = None
                for feat in F:
                    if feat.startswith(f"OUTMUX{letter}."):
                        outmux_sel = feat.split(".", 1)[1]
                        break
                # Lazy carry verdict, only needed when AX is fed but
                # the FF uses D6 and the sibling does not use BYP.
                carry_used = None
                if pip_fed and sel == "D6" and sib_sel != "BYP":
                    if name not in carry_cache:
                        carry_cache[name] = ext._carry_used(site)
                    carry_used = carry_cache[name]
                row = {"pair": pair, "slice": name, "letter": letter,
                       "num": num, "sel": sel, "stype": site.stype,
                       "lut_init": f"0x{lut_init:016x}",
                       "ax": "ok", "pip_fed": pip_fed,
                       "n_pips": len(pips),
                       "pip_sample": [f"{t}.{f}" for (t, f) in pips[:3]],
                       "driver": drv, "same_slice_lut": same_lut,
                       "carry_used": carry_used,
                       "wide_used": wide_used,
                       "sib_sel": sib_sel,
                       "outmux_sel": outmux_sel}
                q2_rows.append(row)
                if sel == "D6" and not pip_fed and len(examples["a"]) < 5:
                    examples["a"].append(row)
                elif sel == "BYP" and pip_fed and len(examples["b"]) < 5:
                    examples["b"].append(row)
                elif ((sel == "D6" and pip_fed and sib_sel != "BYP"
                        and carry_used is False and not wide_used)
                        or (sel == "BYP" and not pip_fed)):
                    if len(examples["anomaly"]) < 10:
                        examples["anomaly"].append(row)
    return {
        "pair": pair,
        "fasm": os.path.basename(path),
        "n_slices": n_slices,
        "n_slicem": n_slicem,
        "n_pip_fwd": n_fwd,
        "n_pip_rev": n_rev,
        "q1_sel": {k: v for k, v in sorted(q1.items())},
        "q1_letter_sel": {f"{k[0]}:{k[1]}": v
                          for k, v in sorted(q1_letter_sel.items())},
        "q1_num": {f"{k[0]}{k[1]}:{k[2]}": v
                   for k, v in sorted(q1_num.items())},
        "q3_all": {f"{k[0]}:{k[1]}": v for k, v in sorted(q3_all.items())},
        "q3_srval": {f"{k[0]}:{k[1]}": v
                     for k, v in sorted(q3_srval.items())},
        "q3_used": {f"{k[0]}:{k[1]}": v for k, v in sorted(q3_used.items())},
        "q2_rows": q2_rows,
        "examples": examples,
    }


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    out = None
    for a in sys.argv[1:]:
        if a.startswith("--out="):
            out = a[len("--out="):]
    pairs = args or sorted(PAIR_FILES)
    db = Db()
    print(f"db root: {db.root} part: {db.part}", flush=True)
    results = []
    for pair in pairs:
        path = os.path.join(PAIRS_DIR, PAIR_FILES[pair])
        print(f"--- {pair} {PAIR_FILES[pair]}", flush=True)
        r = census_pair(db, pair, path)
        print(f"    slices={r['n_slices']} slicem={r['n_slicem']} "
              f"ffmux={sum(r['q1_sel'].values())} "
              f"q2rows={len(r['q2_rows'])}", flush=True)
        results.append(r)
    if out:
        with open(out, "w") as f:
            json.dump(results, f, indent=1)
        print(f"wrote {out}", flush=True)
    # Text rollup.
    tot_q1 = Counter()
    tot_q1_ls = Counter()
    tot_q3_all = Counter()
    tot_q3_used = Counter()
    tot_q3_srval = Counter()
    tot_q2 = Counter()
    for r in results:
        for k, v in r["q1_sel"].items():
            tot_q1[k] += v
        for k, v in r["q1_letter_sel"].items():
            tot_q1_ls[k] += v
        for k, v in r["q3_all"].items():
            tot_q3_all[k] += v
        for k, v in r["q3_used"].items():
            tot_q3_used[k] += v
        for k, v in r["q3_srval"].items():
            tot_q3_srval[k] += v
        for row in r["q2_rows"]:
            sel = row["sel"]
            if row["ax"] == "no-wire":
                tot_q2[(sel, "ax-no-wire")] += 1
            elif sel == "D6" and not row["pip_fed"]:
                tot_q2[("D6", "AX undriven")] += 1
            elif sel == "D6" and row["pip_fed"] and row["sib_sel"] == "BYP":
                tot_q2[("D6", "AX fed, sibling FF uses BYP")] += 1
            elif sel == "D6" and row["pip_fed"] and row["carry_used"]:
                tot_q2[("D6", "AX fed, carry used in slice")] += 1
            elif sel == "D6" and row["pip_fed"] and row["wide_used"]:
                tot_q2[("D6", "AX fed, wide mux uses AX")] += 1
            elif sel == "D6" and row["pip_fed"]:
                tot_q2[("D6", "AX fed, unexplained")] += 1
            elif sel == "BYP" and row["pip_fed"] and row["same_slice_lut"]:
                tot_q2[("BYP", "AX fed by own LUT O")] += 1
            elif sel == "BYP" and row["pip_fed"]:
                tot_q2[("BYP", "AX fabric-fed")] += 1
            elif sel == "BYP" and not row["pip_fed"]:
                tot_q2[("BYP", "AX undriven")] += 1
            else:
                pf = "fed" if row["pip_fed"] else "undriven"
                tot_q2[(sel, f"AX {pf}")] += 1
    print("=== Q1 FFMUX by selection (all 14) ===")
    for k, v in sorted(tot_q1.items(), key=lambda kv: -kv[1]):
        print(f"  {k}: {v}")
    print("=== Q1 FFMUX by letter:selection ===")
    for k, v in sorted(tot_q1_ls.items()):
        print(f"  {k}: {v}")
    print("=== Q3 INIT all slots ===")
    v0 = sum(v for (k, v) in tot_q3_all.items() if k.endswith(":V0"))
    v1 = sum(v for (k, v) in tot_q3_all.items() if k.endswith(":V1"))
    print(f"  V0 total: {v0}  V1 total: {v1}")
    print("=== Q3 INIT used FFs only ===")
    v0 = sum(v for (k, v) in tot_q3_used.items() if k.endswith(":V0"))
    v1 = sum(v for (k, v) in tot_q3_used.items() if k.endswith(":V1"))
    print(f"  V0 total: {v0}  V1 total: {v1}")
    print("=== Q3 SRVAL all slots ===")
    v0 = sum(v for (k, v) in tot_q3_srval.items() if k.endswith(":V0"))
    v1 = sum(v for (k, v) in tot_q3_srval.items() if k.endswith(":V1"))
    print(f"  V0 total: {v0}  V1 total: {v1}")
    print("=== Q2 same-slice LUT + FFMUX population ===")
    for k, v in sorted(tot_q2.items(), key=lambda kv: -kv[1]):
        print(f"  {k[0]} / {k[1]}: {v}")


if __name__ == "__main__":
    main()

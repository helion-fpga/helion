#!/usr/bin/env python3
"""Compare a device.tcl dump against the database Helion builds from.

  compare_db.py <device-dump-dir> <db-dir> [--part-dir P] [--prefix-map VIVADO=DB ...]

<db-dir> is a Project X-Ray / prjuray style family directory: tile_type_<T>.json,
segbits_<t>.db, ppips_<t>.db, site_type_<S>.json, and (with --part-dir) a part
directory holding tilegrid.json and package_pins.csv.

Prints a Markdown report:
  * grid: tiles only in Vivado / only in the DB, type and coordinate mismatches
  * tile types with no tile_type JSON, and with no segbits at all
  * per tile type: Vivado PIPs (non-test) vs DB PIPs, and how many Vivado PIPs
    have neither segbits nor a pseudo-PIP entry (bits Helion cannot set)
  * site types missing from the DB, and BEL-count differences
  * package pins missing or mapped to a different site
Exit code is 0; the report is the result.
"""
import argparse
import csv
import json
import os
import sys
from collections import Counter, defaultdict


def tsv(path):
    with open(path, newline="") as f:
        return list(csv.DictReader(f, delimiter="\t", quoting=csv.QUOTE_NONE))


def db_features(db, ttype):
    """Feature names (TYPE.DST.SRC) with bits, and pseudo-PIP names, for one tile type."""
    out = set()
    for kind in ("segbits", "ppips"):
        p = os.path.join(db, f"{kind}_{ttype.lower()}.db")
        if os.path.exists(p):
            for line in open(p):
                if line.strip():
                    out.add(line.split()[0])
    return out


def pip_feature(ttype, name):
    # Vivado "TILE/TYPE.SRC->>DST" (or "->" bidirectional) -> X-Ray "TYPE.DST.SRC".
    body = name.split("/", 1)[-1]
    body = body.split(".", 1)[1] if "." in body else body
    sep = "->>" if "->>" in body else "->"
    src, dst = body.split(sep, 1)
    return f"{ttype}.{dst}.{src}", sep == "->"


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("dump")
    ap.add_argument("db")
    ap.add_argument("--part-dir", help="dir with package_pins.csv (X-Ray: <family>/<part-with-package>)")
    ap.add_argument("--tilegrid", help="tilegrid.json (X-Ray: <family>/<device>/tilegrid.json)")
    ap.add_argument("--limit", type=int, default=25, help="rows per list")
    a = ap.parse_args()
    L = a.limit
    print(f"# Vivado vs database: `{os.path.basename(a.dump.rstrip('/'))}`\n")

    grid = tsv(os.path.join(a.dump, "grid.tsv"))
    vtypes = Counter(r["TYPE"] for r in grid)
    print(f"Vivado: {len(grid)} tiles, {len(vtypes)} tile types.\n")

    if a.tilegrid:
        tg = json.load(open(a.tilegrid))
        vg = {r["NAME"]: r for r in grid}
        only_v = sorted(set(vg) - set(tg))
        only_d = sorted(set(tg) - set(vg))
        bad_type = [t for t in vg.keys() & tg.keys() if vg[t]["TYPE"] != tg[t]["type"]]
        bad_xy = [t for t in vg.keys() & tg.keys()
                  if (int(vg[t]["COLUMN"]), int(vg[t]["ROW"])) != (tg[t]["grid_x"], tg[t]["grid_y"])]
        site_diff = []
        for t in vg.keys() & tg.keys():
            vs = dict(s.split(":") for s in vg[t]["SITES"].split(",") if s)
            if vs != tg[t].get("sites", {}):
                site_diff.append((t, sorted(set(vs.items()) ^ set(tg[t].get("sites", {}).items()))[:4]))
        no_bits = Counter(tg[t]["type"] for t in tg if not tg[t].get("bits"))
        print("## Grid\n")
        print(f"- tiles only in Vivado: {len(only_v)} {only_v[:L]}")
        print(f"- tiles only in DB: {len(only_d)} {only_d[:L]}")
        print(f"- type mismatches: {len(bad_type)} {bad_type[:L]}")
        print(f"- (col,row) mismatches: {len(bad_xy)} {bad_xy[:L]}")
        print(f"- tiles whose site list differs: {len(site_diff)}")
        for t, d in sorted(site_diff)[:L]:
            print(f"  - {t}: {d}")
        print(f"- DB tiles with no frame-address (`bits`) entry, by type: {dict(no_bits.most_common(L))}\n")

    print("## Tile types\n")
    print("| tile type | tiles | Vivado PIPs | DB PIPs | PIPs with no bits and not pseudo | tile_type JSON | segbits |")
    print("|---|---|---|---|---|---|---|")
    unknown_total = Counter()
    examples = {}
    for t, n in sorted(vtypes.items()):
        pp = os.path.join(a.dump, "tile_types", f"{t}.pips.tsv")
        vp = [r for r in tsv(pp) if r.get("IS_TEST_PIP") != "1"] if os.path.exists(pp) else []
        tj = os.path.join(a.db, f"tile_type_{t}.json")
        dbp = len(json.load(open(tj))["pips"]) if os.path.exists(tj) else None
        has_seg = os.path.exists(os.path.join(a.db, f"segbits_{t.lower()}.db"))
        feats = db_features(a.db, t)
        missing = []
        for r in vp:
            f, bidir = pip_feature(t, r["NAME"])
            fu = f.replace(f"{t}.", f"{t}.PIP.", 1)  # prjuray spelling: TYPE.PIP.DST.SRC
            if f in feats or fu in feats or (bidir and (f + ".FWD" in feats or fu + ".FWD" in feats)):
                continue
            if r.get("IS_PSEUDO") == "1":
                continue
            missing.append(f)
        if vp:
            unknown_total[t] = len(missing)
            examples[t] = missing[:5]
        if vp or dbp:
            print(f"| {t} | {n} | {len(vp)} | {dbp if dbp is not None else '-'} | {len(missing)} | "
                  f"{'yes' if dbp is not None else 'NO'} | {'yes' if has_seg else 'no'} |")
    print("\nTile types with the most PIPs Helion cannot encode (no segbits, not pseudo):\n")
    for t, m in unknown_total.most_common(L):
        if m:
            print(f"- {t}: {m}, e.g. {examples[t]}")

    print("\n## Site types (site pins and site PIPs)\n")
    sdir = os.path.join(a.dump, "site_types")
    for f in sorted(os.listdir(sdir)) if os.path.isdir(sdir) else []:
        if not f.endswith(".pins.tsv"):
            continue
        st = f[:-len(".pins.tsv")]
        vpins = {r["PIN"] for r in tsv(os.path.join(sdir, f))}
        sp = os.path.join(sdir, f"{st}.sitepips.tsv")
        vpips = {r["SITE_PIP"] for r in tsv(sp)} if os.path.exists(sp) else set()
        sj = os.path.join(a.db, f"site_type_{st}.json")
        if not os.path.exists(sj):
            print(f"- {st}: no site_type JSON in DB ({len(vpins)} pins, {len(vpips)} site PIPs in Vivado)")
            continue
        d = json.load(open(sj))
        dpins = set(d.get("site_pins", {}))
        dpips = set(d.get("site_pips", {}))
        if vpins != dpins or vpips != dpips:
            print(f"- {st}: pins only Vivado {sorted(vpins - dpins)[:L]}, only DB {sorted(dpins - vpins)[:L]}; "
                  f"site PIPs only Vivado {len(vpips - dpips)} {sorted(vpips - dpips)[:L]}, only DB {len(dpips - vpips)}")

    if a.part_dir and os.path.exists(os.path.join(a.part_dir, "package_pins.csv")):
        print("\n## Package pins\n")
        dbpins = {r["pin"]: r for r in csv.DictReader(open(os.path.join(a.part_dir, "package_pins.csv")))}
        vpins = {r["NAME"]: r for r in tsv(os.path.join(a.dump, "package_pins.tsv"))}
        only_v = sorted(set(vpins) - set(dbpins))
        only_d = sorted(set(dbpins) - set(vpins))
        print(f"- Vivado {len(vpins)} pins, DB {len(dbpins)}; only Vivado {len(only_v)} {only_v[:L]}; only DB {len(only_d)} {only_d[:L]}")
        site_key = next((k for k in ("SITE", "IOB_SITE") if k in next(iter(vpins.values()), {})), None)
        if site_key:
            bad = [p for p in vpins.keys() & dbpins.keys()
                   if vpins[p][site_key] and dbpins[p]["site"] != vpins[p][site_key]]
            print(f"- pins mapped to a different site: {len(bad)} {bad[:L]}")


if __name__ == "__main__":
    sys.exit(main())

# INT PIP completeness audit

Date: 2026-11-07. Worker: segbits-vendor. Audit code:
`crates/helion-ultrascale/tests/int_pip_audit.rs`
(`cargo test -p helion-ultrascale --test int_pip_audit`).

## Part 1: DB-internal audit (done now)

Question: does `segbits_int.db` (3778 lines) cover every PIP of one INT
tile, as enumerated by the prjuray-db tile-type JSON?

Enumeration source: `tile_type_INT.json` (`pips` object, 3774 entries).
Each entry carries `src_wire`, `dst_wire`, and `is_directional`
(`1` = directional, `0` = bidirectional).

Mapping (verified empirically, see below): a tile pip maps to the segbits
feature `INT.PIP.<dst_wire>.<src_wire>`. The FASM feature names the mux
output first, then the mux input. Evidence: with the naive
`INT.PIP.<src>.<dst>` mapping, 0 of 3774 tile pips match; with the
`dst.src` mapping, 3770 match exactly, and the 4 that do not are
precisely the 4 bidirectional PIPs, each represented in the DB by a
`.FWD` / `.REV` pair.

### Counts

| Quantity | Count |
|---|---|
| Tile PIPs in `tile_type_INT.json` | 3774 |
| Directional tile PIPs (`is_directional` = 1) | 3770 |
| Bidirectional tile PIPs (`is_directional` = 0) | 4 |
| Segbits features in `segbits_int.db` | 3778 |
| Tile PIPs with a segbits entry (directional, 1:1) | 3770 |
| Bidirectional tile PIPs covered via `.FWD` + `.REV` pairs | 4 (8 features) |
| Tile PIPs with NO segbits entry | 0 |
| Segbits features with no tile PIP (unexplained) | 0 |

Coverage: 3774 of 3774 tile PIPs (100 percent). 3770 + 8 = 3778
segbits features, every one accounted for. Nothing is missing on
either side.

### The 4 bidirectional PIPs (each is one tile PIP, two DB features)

- `INT.PIP.BYPASS_E14.INT_NODE_IMUX_18_INT_OUT0` (.FWD / .REV)
- `INT.PIP.BYPASS_E8.INT_NODE_IMUX_5_INT_OUT0` (.FWD / .REV)
- `INT.PIP.BYPASS_W14.INT_NODE_IMUX_50_INT_OUT0` (.FWD / .REV)
- `INT.PIP.BYPASS_W8.INT_NODE_IMUX_37_INT_OUT0` (.FWD / .REV)

The `.FWD` / `.REV` pair selects the direction of the bidirectional
PIP (e.g. `.FWD` sets bit 74_42, `.REV` sets bit 71_31, for the
BYPASS_E14 case). A FASM line for a bidirectional PIP must name the
direction; the bare base feature has no DB entry and the loader
returns `None` for it, which is correct fail-loud behavior.

### Reproduction (Python, against the reference clone)

```sh
cd ~/workspace/prjuray-db-ref/zynqusp
python3 - <<'EOF'
import json
tt = json.load(open('tile_types/tile_type_INT.json'))
pips = tt['pips']
segs = {}
for line in open('segbits_int.db'):
    line = line.strip()
    if line:
        segs[line.split(' ', 1)[0]] = line
feats = {'INT.PIP.' + v['dst_wire'] + '.' + v['src_wire']: k
         for k, v in pips.items()}
print('tile pips:', len(pips))
print('covered 1:1:', len(set(feats) & set(segs)))
print('missing:', sorted(set(feats) - set(segs)))
print('extra:', sorted(set(segs) - set(feats)))
EOF
```

Expected output: tile pips 3774, covered 1:1 is 3770, missing is the
4 bidirectional base features, extra is the 8 `.FWD` / `.REV` features.

## Part 2: probe comparison (PENDING, blocked)

Question: does Vivado agree that one INT tile has exactly these 3774
PIPs? That needs the PinPlanning probe output (`get_pips -of_objects
[get_tiles INT_X<x>Y<y>]` from an in-memory PinPlanning project,
license-free). Status 2026-11-07: the probe has NOT been run on the
cluster yet (license server is down, but the probe needs no license;
Saksham prepared it and it is still not run). Until the probe output
exists, part 2 cannot run. This audit does NOT claim Vivado-level
completeness, only DB-internal completeness: the segbits DB covers
every PIP the tile-type JSON enumerates.

When the probe output lands, the comparison is: probe PIP list (Vivado
wire names) vs the 3774 tile-type PIPs vs the 3778 segbits features.
Any probe PIP with no segbits entry becomes a named fuzzing job per
the day-1 report (plan 3), never a silent skip.

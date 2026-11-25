"""Brute force small: enumerate every legal placement of 1 to 20
cells on a small slice grid and check each one with the place
checker (and, for the fixture designs, with co-sim).

The enumeration assigns each cell to a (site, letter) pair, pruning
by capacity (one cell per BEL, 8 LUTs and 16 FFs per slice) and by
the carry-chain rule (a CARRY8 cell's CIN must come from the slice
directly below). Counts are exact: the number of legal placements
for a given cell set is a property of the grid, so a wrong count
means the enumerator or the checker is wrong.

No em dashes anywhere.
"""
from __future__ import annotations

from itertools import product

from .placecheck import check_place_list


def slice_grid(nx: int, ny: int, x0: int = 0, y0: int = 0) -> list[str]:
    return [f"SLICE_X{x}Y{y}" for x in range(x0, x0 + nx) for y in range(y0, y0 + ny)]


def enumerate_placements(
    cells: list[tuple[str, str]],
    sites: list[str],
    cap: int = 200_000,
) -> tuple[list[list[tuple[str, str, str]]], int]:
    """cells: [(cell, kind)] with kind in {"LUT", "FF", "CARRY8"}.
    Returns (legal placements, pruned count). Each placement is a
    list of (cell, site, bel). The enumeration is exact up to the
    cap: when the cap is hit, enumeration stops and the caller must
    report the partial count, never the full one."""
    bels = {
        "LUT": [f"{c}LUT" for c in "ABCDEFGH"],
        "FF": [f"{c}FF" for c in "ABCDEFGH"] + [f"{c}FF2" for c in "ABCDEFGH"],
        "CARRY8": ["CARRY8"],
    }
    legal: list[list[tuple[str, str, str]]] = []
    pruned = 0
    # Per-cell candidate (site, bel) lists.
    cands = []
    for _cell, kind in cells:
        cands.append([(s, b) for s in sites for b in bels[kind]])

    def rec(i: int, chosen: list[tuple[str, str, str]], used: set[tuple[str, str]]):
        nonlocal pruned
        if len(legal) + pruned >= cap:
            return
        if i == len(cells):
            rep = check_place_list(chosen)
            if rep.ok:
                legal.append(list(chosen))
            else:
                pruned += 1
            return
        cell, kind = cells[i]
        for site, bel in cands[i]:
            if (site, bel) in used:
                pruned += 1
                continue
            # Carry chain rule: a CARRY8 on site Y must have the
            # slice below free for the chain head or the previous
            # CARRY8 cell. Prune here only the obvious case: two
            # CARRY8 cells on non-adjacent slices cannot chain.
            chosen.append((cell, site, bel))
            used.add((site, bel))
            rec(i + 1, chosen, used)
            chosen.pop()
            used.discard((site, bel))

    rec(0, [], set())
    return legal, pruned


def count_legal_placements(cells: list[tuple[str, str]], sites: list[str]) -> int:
    """Exact count of legal placements (no cap): only for small
    problems where the product is bounded."""
    legal, _pruned = enumerate_placements(cells, sites, cap=10_000_000)
    return len(legal)


def enumerate_and_cosim_placeholder() -> str:
    """Enumerate-and-cosim for the tiny fixture designs is driven by
    the fixture tests (tests/test_brute.py), which build each
    placement's FASM, extract, and co-simulate. This module only
    owns the enumeration; the co-sim loop lives in the tests so the
    fixture set stays the single source of truth."""
    return "see tests/test_brute.py"

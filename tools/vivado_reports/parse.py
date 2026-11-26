#!/usr/bin/env python3
"""Turn Vivado 2017.3 text reports into JSON.

    python3 tools/vivado_reports/parse.py KIND FILE
    python3 tools/vivado_reports/parse.py KIND -   # read stdin

KIND is one of: timing_summary, utilization, power, clock_utilization,
control_sets, drc, methodology (the report_ prefix is also accepted).

Every report parses on its own. A missing section becomes null, never a
crash: an empty input yields a dict of nulls, and a report with only some
sections yields nulls for the absent ones.

Python standard library only.
"""
from __future__ import annotations

import argparse
import json
import re
import sys

KINDS = (
    "timing_summary",
    "utilization",
    "power",
    "clock_utilization",
    "control_sets",
    "drc",
    "methodology",
)

_ALIASES = {
    "report_timing_summary": "timing_summary",
    "timing": "timing_summary",
    "report_utilization": "utilization",
    "report_power": "power",
    "report_clock_utilization": "clock_utilization",
    "report_control_sets": "control_sets",
    "report_drc": "drc",
    "report_methodology": "methodology",
}


# ---------------------------------------------------------------------------
# Small value helpers
# ---------------------------------------------------------------------------

def _to_float(raw):
    """Parse a number like '1,234', '12.5%', '-', 'NA' into a float or None."""
    if raw is None:
        return None
    s = str(raw).strip().rstrip("%").replace(",", "")
    if s in ("", "-", "--", "NA", "N/A", "Unspecified", "unspecified"):
        return None
    try:
        return float(s)
    except ValueError:
        return None


def _to_int(raw):
    """Parse an integer count, tolerating '12.0', commas and blanks."""
    value = _to_float(raw)
    if value is None:
        return None
    return int(value)


def _norm_name(raw):
    """Normalize a table row label for matching: lower case, one space."""
    return re.sub(r"\s+", " ", str(raw).strip().strip("*").lower())


def _snake(raw):
    """Turn a header cell like 'Global Clocks' into 'global_clocks'."""
    s = re.sub(r"[^0-9a-zA-Z]+", "_", str(raw).strip().lower())
    return s.strip("_")


# ---------------------------------------------------------------------------
# Generic 2017.3 text structure: setext headings and +-bordered tables
# ---------------------------------------------------------------------------

_BORDER_RE = re.compile(r"^\+[-+]+\+$")


def _is_border(line):
    return bool(_BORDER_RE.match(line.strip()))


def _split_row(line):
    return [cell.strip() for cell in line.strip().strip("|").split("|")]


def _ascii_tables(text):
    """Return every +-bordered table as {'header': [rows], 'rows': [rows]}.

    Header rows sit between the opening border and the first inner border;
    data rows sit between that border and the closing border.
    """
    lines = (text or "").splitlines()
    tables = []
    i = 0
    n = len(lines)
    while i < n:
        if not _is_border(lines[i]):
            i += 1
            continue
        i += 1  # opening border
        header = []
        while i < n and lines[i].strip().startswith("|"):
            header.append(_split_row(lines[i]))
            i += 1
        if i < n and _is_border(lines[i]):
            i += 1  # separator border
        data = []
        while i < n and lines[i].strip().startswith("|"):
            data.append(_split_row(lines[i]))
            i += 1
        if i < n and _is_border(lines[i]):
            i += 1  # closing border
        if header:
            tables.append({"header": header, "rows": data})
    return tables


def _flat_header(header_rows):
    """Merge one or two header rows into a single list of column names."""
    if not header_rows:
        return []
    if len(header_rows) == 1:
        return list(header_rows[0])
    width = max(len(row) for row in header_rows)
    out = []
    for col in range(width):
        parts = []
        for row in header_rows:
            if col < len(row):
                cell = row[col].strip()
                if cell:
                    parts.append(cell)
        out.append(" ".join(parts))
    return out


def _sections(text):
    """Split text into (title, body) pairs on setext headings.

    A heading is a non-empty line followed by a line of only '-' or '='
    (at least three characters), which is how Vivado 2017.3 lays out
    report sections such as 'Design Timing Summary' or '1. Slice Logic'.
    Text before the first heading is returned under the empty title.
    """
    lines = (text or "").splitlines()
    chunks = []
    title = ""
    body = []
    i = 0
    n = len(lines)
    while i < n:
        line = lines[i]
        if (
            line.strip()
            and i + 1 < n
            and re.match(r"^[-=]{3,}\s*$", lines[i + 1].strip())
        ):
            chunks.append((title, "\n".join(body)))
            title = line.strip()
            body = []
            i += 2
            continue
        body.append(line)
        i += 1
    chunks.append((title, "\n".join(body)))
    return chunks


def _section_body(text, *needles):
    """Body of the first section whose title contains any needle, else ''."""
    for title, body in _sections(text):
        lowered = title.lower()
        if any(needle.lower() in lowered for needle in needles):
            return body
    return ""


# ---------------------------------------------------------------------------
# report_timing_summary
# ---------------------------------------------------------------------------

def parse_timing_summary(text):
    """WNS, TNS, WHS, THS, TPWS, failing/total endpoints, per-clock table."""
    text = text or ""
    out = {
        "wns": None,
        "tns": None,
        "whs": None,
        "ths": None,
        "wpws": None,
        "tpws": None,
        "failing_endpoints": {"tns": None, "ths": None, "tpws": None},
        "total_endpoints": {"tns": None, "ths": None, "tpws": None},
        "clocks": None,
    }
    lines = text.splitlines()
    for idx, line in enumerate(lines):
        if "WNS(ns)" not in line or "TNS(ns)" not in line:
            continue
        for cand in lines[idx + 1:]:
            stripped = cand.strip()
            if not stripped or set(stripped) <= set("- "):
                continue
            parts = stripped.split()
            if len(parts) < 12:
                break
            out["wns"] = _to_float(parts[0])
            out["tns"] = _to_float(parts[1])
            out["failing_endpoints"]["tns"] = _to_int(parts[2])
            out["total_endpoints"]["tns"] = _to_int(parts[3])
            out["whs"] = _to_float(parts[4])
            out["ths"] = _to_float(parts[5])
            out["failing_endpoints"]["ths"] = _to_int(parts[6])
            out["total_endpoints"]["ths"] = _to_int(parts[7])
            out["wpws"] = _to_float(parts[8])
            out["tpws"] = _to_float(parts[9])
            out["failing_endpoints"]["tpws"] = _to_int(parts[10])
            out["total_endpoints"]["tpws"] = _to_int(parts[11])
            break
        break

    clock_body = _section_body(text, "Clock Summary")
    clocks = []
    clock_re = re.compile(
        r"^\s*(\S+)\s+\{([^}]*)\}\s+(\S+)\s+(\S+)\s*$"
    )
    for line in clock_body.splitlines():
        m = clock_re.match(line)
        if not m:
            continue
        wave = [_to_float(tok) for tok in m.group(2).split()]
        clocks.append(
            {
                "clock": m.group(1),
                "waveform_ns": wave,
                "period_ns": _to_float(m.group(3)),
                "frequency_mhz": _to_float(m.group(4)),
            }
        )
    if clocks:
        out["clocks"] = clocks
    elif clock_body.strip():
        out["clocks"] = []
    return out


# ---------------------------------------------------------------------------
# report_utilization
# ---------------------------------------------------------------------------

# Wanted site-type rows: normalized label -> output key. A label matches when
# it equals the key, except ramb36/ramb18 which match on a prefix so that
# 'RAMB36/FIFO*' counts as ramb36.
_UTIL_WANTED = {
    "slice luts": "slice_luts",
    "lut as logic": "lut_as_logic",
    "lut as memory": "lut_as_memory",
    "slice registers": "slice_registers",
    "f7 muxes": "f7_muxes",
    "f8 muxes": "f8_muxes",
    "block ram tile": "block_ram_tile",
    "ramb36": "ramb36",
    "ramb18": "ramb18",
    "dsps": "dsps",
    "bonded iob": "bonded_iob",
    "bufgctrl": "bufgctrl",
    "mmcme2_adv": "mmcme2_adv",
}


def _util_key(label):
    name = _norm_name(label)
    if name in _UTIL_WANTED:
        return _UTIL_WANTED[name]
    if name.startswith("ramb36"):
        return "ramb36"
    if name.startswith("ramb18"):
        return "ramb18"
    return None


def parse_utilization(text):
    """Used / available / percent for the site types the task lists."""
    out = {key: None for key in _UTIL_WANTED.values()}
    for table in _ascii_tables(text):
        header = [_norm_name(cell) for cell in _flat_header(table["header"])]
        used_idx = next(
            (i for i, h in enumerate(header) if h == "used"), 1
        )
        avail_idx = next(
            (i for i, h in enumerate(header) if h == "available"), 3
        )
        pct_idx = next(
            (i for i, h in enumerate(header) if "util" in h or "%" in h), 4
        )
        for row in table["rows"]:
            if not row:
                continue
            key = _util_key(row[0])
            if key is None or out[key] is not None:
                continue
            out[key] = {
                "used": _to_int(row[used_idx]) if used_idx < len(row) else None,
                "available": _to_int(row[avail_idx])
                if avail_idx < len(row)
                else None,
                "percent": _to_float(row[pct_idx])
                if pct_idx < len(row)
                else None,
            }
    return out


# ---------------------------------------------------------------------------
# report_power
# ---------------------------------------------------------------------------

_POWER_LINES = (
    (r"Total On-Chip Power \(W\):\s*([0-9.,]+)", "total_on_chip_w"),
    (r"Dynamic \(W\):\s*([0-9.,]+)", "dynamic_w"),
    (r"Device Static \(W\):\s*([0-9.,]+)", "device_static_w"),
    (r"Junction Temperature \(C\):\s*([0-9.,-]+)", "junction_temp_c"),
)


def parse_power(text):
    """Headline watts, junction temperature, confidence, on-chip breakdown."""
    text = text or ""
    out = {
        "total_on_chip_w": None,
        "dynamic_w": None,
        "device_static_w": None,
        "junction_temp_c": None,
        "confidence_level": None,
        "breakdown": None,
    }
    for pattern, key in _POWER_LINES:
        m = re.search(pattern, text)
        if m:
            out[key] = _to_float(m.group(1))
    m = re.search(r"Confidence Level:\s*(\S+)", text)
    if m:
        out["confidence_level"] = m.group(1)

    body = _section_body(text, "On-Chip Power")
    for table in _ascii_tables(body or text):
        header = [_norm_name(cell) for cell in _flat_header(table["header"])]
        power_idx = next(
            (i for i, h in enumerate(header) if "power" in h), None
        )
        if power_idx is None:
            continue
        pct_idx = next(
            (i for i, h in enumerate(header) if "%" in h), None
        )
        rows = []
        for row in table["rows"]:
            if not row or not row[0].strip():
                continue
            rows.append(
                {
                    "name": row[0].strip(),
                    "power_w": _to_float(row[power_idx])
                    if power_idx < len(row)
                    else None,
                    "percent": _to_float(row[pct_idx])
                    if pct_idx is not None and pct_idx < len(row)
                    else None,
                }
            )
        if rows:
            out["breakdown"] = rows
            break
    return out


# ---------------------------------------------------------------------------
# report_clock_utilization
# ---------------------------------------------------------------------------

def parse_clock_utilization(text):
    """Global clocks per clock region, from the region table."""
    out = {"regions": None}
    for table in _ascii_tables(text):
        header = _flat_header(table["header"])
        lowered = [_norm_name(cell) for cell in header]
        if not any("clock region" in h for h in lowered):
            continue
        region_idx = next(
            i for i, h in enumerate(lowered) if "clock region" in h
        )
        keys = [_snake(cell) for cell in header]
        regions = []
        for row in table["rows"]:
            if not row or region_idx >= len(row):
                continue
            entry = {"region": row[region_idx].strip()}
            for i, cell in enumerate(row):
                if i == region_idx or i >= len(keys) or not keys[i]:
                    continue
                value = _to_int(cell)
                if value is None:
                    value = _to_float(cell)
                entry[keys[i]] = value if value is not None else cell.strip()
            regions.append(entry)
        if regions:
            out["regions"] = regions
            break
    return out


# ---------------------------------------------------------------------------
# report_control_sets
# ---------------------------------------------------------------------------

def parse_control_sets(text):
    """Total and unique control sets."""
    text = text or ""
    out = {"total": None, "unique": None}
    m = re.search(
        r"Total (?:number of )?control sets:?\s*(\d+)", text, re.I
    )
    if m:
        out["total"] = int(m.group(1))
    m = re.search(r"Unique control sets:?\s*(\d+)", text, re.I)
    if m:
        out["unique"] = int(m.group(1))
    if out["total"] is None:
        body = _section_body(text, "Control Sets")
        for table in _ascii_tables(body):
            if table["rows"]:
                out["total"] = len(table["rows"])
                break
    return out


# ---------------------------------------------------------------------------
# report_drc and report_methodology
# ---------------------------------------------------------------------------

def _parse_rule_table(text, no_violations_marker):
    """Rules from a Rule/Severity/Count table, or None when absent.

    Returns [] when the report explicitly says there are no violations.
    """
    text = text or ""
    for table in _ascii_tables(text):
        header = [_norm_name(cell) for cell in _flat_header(table["header"])]
        rule_idx = next(
            (
                i
                for i, h in enumerate(header)
                if "rule" in h or "check" in h or h == "id"
            ),
            None,
        )
        sev_idx = next(
            (i for i, h in enumerate(header) if "severity" in h), None
        )
        count_idx = next(
            (
                i
                for i, h in enumerate(header)
                if "count" in h or "number" in h or "#" in h
            ),
            None,
        )
        if rule_idx is None or sev_idx is None or count_idx is None:
            continue
        rules = []
        for row in table["rows"]:
            if not row or rule_idx >= len(row):
                continue
            rule_id = row[rule_idx].strip()
            if not rule_id:
                continue
            rules.append(
                {
                    "rule_id": rule_id,
                    "severity": row[sev_idx].strip()
                    if sev_idx < len(row)
                    else None,
                    "count": _to_int(row[count_idx])
                    if count_idx < len(row)
                    else None,
                }
            )
        return rules
    if re.search(no_violations_marker, text, re.I):
        return []
    return None


def parse_drc(text):
    """DRC rule violations: rule ID, severity, count."""
    return {"rules": _parse_rule_table(text, r"no drc violations")}


def parse_methodology(text):
    """Methodology rule violations: rule ID, severity, count."""
    return {
        "rules": _parse_rule_table(
            text, r"no methodology violations|no violations found"
        )
    }


# ---------------------------------------------------------------------------
# Dispatch and CLI
# ---------------------------------------------------------------------------

_PARSERS = {
    "timing_summary": parse_timing_summary,
    "utilization": parse_utilization,
    "power": parse_power,
    "clock_utilization": parse_clock_utilization,
    "control_sets": parse_control_sets,
    "drc": parse_drc,
    "methodology": parse_methodology,
}


def parse_report(kind, text):
    """Parse one report. kind accepts the report_ prefix and short names."""
    key = _ALIASES.get(str(kind).strip().lower(), str(kind).strip().lower())
    try:
        fn = _PARSERS[key]
    except KeyError:
        raise ValueError(f"unknown report kind: {kind!r}") from None
    return fn(text)


def parse_all(text_by_kind):
    """Parse several reports at once: {kind: text} -> {kind: result}."""
    return {
        kind: parse_report(kind, text)
        for kind, text in (text_by_kind or {}).items()
    }


def main(argv=None):
    ap = argparse.ArgumentParser(
        description="Parse a Vivado 2017.3 text report into JSON."
    )
    ap.add_argument("kind", help="report kind, e.g. timing_summary")
    ap.add_argument("file", help="report file, or - for stdin")
    args = ap.parse_args(argv)
    try:
        result = parse_report(args.kind, _read_input(args.file))
    except ValueError as exc:
        ap.error(str(exc))
    print(json.dumps(result, indent=2))
    return 0


def _read_input(path):
    if path == "-":
        return sys.stdin.read()
    with open(path, encoding="utf-8", errors="replace") as handle:
        return handle.read()


if __name__ == "__main__":
    sys.exit(main())

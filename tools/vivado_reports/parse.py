#!/usr/bin/env python3
"""Turn Vivado 2017.3 text reports into JSON.

    python3 tools/vivado_reports/parse.py KIND FILE
    python3 tools/vivado_reports/parse.py KIND -   # read stdin

KIND is one of: timing_summary, utilization, power, clock_utilization,
control_sets, drc, methodology (the report_ prefix is also accepted).

Every report parses on its own. A missing section becomes null, never a
crash: an empty input yields a dict of nulls, and a report with only some
sections yields nulls for the absent ones.

Format facts, verified against real 2017.3 reports from the cluster:
- Every report starts with a Table of Contents that repeats the section
  titles. A section heading counts only where the next line is a dash
  underline; TOC lines are never headings.
- Power headlines live in a two-column pipe table in section 1. Summary
  (label, value); the on-chip breakdown is section 1.1 On-Chip Components
  with columns On-Chip, Power (W), Used, Available, Utilization (%).
- The Intra Clock Table is whitespace-aligned, not pipes. Column positions
  come from the dash row, because a clock row may leave the setup and hold
  columns blank and fill only the pulse width ones.
- Control sets: section 1. Summary holds a Status and Count table whose
  rows are "Number of unique control sets" and "Unused register locations
  in slices containing registers". There is no total row.
- DRC and methodology: under 1. REPORT SUMMARY come key: value lines, then
  a pipe table with columns Rule, Severity, Description, Violations.
- Clock utilization: region names look like X1Y0 and come from sections
  4. Clock Regions: Key Resource Utilization and 5. Clock Regions :
  Global Clock Summary.

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
    data rows sit between that border and the closing border. A table with
    no inner border (a bare label/value table) has all rows in 'header'
    and none in 'rows'; use _data_rows to read those.
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


def _data_rows(table, markers=()):
    """Data rows of a table, tolerating a missing separator border.

    When the table has an inner border, 'rows' already holds the data.
    When it has none, every row landed in 'header'; then the first row is
    the column-name row when it contains one of the marker words, and the
    remaining rows are the data.
    """
    rows = table["rows"]
    if rows:
        return [list(row) for row in rows]
    header = table["header"]
    if len(header) > 1:
        first = " ".join(_norm_name(cell) for cell in header[0])
        if any(marker in first for marker in markers):
            return [list(row) for row in header[1:]]
    return []


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
    report sections such as '1. Summary' or 'Design Timing Summary'.
    The Table of Contents is dropped: it repeats the section titles, and
    its lines must never be mistaken for headings or content.
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
    return [
        (t, b)
        for (t, b) in chunks
        if _norm_name(t) != "table of contents"
    ]


def _section_body(text, *needles):
    """Body of the section whose title contains any needle, else ''.

    When a title appears more than once (the Table of Contents repeats
    every title), the last non-empty match wins: the real section follows
    the TOC.
    """
    matches = [
        (title, body)
        for title, body in _sections(text)
        if any(needle.lower() in title.lower() for needle in needles)
    ]
    if not matches:
        return ""
    for _title, body in reversed(matches):
        if body.strip():
            return body
    return matches[-1][1]


def _dash_runs(line):
    """(start, end) spans of each run of '-' in a dash row."""
    return [(m.start(), m.end()) for m in re.finditer(r"-+", line)]


def _parse_whitespace_table(lines, start_idx, required):
    """Parse a whitespace-aligned table at lines[start_idx].

    lines[start_idx] is the header, lines[start_idx + 1] the dash row.
    Column positions come from the dash runs, so a row may leave columns
    blank. required is a list of lowercase substrings that must all appear
    in the column names. Returns (rows, next_idx) with rows as dicts keyed
    by snake column name, or (None, start_idx) when the header does not
    match.
    """
    if start_idx + 1 >= len(lines):
        return None, start_idx
    header_line = lines[start_idx]
    runs = _dash_runs(lines[start_idx + 1])
    if len(runs) < 2:
        return None, start_idx
    bounds = [run[0] for run in runs] + [len(header_line)]
    names = [
        header_line[bounds[i]:bounds[i + 1]].strip()
        for i in range(len(runs))
    ]
    lowered = [name.lower() for name in names]
    if not all(any(req in name for name in lowered) for req in required):
        return None, start_idx
    rows = []
    i = start_idx + 2
    while i < len(lines):
        line = lines[i]
        if not line.strip():
            break
        if set(line.strip()) <= set("-="):
            break
        if i + 1 < len(lines) and re.match(
            r"^[-=]{3,}\s*$", lines[i + 1].strip()
        ):
            break
        cells = [
            line[bounds[j]:bounds[j + 1]].strip()
            if bounds[j] < len(line)
            else ""
            for j in range(len(runs))
        ]
        if not any(cells):
            i += 1
            continue
        rows.append(
            {_snake(names[j]) or "col_%d" % j: cells[j] for j in range(len(runs))}
        )
        i += 1
    return rows, i


def _find_whitespace_table(text, required):
    """First whitespace table in text whose header holds all required names."""
    lines = (text or "").splitlines()
    for idx, line in enumerate(lines):
        if "WNS(ns)" not in line:
            continue
        rows, _ = _parse_whitespace_table(lines, idx, required)
        if rows is not None:
            return rows
    return None


# ---------------------------------------------------------------------------
# report_timing_summary
# ---------------------------------------------------------------------------

_INTRA_COLMAP = {
    "clock": ("clock", "str"),
    "wns_ns": ("wns", "float"),
    "tns_ns": ("tns", "float"),
    "tns_failing_endpoints": ("tns_failing_endpoints", "int"),
    "tns_total_endpoints": ("tns_total_endpoints", "int"),
    "whs_ns": ("whs", "float"),
    "ths_ns": ("ths", "float"),
    "ths_failing_endpoints": ("ths_failing_endpoints", "int"),
    "ths_total_endpoints": ("ths_total_endpoints", "int"),
    "wpws_ns": ("wpws", "float"),
    "tpws_ns": ("tpws", "float"),
    "tpws_failing_endpoints": ("tpws_failing_endpoints", "int"),
    "tpws_total_endpoints": ("tpws_total_endpoints", "int"),
}


def parse_timing_summary(text):
    """WNS, TNS, WHS, THS, TPWS, failing/total endpoints, per-clock tables."""
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
        "intra_clock": None,
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
            if len(parts) < 12 or _to_float(parts[0]) is None:
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
        if out["wns"] is not None:
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

    intra_body = _section_body(text, "Intra Clock Table")
    intra_rows = _find_whitespace_table(
        intra_body if intra_body.strip() else text,
        ("wns(ns)", "tns(ns)", "whs(ns)", "ths(ns)", "wpws(ns)", "tpws(ns)",
         "clock"),
    )
    if intra_rows is not None:
        parsed = []
        for row in intra_rows:
            entry = {key: None for _key, (key, _kind) in _INTRA_COLMAP.items()}
            for snake, raw in row.items():
                mapped = _INTRA_COLMAP.get(snake)
                if mapped is None:
                    continue
                key, kind = mapped
                if kind == "str":
                    entry[key] = raw or None
                elif kind == "int":
                    entry[key] = _to_int(raw)
                else:
                    entry[key] = _to_float(raw)
            parsed.append(entry)
        out["intra_clock"] = parsed
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
        for row in _data_rows(table, ("site type", "used")):
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

_POWER_LABELS = {
    "total on-chip power (w)": "total_on_chip_w",
    "dynamic (w)": "dynamic_w",
    "device static (w)": "device_static_w",
    "junction temperature (c)": "junction_temp_c",
}

# Fallback for other Vivado versions that print colon-form lines.
_POWER_LINES = (
    (r"Total On-Chip Power \(W\):\s*([0-9.,]+)", "total_on_chip_w"),
    (r"Dynamic \(W\):\s*([0-9.,]+)", "dynamic_w"),
    (r"Device Static \(W\):\s*([0-9.,]+)", "device_static_w"),
    (r"Junction Temperature \(C\):\s*([0-9.,-]+)", "junction_temp_c"),
)


def _label_value_rows(text):
    """All (label, value) pipe rows in text, from every table."""
    rows = []
    for table in _ascii_tables(text):
        for row in table["header"] + table["rows"]:
            if len(row) >= 2:
                rows.append((row[0], row[1]))
    return rows


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
    summary = _section_body(text, "1. Summary")
    if not summary.strip():
        summary = _section_body(text, "Summary")
    search = summary if summary.strip() else text
    for label, value in _label_value_rows(search):
        key = _POWER_LABELS.get(_norm_name(label))
        if key is not None and out[key] is None:
            out[key] = _to_float(value)
        elif _norm_name(label) == "confidence level":
            out["confidence_level"] = value.strip() or None
    if out["total_on_chip_w"] is None:
        for pattern, key in _POWER_LINES:
            m = re.search(pattern, text)
            if m:
                out[key] = _to_float(m.group(1))
    if out["confidence_level"] is None:
        m = re.search(r"Confidence Level:\s*(\S+)", text)
        if m:
            out["confidence_level"] = m.group(1)

    body = _section_body(text, "On-Chip Components")
    for table in _ascii_tables(body if body.strip() else text):
        header = [_norm_name(cell) for cell in _flat_header(table["header"])]
        power_idx = next(
            (i for i, h in enumerate(header) if h == "power (w)"), None
        )
        if power_idx is None:
            power_idx = next(
                (i for i, h in enumerate(header) if "power" in h), None
            )
        if power_idx is None:
            continue
        pct_idx = next(
            (i for i, h in enumerate(header) if "utilization" in h or "%" in h),
            None,
        )
        rows = []
        for row in _data_rows(table, ("on-chip", "power (w)")):
            if not row or not row[0].strip():
                continue
            if _norm_name(row[0]) in ("on-chip", "power (w)"):
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
        if rows and any(row["power_w"] is not None for row in rows):
            out["breakdown"] = rows
            break
    return out


# ---------------------------------------------------------------------------
# report_clock_utilization
# ---------------------------------------------------------------------------

_REGION_RE = re.compile(r"^X\d+Y\d+$")


def parse_clock_utilization(text):
    """Global clocks per clock region, from the Clock Regions sections."""
    text = text or ""
    regions = {}
    order = []

    def _merge(table):
        header = _flat_header(table["header"])
        lowered = [_norm_name(cell) for cell in header]
        rows = _data_rows(table, ("clock region", "region"))
        if not rows:
            return
        region_idx = None
        for col in range(max((len(row) for row in rows), default=0)):
            if any(
                col < len(row) and _REGION_RE.match(row[col].strip())
                for row in rows
            ):
                region_idx = col
                break
        if region_idx is None:
            region_idx = next(
                (i for i, h in enumerate(lowered) if "region" in h), 0
            )
        keys = [_snake(cell) for cell in header]
        for row in rows:
            if region_idx >= len(row):
                continue
            name = row[region_idx].strip()
            if not _REGION_RE.match(name):
                continue
            entry = regions.setdefault(name, {"region": name})
            if name not in order:
                order.append(name)
            for i, cell in enumerate(row):
                if i == region_idx or i >= len(keys) or not keys[i]:
                    continue
                if keys[i] in entry:
                    continue
                value = _to_int(cell)
                if value is None:
                    value = _to_float(cell)
                entry[keys[i]] = value if value is not None else cell.strip()

    sections = [
        body
        for title, body in _sections(text)
        if "clock regions" in title.lower()
    ]
    for body in sections:
        for table in _ascii_tables(body):
            _merge(table)
    if not regions:
        for table in _ascii_tables(text):
            _merge(table)
    if not regions:
        return {"regions": None}
    return {"regions": [regions[name] for name in order]}


# ---------------------------------------------------------------------------
# report_control_sets
# ---------------------------------------------------------------------------

def parse_control_sets(text):
    """Total and unique control sets, plus unused register locations.

    The 2017.3 report has no total row: the Status and Count table in
    section 1. Summary lists "Number of unique control sets" and
    "Unused register locations in slices containing registers".
    """
    text = text or ""
    out = {"total": None, "unique": None, "unused_register_locations": None}
    summary = _section_body(text, "1. Summary")
    if not summary.strip():
        summary = _section_body(text, "Summary")
    search = summary if summary.strip() else text
    for label, value in _label_value_rows(search):
        norm = _norm_name(label)
        if "unique control sets" in norm and out["unique"] is None:
            out["unique"] = _to_int(value)
        elif (
            "unused register locations" in norm
            and out["unused_register_locations"] is None
        ):
            out["unused_register_locations"] = _to_int(value)
    if out["unique"] is None:
        m = re.search(r"Number of unique control sets:?\s*(\d+)", text, re.I)
        if m:
            out["unique"] = int(m.group(1))
    m = re.search(r"Total (?:number of )?control sets:?\s*(\d+)", text, re.I)
    if m:
        out["total"] = int(m.group(1))
    return out


# ---------------------------------------------------------------------------
# report_drc and report_methodology
# ---------------------------------------------------------------------------

_SUMMARY_KEYS = (
    ("netlist", "netlist", "str"),
    ("floorplan", "floorplan", "str"),
    ("design limits", "design_limits", "str"),
    ("ruledeck", "ruledeck", "str"),
    ("max violations", "max_violations", "int"),
    ("violations found", "violations_found", "int"),
)


def _parse_report_summary(text):
    """The key: value lines under 1. REPORT SUMMARY."""
    body = _section_body(text, "REPORT SUMMARY")
    search = body if body.strip() else (text or "")
    out = {key: None for _label, key, _kind in _SUMMARY_KEYS}
    for line in search.splitlines():
        m = re.match(r"^\s*([^:]+):\s*(.+?)\s*$", line)
        if not m:
            continue
        label = _norm_name(m.group(1))
        for want, key, kind in _SUMMARY_KEYS:
            if label == want:
                out[key] = (
                    _to_int(m.group(2))
                    if kind == "int"
                    else m.group(2).strip()
                )
    return out


def _parse_rule_table(text, no_violations_marker):
    """Rules from a Rule/Severity/Description/Violations table.

    Returns [] when the report says there are no violations, None when no
    rule table is present at all.
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
                if "violations" in h or "count" in h or "number" in h or "#" in h
            ),
            None,
        )
        if rule_idx is None or sev_idx is None or count_idx is None:
            continue
        desc_idx = next(
            (i for i, h in enumerate(header) if "description" in h), None
        )
        rules = []
        for row in _data_rows(table, ("rule", "severity")):
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
                    "description": row[desc_idx].strip()
                    if desc_idx is not None and desc_idx < len(row)
                    else None,
                    "count": _to_int(row[count_idx])
                    if count_idx < len(row)
                    else None,
                }
            )
        return rules
    summary = _parse_report_summary(text)
    if summary.get("violations_found") == 0:
        return []
    if re.search(no_violations_marker, text, re.I):
        return []
    return None


def parse_drc(text):
    """DRC rule violations: rule ID, severity, description, count."""
    return {
        "summary": _parse_report_summary(text),
        "rules": _parse_rule_table(text, r"no drc violations"),
    }


def parse_methodology(text):
    """Methodology rule violations: rule ID, severity, description, count."""
    return {
        "summary": _parse_report_summary(text),
        "rules": _parse_rule_table(
            text, r"no methodology violations|no violations found"
        ),
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

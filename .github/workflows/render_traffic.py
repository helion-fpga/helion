#!/usr/bin/env python3
"""Generate traffic/index.html with inline SVG charts from ndjson history."""
import json
from pathlib import Path

TRAFFIC = Path("traffic")

def load(kind):
    pts = []
    p = TRAFFIC / f"{kind}-history.ndjson"
    if p.exists():
        for line in p.read_text().splitlines():
            line = line.strip()
            if line:
                e = json.loads(line)
                pts.append((e["timestamp"], e["count"], e["uniques"]))
    return sorted(pts)

def svg_chart(title, pts, color="#2f80ed", color2="#a0a0a0"):
    W, H = 720, 260
    pad_l, pad_r, pad_t, pad_b = 48, 16, 20, 36
    cw, ch = W - pad_l - pad_r, H - pad_t - pad_b
    n = len(pts)
    maxv = max((p[1] for p in pts), default=1)
    maxv = max(maxv, 1)
    # nice y max
    y_max = maxv
    def x(i):
        return pad_l + (cw * i / max(n - 1, 1))
    def y(v):
        return pad_t + ch - (ch * v / y_max)
    # gridlines + y labels (4 ticks)
    grid = []
    for t in range(5):
        v = y_max * t / 4
        yy = y(v)
        grid.append(f'<line x1="{pad_l}" y1="{yy:.1f}" x2="{W-pad_r}" y2="{yy:.1f}" stroke="#e8e8e8"/>')
        grid.append(f'<text x="{pad_l-8}" y="{yy+4:.1f}" text-anchor="end" font-size="11" fill="#666">{v:.0f}</text>')
    # x labels: first, middle, last
    xlabels = []
    for i in ([0, n // 2, n - 1] if n > 2 else range(n)):
        if 0 <= i < n:
            xlabels.append(f'<text x="{x(i):.1f}" y="{H-12}" text-anchor="middle" font-size="11" fill="#666">{pts[i][0][5:]}</text>')
    def path(idx):
        d = " ".join(f"{'M' if i == 0 else 'L'}{x(i):.1f},{y(pts[i][idx]):.1f}" for i in range(n))
        return f'<path d="{d}" fill="none" stroke-width="2"/>'
    # area fill under total
    area = ""
    if n:
        d = " ".join(f"{'M' if i == 0 else 'L'}{x(i):.1f},{y(pts[i][1]):.1f}" for i in range(n))
        d += f" L{x(n-1):.1f},{y(0):.1f} L{x(0):.1f},{y(0):.1f} Z"
        area = f'<path d="{d}" fill="{color}" opacity="0.12"/>'
    dots = "".join(
        f'<circle cx="{x(i):.1f}" cy="{y(pts[i][1]):.1f}" r="3" fill="{color}"><title>{pts[i][0]}: {pts[i][1]} clones ({pts[i][2]} unique)</title></circle>'
        for i in range(n)
    )
    return f"""
<h2>{title}</h2>
<svg width="{W}" height="{H}" viewBox="0 0 {W} {H}" font-family="sans-serif">
{''.join(grid)}
{''.join(xlabels)}
{area}
<g stroke="{color2}">{path(2)}</g>
<g stroke="{color}">{path(1)}</g>
{dots}
<g font-size="12" fill="#333">
<circle cx="{pad_l}" cy="14" r="5" fill="{color}"/><text x="{pad_l+10}" y="18">total</text>
<circle cx="{pad_l+90}" cy="14" r="5" fill="{color2}"/><text x="{pad_l+100}" y="18">unique</text>
</g>
</svg>"""

def main():
    clones = load("clones")
    views = load("views")
    tc, tu = sum(p[1] for p in clones), sum(p[2] for p in clones)
    vc, vu = sum(p[1] for p in views), sum(p[2] for p in views)
    html = f"""<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>helion traffic</title>
<style>body{{font-family:sans-serif;max-width:760px;margin:2em auto;padding:0 1em;color:#222}}
h1{{font-size:1.4em}} .tot{{font-size:1.1em;color:#555}} svg{{max-width:100%;height:auto}}</style>
</head><body>
<h1>helion-fpga/helion traffic</h1>
<p class="tot">Clones (all time): <b>{tc}</b> ({tu} unique) &middot; Views: <b>{vc}</b> ({vu} unique) &middot; {len(clones)} days tracked</p>
<p>GitHub drops traffic data after 14 days; this page is rebuilt daily from <a href="clones-history.ndjson">clones-history.ndjson</a> and <a href="views-history.ndjson">views-history.ndjson</a>.</p>
{svg_chart("Clones per day", clones)}
{svg_chart("Views per day", views, color="#28a745", color2="#a0a0a0")}
</body></html>"""
    (TRAFFIC / "index.html").write_text(html)
    print(f"wrote index.html ({len(clones)} clone days, {len(views)} view days)")

if __name__ == "__main__":
    main()

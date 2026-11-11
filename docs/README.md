# Helion website

Static pages published to GitHub Pages at
<https://helion-fpga.github.io/helion/> by
[`.github/workflows/pages.yml`](../.github/workflows/pages.yml) on every push to
`master` that touches `docs/`. There is no build step: edit the HTML, open it
in a browser, commit.

| Page | Job |
|---|---|
| `index.html` | Landing page: what Helion is, the IDE, features, devices, download |
| `start.html` | Install a release or build from source; first run |
| `use.html` | User guide for the IDE (`helion-ide`) and the CLI (`helion`) |
| `architecture.html` | Pipeline, crate map, HAD parts, HNF, constraints, Tcl |
| `benchmarks.html` | Measured comparison against Yosys + nextpnr, with caveats |
| `roadmap.html` | What 4.0 delivers and what comes next (mirrors `ROADMAP.md`) |
| `wiki.html` | Documentation index |
| `get-involved.html` | On-ramp for new contributors |
| `contribute.html` | Code map and pull-request rules |
| `legal.html` | License, trademarks, and the legal fence |
| `404.html` | Not-found page |

Shared assets: `style.css` (design tokens, light and dark themes), `site.js`
(navigation, theme toggle), `brand/` (logos, marks, social image),
`screenshots/` (IDE screenshots used by the README and the site).

Reference docs published alongside the site:

- [`tcl-subset.md`](tcl-subset.md): the Tcl language and commands the IDE console supports
- [`benchmarks/`](benchmarks/): full benchmark reports

Internal engineering notes live in [`notes/`](../notes/) at the repository
root and are not published.

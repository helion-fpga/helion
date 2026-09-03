/* Shared nav current-page + HL10T-C32-1 die (HAD grid, not a stock FPGA photo). */
(function () {
  var path = (location.pathname.split("/").pop() || "index.html");
  if (path === "") path = "index.html";
  document.querySelectorAll("nav a").forEach(function (a) {
    var href = a.getAttribute("href") || "";
    if (href === path || (path === "index.html" && href === "index.html")) {
      a.setAttribute("aria-current", "page");
    }
  });

  var host = document.getElementById("die");
  if (!host) return;

  /* devices/helion/parts/HL10T-C32-1.toml */
  var COLS = 35; /* x=0 IO, x=1 CLK, x=2..33 CLB, x=34 IO */
  var ROWS = 34; /* y=0 IO, y=1..32 interior, y=33 IO */
  var NS = "http://www.w3.org/2000/svg";
  var svg = document.createElementNS(NS, "svg");
  svg.setAttribute("viewBox", "0 0 " + COLS + " " + ROWS);
  svg.setAttribute("role", "img");
  svg.setAttribute("aria-label", "HL10T-C32-1 floorplan: IO ring, clock spine, 32 by 32 CLB");

  var read = document.getElementById("die-read");

  function kind(x, y) {
    if (x === 0 || x === COLS - 1 || y === 0 || y === ROWS - 1) return "IO";
    if (x === 1) return "CLK";
    return "CLB";
  }

  function fill(k) {
    if (k === "IO") return "#c47a3a";
    if (k === "CLK") return "#e6b325";
    return "#163226";
  }

  function tile(x, y, k) {
    var r = document.createElementNS(NS, "rect");
    r.setAttribute("x", String(x));
    r.setAttribute("y", String(y));
    r.setAttribute("width", "1");
    r.setAttribute("height", "1");
    r.setAttribute("fill", fill(k));
    r.setAttribute("data-x", String(x));
    r.setAttribute("data-y", String(y));
    r.setAttribute("data-k", k);
    svg.appendChild(r);
    return r;
  }

  var later = [];
  for (var y = 0; y < ROWS; y++) {
    for (var x = 0; x < COLS; x++) {
      var k = kind(x, y);
      if (k === "CLK") later.push([x, y, k]);
      else tile(x, y, k);
    }
  }
  later.forEach(function (t) { tile(t[0], t[1], t[2]); });

  function setRead(t) {
    if (read) read.textContent = t;
  }

  svg.addEventListener("pointermove", function (ev) {
    var t = ev.target;
    if (!t || !t.getAttribute) return;
    var k = t.getAttribute("data-k");
    if (!k) return;
    setRead(
      k +
        "  x=" +
        t.getAttribute("data-x") +
        "  y=" +
        t.getAttribute("data-y") +
        "  part=HL10T-C32-1"
    );
  });
  svg.addEventListener("pointerleave", function () {
    setRead("IO ring · clock spine x=1 · CLB x=2..33 y=1..32 · hover a tile");
  });

  host.appendChild(svg);
  setRead("IO ring · clock spine x=1 · CLB x=2..33 y=1..32 · hover a tile");
})();

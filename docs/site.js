/* Nav current-page + live HL10T-C32-1 floorplan (HAD grid). */
(function () {
  var path = location.pathname.split("/").pop() || "index.html";
  if (path === "") path = "index.html";
  document.querySelectorAll("nav a").forEach(function (a) {
    var href = a.getAttribute("href") || "";
    if (href === path) a.setAttribute("aria-current", "page");
  });

  var host = document.getElementById("die");
  if (!host) return;

  /* devices/helion/parts/HL10T-C32-1.toml */
  var COLS = 35; /* x=0 IO, x=1 CLK, x=2..33 CLB, x=34 IO */
  var ROWS = 34; /* y=0 IO, y=1..32 interior, y=33 IO */
  var NS = "http://www.w3.org/2000/svg";
  var reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  var svg = document.createElementNS(NS, "svg");
  svg.setAttribute("viewBox", "0 0 " + COLS + " " + ROWS);
  svg.setAttribute("role", "img");
  svg.setAttribute("aria-label", "Live HL10T-C32-1 floorplan. Hover a tile.");
  svg.setAttribute("preserveAspectRatio", "xMidYMid meet");

  var defs = document.createElementNS(NS, "defs");
  defs.innerHTML =
    '<filter id="clkGlow" x="-40%" y="-40%" width="180%" height="180%">' +
    '<feGaussianBlur stdDeviation="0.18" result="b"/>' +
    '<feMerge><feMergeNode in="b"/><feMergeNode in="SourceGraphic"/></feMerge>' +
    "</filter>";
  svg.appendChild(defs);

  function kind(x, y) {
    if (x === 0 || x === COLS - 1 || y === 0 || y === ROWS - 1) return "IO";
    if (x === 1) return "CLK";
    return "CLB";
  }

  function fill(k, x, y) {
    if (k === "IO") return "#c47a3a";
    if (k === "CLK") return "#e6b325";
    return (x + y) % 2 === 0 ? "#1a3a2c" : "#132920";
  }

  for (var y = 0; y < ROWS; y++) {
    for (var x = 0; x < COLS; x++) {
      var k = kind(x, y);
      var r = document.createElementNS(NS, "rect");
      r.setAttribute("x", (x + 0.08).toFixed(2));
      r.setAttribute("y", (y + 0.08).toFixed(2));
      r.setAttribute("width", "0.84");
      r.setAttribute("height", "0.84");
      r.setAttribute("fill", fill(k, x, y));
      r.setAttribute("data-k", k);
      if (k === "CLK") r.setAttribute("filter", "url(#clkGlow)");
      svg.appendChild(r);
    }
  }

  var guides = document.createElementNS(NS, "g");
  guides.setAttribute("stroke", "rgba(235,230,212,0.35)");
  guides.setAttribute("stroke-width", "0.06");
  var gx = document.createElementNS(NS, "line");
  var gy = document.createElementNS(NS, "line");
  gx.setAttribute("y1", "0");
  gx.setAttribute("y2", String(ROWS));
  gy.setAttribute("x1", "0");
  gy.setAttribute("x2", String(COLS));
  guides.appendChild(gx);
  guides.appendChild(gy);
  guides.style.display = "none";
  svg.appendChild(guides);

  var hi = document.createElementNS(NS, "rect");
  hi.setAttribute("width", "1");
  hi.setAttribute("height", "1");
  hi.setAttribute("fill", "none");
  hi.setAttribute("stroke", "#ebe6d4");
  hi.setAttribute("stroke-width", "0.12");
  hi.style.display = "none";
  svg.appendChild(hi);

  var pin = document.createElementNS(NS, "rect");
  pin.setAttribute("width", "1");
  pin.setAttribute("height", "1");
  pin.setAttribute("fill", "none");
  pin.setAttribute("stroke", "#e6b325");
  pin.setAttribute("stroke-width", "0.14");
  pin.style.display = "none";
  svg.appendChild(pin);

  var pulse = document.createElementNS(NS, "rect");
  pulse.setAttribute("class", "clk-pulse");
  pulse.setAttribute("x", "1.12");
  pulse.setAttribute("width", "0.76");
  pulse.setAttribute("height", "2.2");
  pulse.setAttribute("fill", "#fff4b0");
  pulse.setAttribute("opacity", "0.55");
  pulse.setAttribute("rx", "0.12");
  if (!reduce) svg.appendChild(pulse);

  var read = document.getElementById("die-read");
  var pinned = null;

  function cellAt(ev) {
    var ctm = svg.getScreenCTM();
    if (!ctm) return null;
    var pt = svg.createSVGPoint();
    pt.x = ev.clientX;
    pt.y = ev.clientY;
    pt = pt.matrixTransform(ctm.inverse());
    var x = Math.floor(pt.x);
    var y = Math.floor(pt.y);
    if (x < 0 || y < 0 || x >= COLS || y >= ROWS) return null;
    return { x: x, y: y, k: kind(x, y) };
  }

  function describe(c, extra) {
    var bits = [c.k, "x=" + c.x, "y=" + c.y, "part=HL10T-C32-1"];
    if (c.k === "CLB") bits.push("BLE×8");
    if (c.k === "IO") bits.push("user_io");
    if (c.k === "CLK") bits.push("gclk spine");
    if (extra) bits.push(extra);
    return bits.join("  ·  ");
  }

  function setRead(t) {
    if (read) read.textContent = t;
  }

  function idle() {
    if (pinned) {
      setRead(describe(pinned, "pinned"));
      return;
    }
    setRead("IO ring · clock spine x=1 · CLB x=2..33 y=1..32 · hover a tile, click to pin");
  }

  function showHi(c) {
    hi.setAttribute("x", String(c.x));
    hi.setAttribute("y", String(c.y));
    hi.style.display = "";
    gx.setAttribute("x1", String(c.x + 0.5));
    gx.setAttribute("x2", String(c.x + 0.5));
    gy.setAttribute("y1", String(c.y + 0.5));
    gy.setAttribute("y2", String(c.y + 0.5));
    guides.style.display = "";
  }

  svg.addEventListener("pointermove", function (ev) {
    var c = cellAt(ev);
    if (!c) {
      hi.style.display = "none";
      guides.style.display = "none";
      idle();
      return;
    }
    showHi(c);
    setRead(describe(c, pinned && pinned.x === c.x && pinned.y === c.y ? "pinned" : ""));
  });

  svg.addEventListener("pointerleave", function () {
    hi.style.display = "none";
    guides.style.display = "none";
    idle();
  });

  svg.addEventListener("click", function (ev) {
    var c = cellAt(ev);
    if (!c) return;
    if (pinned && pinned.x === c.x && pinned.y === c.y) {
      pinned = null;
      pin.style.display = "none";
      setRead(describe(c));
      return;
    }
    pinned = c;
    pin.setAttribute("x", String(c.x));
    pin.setAttribute("y", String(c.y));
    pin.style.display = "";
    setRead(describe(c, "pinned"));
  });

  host.appendChild(svg);
  idle();

  if (!reduce) {
    var t0 = performance.now();
    function tick(now) {
      var y = 1 + ((now - t0) / 900) % 30;
      pulse.setAttribute("y", y.toFixed(2));
      requestAnimationFrame(tick);
    }
    requestAnimationFrame(tick);
  }
})();

/* Nav, anim toggle, live HL10T die with current on the fabric. */
(function () {
  var path = location.pathname.split("/").pop() || "index.html";
  if (path === "") path = "index.html";
  document.querySelectorAll("nav a").forEach(function (a) {
    if ((a.getAttribute("href") || "") === path) a.setAttribute("aria-current", "page");
  });

  var reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  var animOn = !reduce;
  var toggle = document.getElementById("anim-toggle");
  function syncAnim() {
    document.documentElement.setAttribute("data-anim", animOn ? "on" : "off");
    if (toggle) toggle.textContent = animOn ? "II" : "▶";
    if (toggle) toggle.title = animOn ? "Pause animations" : "Play animations";
  }
  if (toggle) {
    toggle.addEventListener("click", function () {
      animOn = !animOn;
      syncAnim();
    });
  }
  syncAnim();

  (function heroWrap() {
    var stage = document.getElementById("hero-stage");
    var video = document.getElementById("hero-vid");
    var visual = stage && stage.querySelector(".hero-visual");
    var copy = document.getElementById("hero-copy");
    var canvas = document.getElementById("hero-fg");
    if (!stage || !video || !visual || !copy || !canvas) return;

    var ctx = canvas.getContext("2d", { alpha: true });
    var dpr = 1;
    var running = false;
    var visible = true;

    function resize() {
      var r = stage.getBoundingClientRect();
      dpr = Math.min(window.devicePixelRatio || 1, 2);
      var w = Math.max(1, Math.round(r.width * dpr));
      var h = Math.max(1, Math.round(r.height * dpr));
      if (canvas.width !== w || canvas.height !== h) {
        canvas.width = w;
        canvas.height = h;
      }
      canvas.style.width = r.width + "px";
      canvas.style.height = r.height + "px";
      return r;
    }

    function catmull(pts, n) {
      var p = [pts[0]].concat(pts, [pts[pts.length - 1]]);
      var out = [];
      var segs = p.length - 3;
      var per = Math.max(2, Math.floor(n / segs));
      for (var i = 0; i < segs; i++) {
        var p0 = p[i], p1 = p[i + 1], p2 = p[i + 2], p3 = p[i + 3];
        for (var k = 0; k < per; k++) {
          var t = k / per, t2 = t * t, t3 = t2 * t;
          out.push({
            x: 0.5 * ((2 * p1.x) + (-p0.x + p2.x) * t + (2 * p0.x - 5 * p1.x + 4 * p2.x - p3.x) * t2 + (-p0.x + 3 * p1.x - 3 * p2.x + p3.x) * t3),
            y: 0.5 * ((2 * p1.y) + (-p0.y + p2.y) * t + (2 * p0.y - 5 * p1.y + 4 * p2.y - p3.y) * t2 + (-p0.y + 3 * p1.y - 3 * p2.y + p3.y) * t3)
          });
        }
      }
      out.push(pts[pts.length - 1]);
      return out;
    }

    function wiggle(path, t, amp, seed) {
      var out = new Array(path.length);
      for (var i = 0; i < path.length; i++) {
        var s = i / (path.length - 1);
        var ang = (s * 9 + seed) * Math.PI * 2 - t * Math.PI * 2;
        var nx = 0, ny = 1;
        if (i > 0 && i < path.length - 1) {
          var dx = path[i + 1].x - path[i - 1].x;
          var dy = path[i + 1].y - path[i - 1].y;
          var len = Math.hypot(dx, dy) || 1;
          nx = -dy / len;
          ny = dx / len;
        }
        var a = amp * (0.35 + 0.65 * Math.sin(ang) + 0.25 * Math.sin(ang * 2.1 + seed));
        out[i] = { x: path[i].x + nx * a, y: path[i].y + ny * a };
      }
      return out;
    }

    function stroke(path, width, color, alpha) {
      if (path.length < 2) return;
      ctx.beginPath();
      ctx.moveTo(path[0].x, path[0].y);
      for (var i = 1; i < path.length; i++) ctx.lineTo(path[i].x, path[i].y);
      ctx.strokeStyle = color;
      ctx.globalAlpha = alpha;
      ctx.lineWidth = width;
      ctx.lineCap = "round";
      ctx.lineJoin = "round";
      ctx.shadowColor = color;
      ctx.shadowBlur = Math.max(12, width * 1.35);
      ctx.stroke();
    }

    function pathsFor(stageR, vis, box) {
      var ox = vis.right - stageR.left - 18;
      var baseY = vis.top - stageR.top + vis.height * 0.47;
      var left = box.left - stageR.left;
      var top = box.top - stageR.top;
      var right = box.right - stageR.left;
      var bottom = box.bottom - stageR.top;
      var midY = (top + bottom) / 2;
      var endX = stageR.width + 70;
      var gapX = (ox + left) * 0.5;
      return [
        catmull([
          { x: ox, y: baseY - 22 },
          { x: gapX, y: baseY - 28 },
          { x: left + 36, y: top - 28 },
          { x: (left + right) * 0.5, y: top - 34 },
          { x: right + 24, y: top - 6 },
          { x: endX, y: top + 16 }
        ], 90),
        catmull([
          { x: ox, y: baseY - 6 },
          { x: gapX + 8, y: top + 10 },
          { x: left + 48, y: top + 6 },
          { x: (left + right) * 0.52, y: top + 18 },
          { x: right + 20, y: top + 36 },
          { x: endX, y: midY - 36 }
        ], 90),
        catmull([
          { x: ox, y: baseY + 4 },
          { x: gapX, y: midY - 4 },
          { x: left + 40, y: midY + 6 },
          { x: (left + right) * 0.5, y: midY + 8 },
          { x: right + 18, y: midY },
          { x: endX, y: midY + 10 }
        ], 90),
        catmull([
          { x: ox, y: baseY + 16 },
          { x: gapX + 6, y: bottom - 18 },
          { x: left + 44, y: bottom + 10 },
          { x: (left + right) * 0.5, y: bottom + 26 },
          { x: right + 28, y: bottom + 4 },
          { x: endX, y: bottom - 8 }
        ], 90),
        catmull([
          { x: ox, y: baseY + 30 },
          { x: gapX, y: bottom + 8 },
          { x: left + 60, y: bottom + 32 },
          { x: (left + right) * 0.62, y: bottom + 44 },
          { x: right + 36, y: bottom + 16 },
          { x: endX, y: bottom + 10 }
        ], 90)
      ];
    }

    function frame() {
      if (!running) return;
      var r = resize();
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, r.width, r.height);
      var vis = visual.getBoundingClientRect();
      var box = copy.getBoundingClientRect();
      var dur = video.duration || 3.5;
      var t = ((video.currentTime || 0) / dur) % 1;
      var bases = pathsFor(r, vis, box);
      ctx.save();
      ctx.globalCompositeOperation = "lighter";
      for (var p = 0; p < bases.length; p++) {
        var path = wiggle(bases[p], t, 11, p * 0.29);
        var path2 = wiggle(bases[p], t + 0.17, 18, p * 0.29 + 0.4);
        stroke(path2, 26, "rgba(186, 80, 255, 0.55)", 0.2);
        stroke(path, 16, "rgba(90, 235, 220, 0.85)", 0.28);
        stroke(path, 7, "rgba(180, 255, 250, 0.9)", 0.22);
      }
      ctx.restore();
      requestAnimationFrame(frame);
    }

    function start() {
      if (running || !visible || !animOn) return;
      running = true;
      frame();
    }
    function stop() { running = false; }

    var origSync = syncAnim;
    syncAnim = function () {
      origSync();
      if (!animOn) {
        stop();
        canvas.style.display = "none";
        video.pause();
      } else {
        canvas.style.display = "";
        video.play().catch(function () {});
        start();
      }
    };

    new ResizeObserver(resize).observe(stage);
    video.addEventListener("play", start);
    new IntersectionObserver(function (entries) {
      visible = !!(entries[0] && entries[0].isIntersecting);
      if (visible) start();
      else stop();
    }, { threshold: 0.05 }).observe(stage);

    resize();
    if (reduce) canvas.style.display = "none";
    else start();
  })();

  var host = document.getElementById("die");
  if (!host) return;

  var COLS = 35, ROWS = 34;
  var NS = "http://www.w3.org/2000/svg";
  var svg = document.createElementNS(NS, "svg");
  svg.setAttribute("viewBox", "0 0 " + COLS + " " + ROWS);
  svg.setAttribute("role", "img");
  svg.setAttribute("aria-label", "Live HL10T-C32-1. Current on the IO ring, clock spine, and CLBs.");
  svg.setAttribute("preserveAspectRatio", "xMidYMid meet");

  var defs = document.createElementNS(NS, "defs");
  defs.innerHTML =
    '<filter id="pulseGlow" x="-80%" y="-80%" width="260%" height="260%">' +
    '<feGaussianBlur stdDeviation="0.28" result="b"/>' +
    '<feMerge><feMergeNode in="b"/><feMergeNode in="SourceGraphic"/></feMerge></filter>' +
    '<filter id="clkGlow" x="-40%" y="-40%" width="180%" height="180%">' +
    '<feGaussianBlur stdDeviation="0.14" result="b"/>' +
    '<feMerge><feMergeNode in="b"/><feMergeNode in="SourceGraphic"/></feMerge></filter>';
  svg.appendChild(defs);

  function kind(x, y) {
    if (x === 0 || x === COLS - 1 || y === 0 || y === ROWS - 1) return "IO";
    if (x === 1) return "CLK";
    return "CLB";
  }
  function baseFill(k, x, y) {
    if (k === "IO") return [196, 122, 58];
    if (k === "CLK") return [226, 180, 58];
    return (x + y) % 2 === 0 ? [36, 56, 46] : [26, 44, 36];
  }
  function rgb(c, boost) {
    var t = Math.min(1, boost);
    return "rgb(" +
      Math.round(c[0] + (255 - c[0]) * t) + "," +
      Math.round(c[1] + (255 - c[1]) * t) + "," +
      Math.round(c[2] + (220 - c[2]) * t) + ")";
  }

  var rects = [];
  var energy = [];
  var bases = [];
  for (var y = 0; y < ROWS; y++) {
    rects[y] = [];
    energy[y] = [];
    bases[y] = [];
    for (var x = 0; x < COLS; x++) {
      var k = kind(x, y);
      var r = document.createElementNS(NS, "rect");
      r.setAttribute("x", (x + 0.08).toFixed(2));
      r.setAttribute("y", (y + 0.08).toFixed(2));
      r.setAttribute("width", "0.84");
      r.setAttribute("height", "0.84");
      var b = baseFill(k, x, y);
      bases[y][x] = b;
      energy[y][x] = 0;
      r.setAttribute("fill", rgb(b, 0));
      if (k === "CLK") r.setAttribute("filter", "url(#clkGlow)");
      svg.appendChild(r);
      rects[y][x] = r;
    }
  }

  function ioPath() {
    var p = [];
    var x, y;
    for (x = 0; x < COLS; x++) p.push([x, 0]);
    for (y = 1; y < ROWS; y++) p.push([COLS - 1, y]);
    for (x = COLS - 2; x >= 0; x--) p.push([x, ROWS - 1]);
    for (y = ROWS - 2; y >= 1; y--) p.push([0, y]);
    return p;
  }
  function clkPath() {
    var p = [];
    for (var y = 1; y <= 32; y++) p.push([1, y]);
    return p;
  }
  function rowPath(y) {
    var p = [];
    for (var x = 2; x <= 33; x++) p.push([x, y]);
    return p;
  }

  var io = ioPath();
  var clk = clkPath();
  var rows = [];
  for (var ry = 2; ry <= 31; ry += 3) rows.push(rowPath(ry));

  var dots = [];
  function addDot() {
    var c = document.createElementNS(NS, "circle");
    c.setAttribute("r", "0.42");
    c.setAttribute("filter", "url(#pulseGlow)");
    c.setAttribute("fill", "#fff6c2");
    svg.appendChild(c);
    return c;
  }

  var pulses = [];
  function spawn(path, speed, i0, color) {
    var d = addDot();
    if (color) d.setAttribute("fill", color);
    pulses.push({ path: path, i: i0 || 0, speed: speed, el: d, alive: true });
  }
  spawn(io, 0.35, 0, "#ffd089");
  spawn(io, 0.28, io.length * 0.33, "#ffe8a8");
  spawn(io, 0.42, io.length * 0.66, "#fff4c0");
  spawn(clk, 0.22, 0, "#fff1a0");
  spawn(clk, 0.18, 12, "#ffe07a");
  rows.forEach(function (rp, i) {
    spawn(rp, 0.3 + (i % 3) * 0.05, (i * 7) % rp.length, "#d7ffc8");
  });

  var hi = document.createElementNS(NS, "rect");
  hi.setAttribute("width", "1"); hi.setAttribute("height", "1");
  hi.setAttribute("fill", "none"); hi.setAttribute("stroke", "#f3f0ea");
  hi.setAttribute("stroke-width", "0.12");
  hi.style.display = "none";
  svg.appendChild(hi);

  var pinEl = document.createElementNS(NS, "rect");
  pinEl.setAttribute("width", "1"); pinEl.setAttribute("height", "1");
  pinEl.setAttribute("fill", "none"); pinEl.setAttribute("stroke", "#e2b43a");
  pinEl.setAttribute("stroke-width", "0.14");
  pinEl.style.display = "none";
  svg.appendChild(pinEl);

  var read = document.getElementById("die-read");
  var pinned = null;

  function cellAt(ev) {
    var ctm = svg.getScreenCTM();
    if (!ctm) return null;
    var pt = svg.createSVGPoint();
    pt.x = ev.clientX; pt.y = ev.clientY;
    pt = pt.matrixTransform(ctm.inverse());
    var x = Math.floor(pt.x), y = Math.floor(pt.y);
    if (x < 0 || y < 0 || x >= COLS || y >= ROWS) return null;
    return { x: x, y: y, k: kind(x, y) };
  }
  function describe(c, extra) {
    var bits = [c.k, "x=" + c.x, "y=" + c.y, "part=HL10T-C32-1"];
    if (c.k === "CLB") bits.push("BLE×8");
    if (c.k === "IO") bits.push("pad");
    if (c.k === "CLK") bits.push("gclk");
    if (extra) bits.push(extra);
    return bits.join("  ·  ");
  }
  function idle() {
    if (read) read.textContent = pinned
      ? describe(pinned, "pinned")
      : "Current on the ring, spine, and fabric · hover a tile · click to pin";
  }

  svg.addEventListener("pointermove", function (ev) {
    var c = cellAt(ev);
    if (!c) { hi.style.display = "none"; idle(); return; }
    hi.setAttribute("x", String(c.x));
    hi.setAttribute("y", String(c.y));
    hi.style.display = "";
    if (read) read.textContent = describe(c, pinned && pinned.x === c.x && pinned.y === c.y ? "pinned" : "");
  });
  svg.addEventListener("pointerleave", function () {
    hi.style.display = "none";
    idle();
  });
  svg.addEventListener("click", function (ev) {
    var c = cellAt(ev);
    if (!c) return;
    if (pinned && pinned.x === c.x && pinned.y === c.y) {
      pinned = null; pinEl.style.display = "none";
      if (read) read.textContent = describe(c);
      return;
    }
    pinned = c;
    pinEl.setAttribute("x", String(c.x));
    pinEl.setAttribute("y", String(c.y));
    pinEl.style.display = "";
    if (read) read.textContent = describe(c, "pinned");
  });

  host.appendChild(svg);
  idle();

  var last = performance.now();
  function tick(now) {
    var dt = Math.min(0.05, (now - last) / 1000);
    last = now;
    if (animOn) {
      var y, x;
      for (y = 0; y < ROWS; y++) {
        for (x = 0; x < COLS; x++) {
          energy[y][x] *= 0.86;
        }
      }
      pulses.forEach(function (p) {
        p.i += p.speed;
        if (p.i >= p.path.length) p.i -= p.path.length;
        var i0 = Math.floor(p.i) % p.path.length;
        var i1 = (i0 + 1) % p.path.length;
        var f = p.i - Math.floor(p.i);
        var a = p.path[i0], b = p.path[i1];
        var px = a[0] + (b[0] - a[0]) * f + 0.5;
        var py = a[1] + (b[1] - a[1]) * f + 0.5;
        p.el.setAttribute("cx", px.toFixed(3));
        p.el.setAttribute("cy", py.toFixed(3));
        var cx = a[0], cy = a[1];
        if (energy[cy] && energy[cy][cx] !== undefined) energy[cy][cx] = Math.min(1, energy[cy][cx] + 0.85);
        if (kind(cx, cy) === "CLK" && Math.random() < 0.04) {
          var row = rows[cy % rows.length];
          if (row) {
            /* inject a short burst into a CLB row */
            var burst = Math.min(row.length - 1, Math.floor(Math.random() * 8) + 2);
            for (var k = 0; k < burst; k++) {
              var t = row[k];
              energy[t[1]][t[0]] = Math.min(1, energy[t[1]][t[0]] + 0.55 * (1 - k / burst));
            }
          }
        }
      });
      for (y = 0; y < ROWS; y++) {
        for (x = 0; x < COLS; x++) {
          rects[y][x].setAttribute("fill", rgb(bases[y][x], energy[y][x]));
        }
      }
    }
    requestAnimationFrame(tick);
  }
  requestAnimationFrame(tick);
})();

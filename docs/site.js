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

  (function heroThrough() {
    var stage = document.getElementById("hero-stage");
    var video = document.getElementById("hero-vid");
    var canvas = document.getElementById("hero-fg");
    if (!stage || !video || !canvas) return;

    var ctx = canvas.getContext("2d", { alpha: true });
    var mask = document.createElement("canvas");
    var mctx = mask.getContext("2d");
    var dpr = 1;
    var running = false;
    var visible = true;

    function stageRect() {
      return stage.getBoundingClientRect();
    }

    function resizeLayer(el) {
      var r = stageRect();
      dpr = Math.min(window.devicePixelRatio || 1, 2);
      var w = Math.max(1, Math.round(r.width * dpr));
      var h = Math.max(1, Math.round(r.height * dpr));
      if (el.width !== w || el.height !== h) {
        el.width = w;
        el.height = h;
      }
      el.style.width = r.width + "px";
      el.style.height = r.height + "px";
      return r;
    }

    function drawVideo(g, ox, oy, zoom) {
      var r = stageRect();
      var vw = video.videoWidth;
      var vh = video.videoHeight;
      if (!vw || !vh) return;
      var elW = r.width * 1.12;
      var elH = r.height * 1.12;
      var elX = r.width * -0.06 + (ox || 0);
      var elY = r.height * -0.06 + (oy || 0);
      var s = Math.max(elW / vw, elH / vh) * (zoom || 1);
      var dw = vw * s;
      var dh = vh * s;
      var dx = elX + (elW - dw) * 0.12;
      var dy = elY + (elH - dh) * 0.48;
      g.drawImage(video, dx, dy, dw, dh);
    }

    function punchGlyphs(g, origin) {
      var nodes = stage.querySelectorAll("[data-punch]");
      var range = document.createRange();
      g.fillStyle = "#000";
      g.strokeStyle = "#000";
      g.lineJoin = "round";
      g.lineWidth = 2.6;
      g.textBaseline = "alphabetic";
      g.letterSpacing = "0px";
      nodes.forEach(function (el) {
        var walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
        var node;
        while ((node = walker.nextNode())) {
          var str = node.nodeValue;
          var cs = window.getComputedStyle(node.parentElement);
          g.font =
            cs.fontStyle + " " + cs.fontWeight + " " + cs.fontSize + " " + cs.fontFamily;
          var m0 = g.measureText("Hg");
          var ascent = m0.fontBoundingBoxAscent || parseFloat(cs.fontSize) * 0.8;
          var descent = m0.fontBoundingBoxDescent || parseFloat(cs.fontSize) * 0.2;
          var em = ascent + descent;
          for (var i = 0; i < str.length; i++) {
            if (str[i] === "\n") continue;
            range.setStart(node, i);
            range.setEnd(node, i + 1);
            var b = range.getBoundingClientRect();
            if (b.width < 0.15 || b.height < 0.5) continue;
            var extra = Math.max(0, b.height - em);
            var x = b.left - origin.left;
            var y = b.top - origin.top + extra / 2 + ascent;
            g.strokeText(str[i], x, y);
            g.fillText(str[i], x, y);
          }
        }
      });
    }

    function rebuildMask() {
      var r = resizeLayer(mask);
      mctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      mctx.globalCompositeOperation = "source-over";
      mctx.fillStyle = "#fff";
      mctx.fillRect(0, 0, r.width, r.height);
      mctx.globalCompositeOperation = "destination-out";
      punchGlyphs(mctx, r);
      mctx.globalCompositeOperation = "source-over";
    }

    function frame() {
      if (!running) return;
      var r = resizeLayer(canvas);
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.clearRect(0, 0, canvas.width, canvas.height);
      if (video.readyState >= 2) {
        ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
        drawVideo(ctx, 18, -10, 1.045);
        ctx.setTransform(1, 0, 0, 1, 0, 0);
        ctx.globalCompositeOperation = "destination-in";
        ctx.drawImage(mask, 0, 0);
        ctx.globalCompositeOperation = "source-over";
      }
      if (video.requestVideoFrameCallback) {
        video.requestVideoFrameCallback(function () { frame(); });
      } else {
        requestAnimationFrame(frame);
      }
    }

    function start() {
      if (running || !visible || !animOn) return;
      running = true;
      frame();
    }
    function stop() { running = false; }

    function layout() {
      resizeLayer(canvas);
      rebuildMask();
    }

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

    new ResizeObserver(layout).observe(stage);
    window.addEventListener("resize", layout);
    if (document.fonts && document.fonts.ready) document.fonts.ready.then(layout);
    video.addEventListener("loadeddata", layout);
    video.addEventListener("play", start);
    new IntersectionObserver(function (entries) {
      visible = !!(entries[0] && entries[0].isIntersecting);
      if (visible) start();
      else stop();
    }, { threshold: 0.05 }).observe(stage);

    layout();
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

/* Helion v4: living canvas background, limelight nav, reveals, counters. */
(function () {
  "use strict";
  var reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  var shotMode = /(?:\?|&)shot=1/.test(location.search); /* deterministic screenshots */
  if (shotMode) reduced = true;

  /* ---------- active nav link ---------- */
  var path = (location.pathname.split("/").pop() || "index.html");
  document.querySelectorAll("#site-nav a").forEach(function (a) {
    var href = a.getAttribute("href") || "";
    if (href === path) a.classList.add("active");
  });

  /* ---------- mobile drawer ---------- */
  var pill = document.getElementById("pillNav");
  var toggle = document.querySelector(".nav-toggle");
  if (toggle && pill) {
    toggle.addEventListener("click", function () {
      var open = pill.classList.toggle("open");
      toggle.setAttribute("aria-expanded", open ? "true" : "false");
    });
  }

  /* ---------- slim nav on scroll ---------- */
  if (pill) {
    var onScroll = function () { pill.classList.toggle("scrolled", window.scrollY > 40); };
    window.addEventListener("scroll", onScroll, { passive: true });
    onScroll();
  }

  /* ---------- limelight spotlight follows hovered nav item ---------- */
  var nav = document.getElementById("site-nav");
  var light = nav ? nav.querySelector(".limelight") : null;
  if (nav && light && !reduced) {
    var move = function (el) {
      if (!el) { light.style.opacity = "0"; return; }
      light.style.opacity = "1";
      light.style.width = el.offsetWidth + "px";
      light.style.transform = "translateX(" + el.offsetLeft + "px)";
    };
    nav.querySelectorAll("a").forEach(function (a) {
      a.addEventListener("mouseenter", function () { move(a); });
      a.addEventListener("focus", function () { move(a); });
    });
    nav.addEventListener("mouseleave", function () { move(nav.querySelector("a.active")); });
    move(nav.querySelector("a.active"));
    setTimeout(function () { if (!nav.matches(":hover")) light.style.opacity = "0"; }, 1200);
  }

  /* ---------- scroll reveals ---------- */
  var revealEls = document.querySelectorAll(".reveal");
  if (revealEls.length && !reduced && "IntersectionObserver" in window) {
    var io = new IntersectionObserver(function (entries) {
      entries.forEach(function (e) {
        if (e.isIntersecting) { e.target.classList.add("in"); io.unobserve(e.target); }
      });
    }, { threshold: 0.12, rootMargin: "0px 0px -40px 0px" });
    revealEls.forEach(function (el) { io.observe(el); });
  } else {
    revealEls.forEach(function (el) { el.classList.add("in"); });
  }

  /* ---------- animated counters ---------- */
  var counters = document.querySelectorAll("[data-count]");
  var runCounter = function (el) {
    var target = parseFloat(el.getAttribute("data-count"));
    var prefix = el.getAttribute("data-prefix") || "";
    var suffix = el.getAttribute("data-suffix") || "";
    if (reduced) { el.innerHTML = prefix + target.toLocaleString("en-US") + '<span class="unit">' + suffix + "</span>"; return; }
    var dur = 1400, t0 = null;
    var step = function (t) {
      if (!t0) t0 = t;
      var p = Math.min(1, (t - t0) / dur);
      var eased = 1 - Math.pow(1 - p, 3);
      var val = Math.round(target * eased).toLocaleString("en-US");
      el.innerHTML = prefix + val + '<span class="unit">' + suffix + "</span>";
      if (p < 1) requestAnimationFrame(step);
    };
    requestAnimationFrame(step);
  };
  if (counters.length && "IntersectionObserver" in window && !reduced) {
    var cio = new IntersectionObserver(function (entries) {
      entries.forEach(function (e) {
        if (e.isIntersecting) { runCounter(e.target); cio.unobserve(e.target); }
      });
    }, { threshold: 0.4 });
    counters.forEach(function (el) { cio.observe(el); });
  } else {
    counters.forEach(runCounter);
  }

  /* ---------- living background: drifting blobs + flow field ---------- */
  var stage = document.querySelector(".bg-stage");
  if (!stage) return;
  var canvas = document.createElement("canvas");
  canvas.setAttribute("aria-hidden", "true");
  stage.appendChild(canvas);
  var ctx = canvas.getContext("2d");

  var blobs = [
    { x: 0.22, y: 0.30, r: 0.42, c: "156,193,231", sx: 0.045, sy: 0.030, p: 0.0 },   /* periwinkle */
    { x: 0.78, y: 0.22, r: 0.38, c: "196,181,253", sx: 0.038, sy: 0.042, p: 2.1 },   /* violet */
    { x: 0.55, y: 0.55, r: 0.46, c: "122,168,224", sx: 0.030, sy: 0.036, p: 4.2 },   /* blue */
    { x: 0.88, y: 0.62, r: 0.30, c: "232,180,138", sx: 0.050, sy: 0.028, p: 1.2 },   /* copper whisper */
    { x: 0.10, y: 0.66, r: 0.32, c: "174,206,240", sx: 0.042, sy: 0.034, p: 5.4 }    /* pale blue */
  ];

  var parts = [];
  var W = 0, H = 0, DPR = 1;
  var resize = function () {
    DPR = Math.min(2, window.devicePixelRatio || 1);
    W = stage.clientWidth; H = stage.clientHeight;
    canvas.width = Math.max(1, W * DPR); canvas.height = Math.max(1, H * DPR);
    ctx.setTransform(DPR, 0, 0, DPR, 0, 0);
    parts = [];
    var n = Math.floor((W * H) / 26000);
    for (var i = 0; i < n; i++) {
      parts.push({
        x: Math.random() * W, y: Math.random() * H,
        s: 0.6 + Math.random() * 1.4,
        a: 0.05 + Math.random() * 0.10,
        seed: Math.random() * 1000
      });
    }
  };
  resize();
  window.addEventListener("resize", resize);

  var draw = function (t) {
    ctx.clearRect(0, 0, W, H);
    /* blobs: soft radial washes drifting on slow sine paths */
    blobs.forEach(function (b) {
      var bx = (b.x + Math.sin(t * b.sx + b.p) * 0.06) * W;
      var by = (b.y + Math.cos(t * b.sy + b.p * 1.7) * 0.06) * H;
      var br = b.r * Math.max(W, H) * (1 + 0.05 * Math.sin(t * 0.11 + b.p));
      var g = ctx.createRadialGradient(bx, by, 0, bx, by, br);
      g.addColorStop(0, "rgba(" + b.c + ",0.42)");
      g.addColorStop(0.55, "rgba(" + b.c + ",0.16)");
      g.addColorStop(1, "rgba(" + b.c + ",0)");
      ctx.fillStyle = g;
      ctx.fillRect(bx - br, by - br, br * 2, br * 2);
    });
    /* flow field: faint particles riding a slow pseudo-noise field */
    ctx.fillStyle = "#5b6b8c";
    parts.forEach(function (p) {
      var ang = Math.sin(p.x * 0.004 + t * 0.25 + p.seed) + Math.cos(p.y * 0.005 - t * 0.2);
      p.x += Math.cos(ang) * 0.35; p.y += Math.sin(ang) * 0.35;
      if (p.x < -8) p.x = W + 8; if (p.x > W + 8) p.x = -8;
      if (p.y < -8) p.y = H + 8; if (p.y > H + 8) p.y = -8;
      ctx.globalAlpha = p.a;
      ctx.beginPath(); ctx.arc(p.x, p.y, p.s, 0, 6.2832); ctx.fill();
    });
    ctx.globalAlpha = 1;
  };

  if (reduced) {
    draw(8.4); /* one calm static frame */
  } else {
    var start = null, last = 0;
    var running = true;
    var frame = function (now) {
      if (!running) return;
      if (!start) start = now;
      if (now - last > 66) { draw((now - start) / 1000); last = now; } /* ~15fps, cheap */
      requestAnimationFrame(frame);
    };
    /* pause when the hero is off-screen */
    if ("IntersectionObserver" in window) {
      new IntersectionObserver(function (entries) {
        var vis = entries[0].isIntersecting;
        if (vis && !running) { running = true; requestAnimationFrame(frame); }
        else running = vis;
      }).observe(stage);
    }
    requestAnimationFrame(frame);
  }
})();

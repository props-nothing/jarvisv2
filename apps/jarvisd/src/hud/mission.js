/* JARVIS console: the live mission view (ADR-0152).
 *
 * While JARVIS works, the run's own event stream says what it is doing: which tool it asked for, what about, whether it worked, and
 * for web tools which pages it touched. This file turns that into three things on the face page:
 *
 *   - a feed of live actions (what, about what, how long, and the links it found),
 *   - a strip of sources, each a real link that opens in a new tab,
 *   - animation around the face: satellites on the outer ring for each action, data flowing in or out along a beam, and an effect
 *     per kind of work (radar for a search, a scan for a page or file, contracting rings for memory, sparks for writing and
 *     commands, a twin orb for a sub-agent, a thought net while it thinks).
 *
 * Everything shown comes from a model's arguments or from a page, so it is untrusted: text is only ever set with textContent, and a
 * link is offered only when it is a plain http or https address (the daemon checks too). Links open with rel="noopener noreferrer".
 * Nothing here talks to the network except by being handed events.
 */
(function () {
  "use strict";

  function h(tag, className, text) {
    var node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined && text !== null) node.textContent = text;
    return node;
  }
  // The SVG namespace is an identifier, not a request; it is split so the page can be checked for "loads nothing from elsewhere".
  var SVG = "http:" + "//www.w3.org/2000/svg";

  // ---- what each tool is, in words and colour ------------------------------------------------------------------------
  var COLORS = {
    web: [94, 227, 255], file: [107, 226, 160], memory: [176, 150, 255], mail: [255, 180, 84],
    cmd: [255, 138, 92], agent: [255, 120, 214], time: [120, 240, 214], project: [150, 200, 255], tool: [170, 200, 220]
  };
  var TOOLS = {
    "jarvis.web.search": ["web", "Searching the web", "in", "radar"],
    "jarvis.web.fetch": ["web", "Reading a page", "in", "scan"],
    "jarvis.memory.search": ["memory", "Recalling", "in", "pull"],
    "jarvis.memory.propose": ["memory", "Noting a memory", "out", "pull"],
    "jarvis.files.read": ["file", "Reading", "in", "scan"],
    "jarvis.files.list": ["file", "Listing", "in", "scan"],
    "jarvis.files.search": ["file", "Searching files", "in", "radar"],
    "jarvis.files.write": ["file", "Writing", "out", "spark"],
    "jarvis.files.edit": ["file", "Editing", "out", "spark"],
    "jarvis.command.run": ["cmd", "Running", "out", "spark"],
    "jarvis.code.run": ["cmd", "Running code", "out", "spark"],
    "jarvis.agent.delegate": ["agent", "Sub-agent", "out", "twin"],
    "jarvis.agent.result": ["agent", "Collecting a result", "in", "pull"],
    "jarvis.gmail.search": ["mail", "Searching mail", "in", "radar"],
    "jarvis.gmail.read": ["mail", "Reading an email", "in", "scan"],
    "jarvis.gmail.send": ["mail", "Sending an email", "out", "spark"],
    "jarvis.calendar.events": ["mail", "Checking the calendar", "in", "scan"],
    "jarvis.calendar.create": ["mail", "Adding an event", "out", "spark"],
    "jarvis.schedule.add": ["time", "Scheduling", "out", "pull"],
    "jarvis.schedule.list": ["time", "Checking the schedule", "in", "pull"],
    "jarvis.schedule.remove": ["time", "Removing a schedule", "out", "pull"],
    "jarvis.project.note": ["project", "Writing the journal", "out", "pull"]
  };
  function describe(tool) {
    var known = TOOLS[tool];
    if (known) return { kind: known[0], verb: known[1], dir: known[2], fx: known[3] };
    return { kind: "tool", verb: String(tool || "working").replace(/^jarvis\./, ""), dir: "out", fx: "pull" };
  }

  var ICONS = {
    web: ["circle 8 8 6", "ellipse 8 8 2.6 6", "path M2 8h12"],
    file: ["path M4 2h5l3 3v9H4z", "path M9 2v3h3"],
    memory: ["path M3 4c0-1.5 2.2-2.5 5-2.5s5 1 5 2.5-2.2 2.5-5 2.5S3 5.5 3 4z", "path M3 4v8c0 1.5 2.2 2.5 5 2.5s5-1 5-2.5V4", "path M3 8c0 1.5 2.2 2.5 5 2.5S13 9.5 13 8"],
    mail: ["path M2 4h12v8H2z", "path M2 4l6 5 6-5"],
    cmd: ["path M3 4l4 4-4 4", "path M8 12h5"],
    agent: ["circle 4 8 2", "circle 12 4 2", "circle 12 12 2", "path M6 7.4l4-2.4M6 8.6l4 2.4"],
    time: ["circle 8 8 6", "path M8 4v4l3 2"],
    project: ["path M4 2v12", "path M4 3h8l-2 3 2 3H4"],
    tool: ["path M8 2l6 6-6 6-6-6z"]
  };
  function icon(kind) {
    var svg = document.createElementNS(SVG, "svg");
    svg.setAttribute("viewBox", "0 0 16 16");
    svg.setAttribute("width", "16");
    svg.setAttribute("height", "16");
    svg.setAttribute("aria-hidden", "true");
    (ICONS[kind] || ICONS.tool).forEach(function (spec) {
      var parts = spec.split(" ");
      var node;
      if (parts[0] === "circle") {
        node = document.createElementNS(SVG, "circle");
        node.setAttribute("cx", parts[1]); node.setAttribute("cy", parts[2]); node.setAttribute("r", parts[3]);
      } else if (parts[0] === "ellipse") {
        node = document.createElementNS(SVG, "ellipse");
        node.setAttribute("cx", parts[1]); node.setAttribute("cy", parts[2]); node.setAttribute("rx", parts[3]); node.setAttribute("ry", parts[4]);
      } else {
        node = document.createElementNS(SVG, "path");
        node.setAttribute("d", spec.slice(5));
      }
      node.setAttribute("fill", "none");
      node.setAttribute("stroke", "currentColor");
      node.setAttribute("stroke-width", "1.4");
      node.setAttribute("stroke-linecap", "round");
      node.setAttribute("stroke-linejoin", "round");
      svg.appendChild(node);
    });
    return svg;
  }

  // ---- links ---------------------------------------------------------------------------------------------------------
  // Only a plain web address is ever a link. The daemon has checked it too; this is the second lock, on the side that renders it.
  function safeHref(candidate) {
    try {
      var url = new URL(String(candidate));
      if (url.protocol !== "http:" && url.protocol !== "https:") return null;
      if (url.username || url.password) return null;
      return url.href;
    } catch (ignored) { return null; }
  }
  function hostOf(href) {
    try { return new URL(href).hostname.replace(/^www\./, ""); } catch (ignored) { return href; }
  }
  // A little of the path, so two pages of one site can be told apart.
  function pathOf(href) {
    try {
      var path = new URL(href).pathname.replace(/\/$/, "");
      return path.length > 1 ? (path.length > 26 ? path.slice(0, 25) + "\u2026" : path) : "";
    } catch (ignored) { return ""; }
  }
  function hue(text) {
    var sum = 7;
    for (var i = 0; i < text.length; i += 1) sum = (sum * 31 + text.charCodeAt(i)) % 360;
    return sum;
  }
  function chip(link, className) {
    var href = safeHref(link.url);
    if (!href) return null;
    var host = hostOf(href);
    var label = host + pathOf(href);
    var node = h("a", className || "mchip");
    node.href = href;
    node.target = "_blank";
    node.rel = "noopener noreferrer";
    node.title = (link.title ? link.title + "\n" : "") + href;
    var dot = h("i");
    dot.style.background = "hsl(" + hue(host) + ", 70%, 62%)";
    node.appendChild(dot);
    node.appendChild(h("span", "", label));
    return node;
  }

  // ---- state ---------------------------------------------------------------------------------------------------------
  var MAX_ROWS = 7, MAX_SOURCES = 14, KEEP_DONE_MS = 5000, FADE_MS = 26000, SOURCES_MS = 240000;
  var M = { items: [], runs: {}, sources: [], seq: 0, lastTouch: 0, onStart: null };

  function now() { return Date.now(); }
  function trim(text, limit) {
    var flat = String(text || "").replace(/\s+/g, " ").trim();
    return flat.length > limit ? flat.slice(0, limit - 1) + "\u2026" : flat;
  }
  function runOf(id, tag) {
    var run = M.runs[id];
    if (!run) run = M.runs[id] = { id: id, tag: tag || "", thinking: false, ended: false, pending: [], last: 0, seq: 0 };
    if (tag && !run.tag) run.tag = tag;
    return run;
  }

  function begin() {
    // A new request: what the last one found is put away.
    M.sources = [];
    M.items = M.items.filter(function (item) { return item.state === "run" || item.state === "wait"; });
    paintAll();
  }

  function startItem(run, payload, when, live) {
    var info = describe(payload.tool);
    var item = {
      id: ++M.seq, run: run.id, tag: run.tag, tool: payload.tool, kind: info.kind, verb: info.verb, dir: info.dir, fx: info.fx,
      target: trim(payload.target || "", 90), state: "run", born: when, ended: 0, links: [], detail: "",
      angle: M.seq * 2.399, spin: (M.seq % 2 ? 1 : -1) * (0.22 + (M.seq % 3) * 0.05), burst: false, replay: !live, node: null
    };
    M.items.push(item);
    run.pending.push(item);
    run.thinking = false;
    if (M.items.length > 80) M.items.shift();
    if (live && M.onStart) { try { M.onStart(item); } catch (ignored) { /* the face is decoration */ } }
    return item;
  }

  function settle(item, ok, payload, when, live) {
    item.state = ok ? "ok" : "bad";
    item.ended = when;
    item.burst = live;
    item.detail = payload && payload.detail ? trim(payload.detail, 120) : "";
    item.count = payload && typeof payload.count === "number" ? payload.count : null;
    var links = (payload && payload.links) || [];
    links.forEach(function (link) {
      var href = safeHref(link && link.url);
      if (!href) return;
      var entry = { title: trim(link.title || "", 120), url: href };
      item.links.push(entry);
      var known = M.sources.filter(function (source) { return source.url === href; })[0];
      if (!known) M.sources.unshift({ title: entry.title, url: href, at: now(), run: item.run });
    });
    M.sources = M.sources.slice(0, MAX_SOURCES);
  }

  // One frame of a run's stream. `tag` says what kind of run it is ("scheduled", "sub-agent", or empty for the one you asked).
  function event(runId, tag, frame) {
    if (!frame || !frame.event) return;
    var run = runOf(runId, tag);
    var seq = Number(frame.id) || 0;
    if (seq && seq <= run.seq) return;
    if (seq) run.seq = seq;
    var body = frame.body || {};
    var payload = body.payload || {};
    var when = Date.parse(body.occurred_at) || now();
    var live = now() - when < 8000;
    run.last = Math.max(run.last, when);
    M.lastTouch = now();
    var name = frame.event;
    if (name === "tool_requested" && payload.tool) {
      startItem(run, payload, when, live);
    } else if (name === "activity_updated") {
      if (payload.phase === "tool_result") {
        var at = -1;
        for (var i = 0; i < run.pending.length; i += 1) { if (run.pending[i].tool === payload.tool) { at = i; break; } }
        if (at >= 0) settle(run.pending.splice(at, 1)[0], payload.ok !== false, payload, when, live);
      } else if (payload.phase) {
        run.thinking = true;
      }
    } else if (name === "state_changed" && body.summary === "calling the model") {
      run.thinking = true;
    } else if (name === "output_delta") {
      run.thinking = false;
    } else if (name === "approval_requested") {
      var held = run.pending[run.pending.length - 1];
      if (held) held.state = "wait";
    } else if (name === "run_completed" || name === "run_failed" || name === "run_cancelled") {
      run.pending.splice(0).forEach(function (item) { settle(item, name === "run_completed", null, when, live); });
      run.thinking = false;
      run.ended = true;
    }
    paintAll();
  }

  // ---- the feed ------------------------------------------------------------------------------------------------------
  var root = null, feed = null, sourcesBox = null, chipsBox = null, countNode = null, titleNode = null;

  function seconds(ms) {
    var s = Math.max(0, ms) / 1000;
    if (s < 10) return s.toFixed(1) + "s";
    if (s < 90) return Math.round(s) + "s";
    return Math.floor(s / 60) + "m " + (Math.round(s) % 60) + "s";
  }

  function makeRow(item) {
    var row = h("div", "mrow kind-" + item.kind);
    var rgb = COLORS[item.kind] || COLORS.tool;
    row.style.setProperty("--k", rgb[0] + "," + rgb[1] + "," + rgb[2]);
    var ico = h("span", "mico");
    ico.appendChild(icon(item.kind));
    var main = h("div", "mmain");
    var line = h("div", "mline");
    line.appendChild(h("span", "mverb", item.verb));
    if (item.tag) line.appendChild(h("span", "mtag", item.tag));
    line.appendChild(h("span", "mtime", ""));
    main.appendChild(line);
    main.appendChild(h("div", "mtarget", item.target));
    var links = h("div", "mlinks");
    main.appendChild(links);
    var state = h("span", "mstate");
    row.appendChild(ico);
    row.appendChild(main);
    row.appendChild(state);
    return row;
  }

  function paintRow(item) {
    var row = item.node;
    row.className = "mrow kind-" + item.kind + " " + item.state;
    var ms = (item.ended || now()) - item.born;
    row.querySelector(".mtime").textContent = item.state === "wait" ? "needs you" : seconds(ms);
    var target = row.querySelector(".mtarget");
    var shown = item.state === "bad" && item.detail ? item.detail : item.target;
    if (item.state === "ok" && item.count !== null && item.count !== undefined && !item.links.length) shown = (item.target ? item.target + " \u00B7 " : "") + item.count + " results";
    target.textContent = shown;
    target.hidden = !shown;
    var box = row.querySelector(".mlinks");
    if (box.childNodes.length !== Math.min(item.links.length, 4) + (item.links.length > 4 ? 1 : 0)) {
      box.replaceChildren();
      item.links.slice(0, 4).forEach(function (link) {
        var node = chip(link);
        if (node) box.appendChild(node);
      });
      if (item.links.length > 4) box.appendChild(h("span", "mmore", "+" + (item.links.length - 4)));
    }
  }

  function paintAll() {
    if (!root) return;
    var t = now();
    // The newest rows; older ones leave.
    var visible = M.items.filter(function (item) { return !item.replay || item.state === "run" || item.state === "wait" || t - (item.ended || item.born) < 90000; })
      .slice(-MAX_ROWS);
    M.items.forEach(function (item) {
      if (visible.indexOf(item) < 0 && item.node) { item.node.remove(); item.node = null; }
    });
    // Newest on top, so what is happening now is what you see first however small the panel is.
    visible.slice().reverse().forEach(function (item, position) {
      if (!item.node) {
        item.node = makeRow(item);
        item.node.classList.add("enter");
        requestAnimationFrame(function () { if (item.node) item.node.classList.remove("enter"); });
      }
      if (feed.children[position] !== item.node) feed.insertBefore(item.node, feed.children[position] || null);
      paintRow(item);
    });
    var running = M.items.filter(function (item) { return item.state === "run" || item.state === "wait"; }).length;
    var recent = t - M.lastTouch < FADE_MS;
    var sourcesAlive = M.sources.length > 0 && t - M.sources[0].at < SOURCES_MS;
    var on = running > 0 || (recent && visible.length > 0) || sourcesAlive;
    root.classList.toggle("on", on);
    root.classList.toggle("busy", running > 0);
    titleNode.textContent = running > 0 ? "LIVE" : "DONE";
    countNode.textContent = running > 0 ? running + (running === 1 ? " action" : " actions") : (visible.length ? visible.length + " steps" : "");

    sourcesBox.hidden = !sourcesAlive;
    if (sourcesAlive) {
      var signature = M.sources.map(function (source) { return source.url; }).join("|");
      if (chipsBox.dataset.sig !== signature) {
        chipsBox.dataset.sig = signature;
        chipsBox.replaceChildren();
        M.sources.forEach(function (source) {
          var node = chip(source, "mchip big");
          if (!node) return;
          node.querySelector("span").textContent = hostOf(source.url) + pathOf(source.url) + (source.title ? " \u2014 " + trim(source.title, 38) : "");
          chipsBox.appendChild(node);
        });
      }
      sourcesBox.querySelector(".mshead b").textContent = String(M.sources.length);
    }
    // Finished runs are forgotten a while after they end.
    Object.keys(M.runs).forEach(function (id) {
      var run = M.runs[id];
      if (run.ended && t - run.last > 600000) delete M.runs[id];
    });
  }

  // How far the face has made room for the feed, 0 to 1, eased so it glides. The page asks once a frame.
  var dockValue = 0;
  function dock(width) {
    var want = root && root.classList.contains("on") && width >= 760 ? 1 : 0;
    dockValue += (want - dockValue) * 0.07;
    if (Math.abs(want - dockValue) < 0.003) dockValue = want;
    return dockValue;
  }

  function mount() {
    root = document.getElementById("mission");
    if (!root) return false;
    feed = root.querySelector("#mfeed");
    sourcesBox = root.querySelector("#msources");
    chipsBox = root.querySelector("#mchips");
    countNode = root.querySelector("#mcount");
    titleNode = root.querySelector("#mtitle");
    setInterval(function () {
      M.items.forEach(function (item) { if (item.node && (item.state === "run" || item.state === "wait")) paintRow(item); });
      paintAll();
    }, 500);
    return true;
  }

  // ---- what the caption says -----------------------------------------------------------------------------------------
  function headline() {
    for (var i = M.items.length - 1; i >= 0; i -= 1) {
      var item = M.items[i];
      if (item.state === "run") return item.verb.toLowerCase() + (item.target ? " \u00B7 " + trim(item.target, 40) : "");
    }
    return "";
  }
  // What a run is doing this instant, for its card on the Ops page.
  function nowFor(runId) {
    for (var i = M.items.length - 1; i >= 0; i -= 1) {
      var item = M.items[i];
      if (item.run === runId && (item.state === "run" || item.state === "wait")) {
        return (item.state === "wait" ? "waiting for you: " : "") + item.verb.toLowerCase() + (item.target ? " \u00B7 " + trim(item.target, 90) : "");
      }
    }
    var run = M.runs[runId];
    return run && run.thinking ? "thinking" : "";
  }
  function running() {
    return M.items.filter(function (item) { return item.state === "run"; }).length;
  }
  function thinking() {
    return Object.keys(M.runs).some(function (id) { return M.runs[id].thinking && !M.runs[id].ended; });
  }
  // The links a finished answer gets under it: everything the run touched, once each.
  function sourcesOf(runId) {
    return M.sources.filter(function (source) { return source.run === runId; });
  }

  // ---- the animation -------------------------------------------------------------------------------------------------
  var fx = { rings: [], sparks: [], lastT: 0, ringTimer: {}, sparkTimer: 0 };
  var TAU = Math.PI * 2;
  function rgba(rgb, alpha) { return "rgba(" + Math.round(rgb[0]) + "," + Math.round(rgb[1]) + "," + Math.round(rgb[2]) + "," + Math.max(0, Math.min(1, alpha)) + ")"; }

  function addRing(x, y, from, to, rgb, t, life, width) {
    if (fx.rings.length < 40) fx.rings.push({ x: x, y: y, from: from, to: to, rgb: rgb, born: t, life: life, width: width || 1.5 });
  }

  function draw(ctx, cx, cy, R, t, baseRgb, calm, W, H, room) {
    var leftEdge = 8 + (room || 0);
    var dt = Math.min(0.1, Math.max(0, t - fx.lastT));
    fx.lastT = t;
    var clock = now();
    var active = M.items.filter(function (item) {
      return item.state === "run" || item.state === "wait" || (item.ended && clock - item.ended < KEEP_DONE_MS && !item.replay);
    }).slice(-8);
    var flavours = {};
    var thinkingNow = thinking();

    ctx.save();
    ctx.lineCap = "round";

    // ---- per-flavour effects over the face -------------------------------------------------------------------------
    active.forEach(function (item) { if (item.state === "run" || item.state === "wait") flavours[item.fx] = item; });

    if (flavours.radar && !calm) {
      var radarRgb = COLORS[flavours.radar.kind];
      // a rotating sweep and rings that leave the core
      if (ctx.createConicGradient) {
        var sweep = ctx.createConicGradient(t * 2.2, cx, cy);
        sweep.addColorStop(0, rgba(radarRgb, 0.0));
        sweep.addColorStop(0.9, rgba(radarRgb, 0.0));
        sweep.addColorStop(1, rgba(radarRgb, 0.38));
        ctx.fillStyle = sweep;
        ctx.beginPath();
        ctx.arc(cx, cy, R * 0.74, 0, TAU);
        ctx.fill();
      }
      fx.ringTimer.radar = (fx.ringTimer.radar || 0) + dt;
      if (fx.ringTimer.radar > 0.9) { fx.ringTimer.radar = 0; addRing(cx, cy, R * 0.2, R * 0.78, radarRgb, t, 1.6, 1.6); }
    }
    if (flavours.pull && !calm) {
      var pullRgb = COLORS[flavours.pull.kind];
      fx.ringTimer.pull = (fx.ringTimer.pull || 0) + dt;
      if (fx.ringTimer.pull > 1.0) { fx.ringTimer.pull = 0; addRing(cx, cy, R * 0.8, R * 0.3, pullRgb, t, 1.4, 2); }
    }
    if (flavours.scan) {
      var scanRgb = COLORS[flavours.scan.kind];
      ctx.save();
      ctx.beginPath();
      ctx.arc(cx, cy, R * 0.6, 0, TAU);
      ctx.clip();
      var phase = calm ? 0.5 : (Math.sin(t * 1.7) * 0.5 + 0.5);
      var y = cy - R * 0.6 + phase * R * 1.2;
      var band = ctx.createLinearGradient(0, y - R * 0.2, 0, y + 2);
      band.addColorStop(0, rgba(scanRgb, 0));
      band.addColorStop(1, rgba(scanRgb, 0.34));
      ctx.fillStyle = band;
      ctx.fillRect(cx - R, y - R * 0.2, R * 2, R * 0.2 + 2);
      ctx.fillStyle = rgba(scanRgb, 0.95);
      ctx.fillRect(cx - R * 0.6, y, R * 1.2, 1.6);
      // a few bright "data" ticks riding the line
      for (var d = 0; d < 9; d += 1) {
        var dx = cx - R * 0.6 + (((d * 0.137 + t * 0.5) % 1) * R * 1.2);
        ctx.fillRect(dx, y - 3, 2, 3);
      }
      ctx.restore();
    }
    if (flavours.spark && !calm) {
      var sparkRgb = COLORS[flavours.spark.kind];
      fx.sparkTimer += dt;
      while (fx.sparkTimer > 0.03 && fx.sparks.length < 90) {
        fx.sparkTimer -= 0.03;
        var a = Math.random() * TAU;
        fx.sparks.push({ x: cx + Math.cos(a) * R * 0.5, y: cy + Math.sin(a) * R * 0.5, vx: Math.cos(a), vy: Math.sin(a), s: R * (0.35 + Math.random() * 0.5), born: t, life: 0.5 + Math.random() * 0.5, rgb: sparkRgb });
      }
    }
    if (thinkingNow && !calm && active.length === 0) drawThoughts(ctx, cx, cy, R, t, baseRgb);

    // ---- rings and sparks already in flight -----------------------------------------------------------------------
    fx.rings = fx.rings.filter(function (ring) { return t - ring.born < ring.life; });
    fx.rings.forEach(function (ring) {
      var u = (t - ring.born) / ring.life;
      var radius = ring.from + (ring.to - ring.from) * u;
      ctx.beginPath();
      ctx.arc(ring.x, ring.y, Math.max(1, radius), 0, TAU);
      ctx.lineWidth = ring.width;
      ctx.strokeStyle = rgba(ring.rgb, 0.7 * (1 - u));
      ctx.stroke();
    });
    fx.sparks = fx.sparks.filter(function (spark) { return t - spark.born < spark.life; });
    fx.sparks.forEach(function (spark) {
      var age = t - spark.born;
      var px = spark.x + spark.vx * spark.s * age, py = spark.y + spark.vy * spark.s * age;
      ctx.strokeStyle = rgba(spark.rgb, 0.9 * (1 - age / spark.life));
      ctx.lineWidth = 1.6;
      ctx.beginPath();
      ctx.moveTo(px, py);
      ctx.lineTo(px - spark.vx * 8, py - spark.vy * 8);
      ctx.stroke();
    });

    // ---- one satellite per action, with its beam -------------------------------------------------------------------
    var labels = 0;
    active.forEach(function (item) {
      var rgb = COLORS[item.kind] || COLORS.tool;
      var done = item.state === "ok" || item.state === "bad";
      var age = item.ended ? (clock - item.ended) / KEEP_DONE_MS : 0;
      var fade = done ? 1 - age : 1;
      var angle = item.angle + (calm ? 0 : t * item.spin);
      var radius = R * 1.0;
      var sx = cx + Math.cos(angle) * radius, sy = cy + Math.sin(angle) * radius;
      var born = Math.min(1, (clock - item.born) / 500);

      if (item.burst) {
        item.burst = false;
        var burstRgb = item.state === "ok" ? [107, 226, 160] : [255, 107, 107];
        addRing(sx, sy, 4, R * 0.16, burstRgb, t, 0.9, 2);
        addRing(cx, cy, R * 0.45, R * 0.72, burstRgb, t, 0.8, 1.4);
      }

      // the beam, and packets travelling along it
      var ex = cx + Math.cos(angle) * R * 0.52, ey = cy + Math.sin(angle) * R * 0.52;
      var beam = ctx.createLinearGradient(sx, sy, ex, ey);
      beam.addColorStop(0, rgba(rgb, 0.45 * fade * born));
      beam.addColorStop(1, rgba(rgb, 0.04 * fade));
      ctx.strokeStyle = beam;
      ctx.lineWidth = 1.2;
      ctx.beginPath();
      ctx.moveTo(sx, sy);
      ctx.lineTo(ex, ey);
      ctx.stroke();
      if (!done) {
        for (var k = 0; k < 3; k += 1) {
          var u = calm ? 0.5 : ((t * 0.7 + k / 3 + item.id * 0.17) % 1);
          var v = item.dir === "in" ? u : 1 - u;
          var px = sx + (ex - sx) * v, py = sy + (ey - sy) * v;
          var glow = Math.sin(Math.PI * u);
          ctx.fillStyle = rgba(rgb, 0.95 * glow);
          ctx.beginPath();
          ctx.arc(px, py, 2.4, 0, TAU);
          ctx.fill();
          ctx.fillStyle = rgba(rgb, 0.18 * glow);
          ctx.beginPath();
          ctx.arc(px, py, 6, 0, TAU);
          ctx.fill();
        }
      }

      // the satellite
      var size = (item.fx === "twin" ? 8 : 5) * (0.6 + 0.4 * born);
      var tone = done ? (item.state === "ok" ? [107, 226, 160] : [255, 107, 107]) : item.state === "wait" ? [255, 180, 84] : rgb;
      var pulse = done || calm ? 1 : 1 + 0.22 * Math.sin(t * 6 + item.id);
      var halo = ctx.createRadialGradient(sx, sy, 0, sx, sy, size * 3.2 * pulse);
      halo.addColorStop(0, rgba(tone, 0.55 * fade));
      halo.addColorStop(1, rgba(tone, 0));
      ctx.fillStyle = halo;
      ctx.beginPath();
      ctx.arc(sx, sy, size * 3.2 * pulse, 0, TAU);
      ctx.fill();
      ctx.fillStyle = rgba(tone, fade);
      ctx.beginPath();
      ctx.arc(sx, sy, size, 0, TAU);
      ctx.fill();
      if (item.fx === "twin") {
        // a small orb of its own: a sub-agent is another worker
        for (var q = 0; q < 3; q += 1) {
          var from = (calm ? 0 : t * 2.4) + q * (TAU / 3);
          ctx.beginPath();
          ctx.arc(sx, sy, size + 6, from, from + 1.2);
          ctx.lineWidth = 1.6;
          ctx.strokeStyle = rgba(tone, 0.85 * fade);
          ctx.stroke();
        }
      } else if (!done) {
        ctx.beginPath();
        ctx.arc(sx, sy, size + 5, angle + Math.PI - 0.5, angle + Math.PI + 0.5);
        ctx.lineWidth = 1.4;
        ctx.strokeStyle = rgba(tone, 0.5);
        ctx.stroke();
      }

      // its label, outside the ring when there is room and inside when there is not
      if (!done && labels < 5 && W > 520) {
        labels += 1;
        var text = (item.verb + (item.target ? " \u00B7 " + trim(item.target, 26) : "")).toUpperCase();
        ctx.font = "10px Consolas, 'Cascadia Mono', monospace";
        ctx.textBaseline = "middle";
        var width = ctx.measureText(text).width;
        var right = Math.cos(angle) >= 0;
        var tx = right ? sx + size + 10 : sx - size - 10;
        if (right && tx + width > W - 8) { ctx.textAlign = "right"; tx = sx - size - 10; }
        else if (!right && tx - width < leftEdge) { ctx.textAlign = "left"; tx = sx + size + 10; }
        else ctx.textAlign = right ? "left" : "right";
        ctx.fillStyle = rgba(tone, 0.95);
        ctx.fillText(text, tx, sy);
      }
    });
    ctx.restore();
  }

  // A quiet net of thoughts around the head while the model is reasoning and has not asked for anything yet.
  function drawThoughts(ctx, cx, cy, R, t, rgb) {
    var points = [];
    for (var i = 0; i < 22; i += 1) {
      var base = i * 2.399;
      var r = R * (0.4 + 0.2 * ((i * 0.618) % 1));
      var a = base + t * (i % 2 ? 0.07 : -0.05);
      points.push([cx + Math.cos(a) * r, cy + Math.sin(a) * r * 0.92, 0.5 + 0.5 * Math.sin(t * 2 + i)]);
    }
    ctx.lineWidth = 1;
    for (var m = 0; m < points.length; m += 1) {
      for (var n = m + 1; n < points.length; n += 1) {
        var dx = points[m][0] - points[n][0], dy = points[m][1] - points[n][1];
        var dist = Math.sqrt(dx * dx + dy * dy);
        if (dist < R * 0.3) {
          ctx.strokeStyle = rgba(rgb, 0.22 * (1 - dist / (R * 0.3)));
          ctx.beginPath();
          ctx.moveTo(points[m][0], points[m][1]);
          ctx.lineTo(points[n][0], points[n][1]);
          ctx.stroke();
        }
      }
    }
    points.forEach(function (p) {
      ctx.fillStyle = rgba(rgb, 0.25 + 0.55 * p[2]);
      ctx.beginPath();
      ctx.arc(p[0], p[1], 1.8 + 1.4 * p[2], 0, TAU);
      ctx.fill();
    });
  }

  window.JarvisMission = {
    mount: mount, event: event, begin: begin, draw: draw, dock: dock, headline: headline, nowFor: nowFor, running: running, thinking: thinking,
    sourcesOf: sourcesOf, safeHref: safeHref, chip: chip, state: M,
    set onStart(callback) { M.onStart = callback; }
  };
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", mount);
  else mount();
})();

"use strict";
// The JARVIS console: talk to the assistant, watch it work, stop it. No dependencies.
//
// Security notes that the rest of the file relies on:
//  - The credential arrives in the URL fragment (never sent to a server), moves to this tab's session storage and is
//    removed from the address bar at once.
//  - Every value that came from the daemon or the model is inserted with textContent / text nodes, never as markup.
//  - Speech recognition and synthesis are the browser's own: nothing here sends audio anywhere itself. In Chrome and
//    Edge the browser's recognizer may use its vendor's cloud service; the page says so.
(function () {
  var fragment = new URLSearchParams(location.hash.slice(1));
  if (fragment.get("token")) {
    sessionStorage.setItem("jarvis-token", fragment.get("token"));
    history.replaceState(null, "", location.pathname);
  }
  var token = sessionStorage.getItem("jarvis-token");

  function $(id) { return document.getElementById(id); }
  function el(tag, className, text) {
    var node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = text;
    return node;
  }
  function sleep(ms) { return new Promise(function (resolve) { setTimeout(resolve, ms); }); }
  // A one-line plain rendering of model text for the side panels: markdown marks are dropped, not interpreted.
  function plainLine(text) {
    return String(text || "").replace(/```[\s\S]*?```/g, " code ").replace(/[`*_#>]/g, "").replace(/^\s*-\s+/gm, "");
  }
  function clip(text, limit) {
    var flat = String(text || "").split(/\s+/).join(" ");
    return flat.length > limit ? flat.slice(0, limit) + "..." : flat;
  }

  // ---- state --------------------------------------------------------------------------------------------------
  var S = {
    session: sessionStorage.getItem("jarvis-session"),
    offline: true, needYou: 0, working: 0, thinking: 0,
    listening: false, speaking: false, level: 0,
    speak: localStorage.getItem("jarvis-speak") === "1",
    wake: false
  };

  // ---- api ----------------------------------------------------------------------------------------------------
  function api(path, method, body) {
    var headers = { Authorization: "Bearer " + token };
    var init = { method: method || "GET", headers: headers, cache: "no-store" };
    if (body !== undefined) {
      headers["Content-Type"] = "application/json";
      init.body = JSON.stringify(body);
    }
    return fetch("/api/v1" + path, init).then(function (response) {
      if (!response.ok) {
        var error = new Error(String(response.status));
        error.status = response.status;
        throw error;
      }
      return response.status === 204 ? null : response.json();
    });
  }

  // ---- markdown-lite, as DOM nodes ------------------------------------------------------------------------------
  function inline(text, parent) {
    var re = /(`[^`]+`|\*\*[^*]+\*\*)/g, last = 0, match;
    while ((match = re.exec(text))) {
      if (match.index > last) parent.appendChild(document.createTextNode(text.slice(last, match.index)));
      var piece = match[0];
      parent.appendChild(piece[0] === "`" ? el("code", "", piece.slice(1, -1)) : el("strong", "", piece.slice(2, -2)));
      last = match.index + piece.length;
    }
    if (last < text.length) parent.appendChild(document.createTextNode(text.slice(last)));
  }
  function blocks(text, into) {
    text.split(/\n{2,}/).forEach(function (paragraph) {
      if (!paragraph.trim()) return;
      var lines = paragraph.split("\n");
      var bullet = /^\s*([-*]|\d+\.)\s+/;
      if (lines.every(function (line) { return bullet.test(line); })) {
        var list = el("ul");
        lines.forEach(function (line) {
          var item = el("li");
          inline(line.replace(bullet, ""), item);
          list.appendChild(item);
        });
        into.appendChild(list);
      } else {
        var p = el("p");
        lines.forEach(function (line, index) {
          if (index) p.appendChild(document.createElement("br"));
          inline(line, p);
        });
        into.appendChild(p);
      }
    });
  }
  function markdown(text) {
    var into = document.createDocumentFragment();
    String(text).split("```").forEach(function (part, index) {
      if (index % 2 === 1) {
        var pre = el("pre");
        pre.appendChild(el("code", "", part.replace(/^[a-z0-9+#-]*\n/i, "").replace(/\n$/, "")));
        into.appendChild(pre);
      } else {
        blocks(part, into);
      }
    });
    return into;
  }

  // ---- chat ---------------------------------------------------------------------------------------------------
  var chat = $("chat");
  var transcript = [];
  try { transcript = JSON.parse(sessionStorage.getItem("jarvis-chat") || "[]"); } catch (ignored) { transcript = []; }

  function remember(role, text) {
    transcript.push({ role: role, text: text });
    transcript = transcript.slice(-60);
    sessionStorage.setItem("jarvis-chat", JSON.stringify(transcript));
  }
  function scroll() { chat.scrollTop = chat.scrollHeight; }
  function clearEmpty() {
    var empty = chat.querySelector(".empty");
    if (empty) empty.remove();
  }
  function bubble(role, label) {
    clearEmpty();
    var node = el("div", "msg " + role);
    if (label) node.appendChild(el("div", "who", label));
    var chips = el("div", "chips");
    chips.hidden = true;
    var body = el("div", "body");
    node.appendChild(chips);
    node.appendChild(body);
    chat.appendChild(node);
    scroll();
    return { node: node, body: body, chips: chips };
  }
  function show(role, text) {
    var view = bubble(role, role === "user" ? "you" : role === "jarvis" ? "jarvis" : "");
    view.body.appendChild(role === "jarvis" ? markdown(text) : document.createTextNode(text));
    return view;
  }
  function renderEmpty() {
    var empty = el("div", "empty");
    empty.appendChild(el("p", "", "Nothing here yet."));
    var list = el("ul");
    ["Type below, or press the microphone and talk.",
     "Turn on the wake word and just say \u201CJarvis, \u2026\u201D.",
     "Say \u201CJarvis, stop\u201D to cancel everything that is running.",
     "Anything that needs your approval appears on the right."].forEach(function (line) {
      list.appendChild(el("li", "", line));
    });
    empty.appendChild(list);
    chat.appendChild(empty);
  }
  if (transcript.length === 0) renderEmpty();
  else transcript.forEach(function (entry) { show(entry.role, entry.text); });

  function pendingAnswer() {
    var view = bubble("jarvis", "jarvis");
    view.body.classList.add("cursor");
    var text = "";
    var queued = false;
    var done = false;
    function paint() {
      queued = false;
      if (done) return;
      view.body.replaceChildren(markdown(text));
      view.body.classList.add("cursor");
      scroll();
    }
    return {
      add: function (piece) {
        text += piece;
        if (!queued) { queued = true; requestAnimationFrame(paint); }
      },
      chip: function (label, waiting) {
        view.chips.hidden = false;
        view.chips.appendChild(el("span", "chip" + (waiting ? " wait" : ""), label));
      },
      finish: function (final) {
        done = true;
        if (typeof final === "string" && final.length) text = final;
        view.body.classList.remove("cursor");
        view.body.replaceChildren(markdown(text || "(no answer)"));
        remember("jarvis", text || "(no answer)");
        scroll();
        return text;
      },
      fail: function (message) {
        done = true;
        view.node.classList.add("error");
        view.body.classList.remove("cursor");
        view.body.replaceChildren(document.createTextNode(message));
        remember("system", message);
      }
    };
  }

  // ---- following a run's stream (server-sent events over fetch, so the credential can be a header) -------------
  function parseBlock(block) {
    var frame = { id: null, event: null, data: "" };
    block.split("\n").forEach(function (line) {
      if (line.indexOf("id:") === 0) frame.id = line.slice(3).trim();
      else if (line.indexOf("event:") === 0) frame.event = line.slice(6).trim();
      else if (line.indexOf("data:") === 0) frame.data += line.slice(5).trim();
    });
    if (!frame.event) return null;
    try { frame.body = JSON.parse(frame.data); } catch (ignored) { frame.body = {}; }
    return frame;
  }

  var TERMINAL = { run_completed: 1, run_failed: 1, run_cancelled: 1 };

  async function follow(runId, view) {
    var last = 0, finished = false, attempts = 0, finalText = null, failure = null;
    while (!finished && attempts < 4) {
      try {
        var headers = { Authorization: "Bearer " + token };
        if (last) headers["Last-Event-ID"] = String(last);
        var response = await fetch("/api/v1/runs/" + encodeURIComponent(runId) + "/stream", { headers: headers, cache: "no-store" });
        if (!response.ok) throw new Error(String(response.status));
        var reader = response.body.getReader();
        var decoder = new TextDecoder();
        var buffer = "";
        for (;;) {
          var chunk = await reader.read();
          if (chunk.done) break;
          buffer += decoder.decode(chunk.value, { stream: true }).replace(/\r\n/g, "\n");
          var cut;
          while ((cut = buffer.indexOf("\n\n")) >= 0) {
            var frame = parseBlock(buffer.slice(0, cut));
            buffer = buffer.slice(cut + 2);
            if (!frame) continue;
            if (frame.id) last = Number(frame.id) || last;
            var payload = (frame.body && frame.body.payload) || {};
            var summary = (frame.body && frame.body.summary) || "";
            if (frame.event === "output_delta") view.add(payload.text || "");
            else if (frame.event === "output_completed") finalText = payload.text;
            else if (frame.event === "tool_requested") view.chip(summary || "using a tool");
            else if (frame.event === "approval_requested") view.chip("waiting for your approval", true);
            else if (frame.event === "run_failed") failure = summary || "the run failed";
            else if (frame.event === "run_cancelled") failure = "stopped";
            if (TERMINAL[frame.event]) finished = true;
          }
          if (finished) break;
        }
      } catch (error) {
        await sleep(700);
      }
      if (!finished) attempts += 1;
    }
    if (failure) { view.fail(failure === "stopped" ? "Stopped." : "That did not complete: " + failure); return null; }
    return view.finish(finalText);
  }

  var busy = false;
  async function send(text) {
    text = String(text || "").trim();
    if (!text || busy) return;
    interruptSpeech();
    clearEmpty();
    show("user", text);
    remember("user", text);
    var view = pendingAnswer();
    busy = true;
    S.thinking = 1;
    try {
      var body = { objective: text };
      if (S.session) body.session_id = S.session;
      var reply = await api("/runs", "POST", body);
      S.session = reply.session_id;
      sessionStorage.setItem("jarvis-session", S.session);
      var answer = await follow(reply.run_id, view);
      if (answer && S.speak) speak(answer);
    } catch (error) {
      view.fail(error.status === 401 ? "The daemon rejected the credential. Open this page with `jarvis hud`." : "Could not reach the assistant.");
    } finally {
      busy = false;
      S.thinking = 0;
      refresh();
    }
  }

  $("composer").addEventListener("submit", function (event) {
    event.preventDefault();
    var input = $("input");
    var text = input.value;
    input.value = "";
    send(text);
  });

  // ---- voice: speaking -------------------------------------------------------------------------------------------
  var synth = window.speechSynthesis;
  function plain(text) {
    return String(text).replace(/```[\s\S]*?```/g, " code omitted. ").replace(/[`*_#>]/g, "").replace(/\s+/g, " ").trim();
  }
  function pickVoice() {
    if (!synth) return null;
    var voices = synth.getVoices().filter(function (voice) { return /^en/i.test(voice.lang); });
    var preferred = /(UK English Male|Daniel|Ryan|George|Guy|Aria|Google US English)/i;
    return voices.find(function (voice) { return preferred.test(voice.name); }) || voices[0] || null;
  }
  function speak(text) {
    if (!synth) return;
    interruptSpeech();
    var sentences = plain(text).slice(0, 1800).match(/[^.!?]+[.!?]*/g) || [];
    if (!sentences.length) return;
    stopListening();
    S.speaking = true;
    sentences.forEach(function (sentence, index) {
      var utterance = new SpeechSynthesisUtterance(sentence.trim());
      var voice = pickVoice();
      if (voice) utterance.voice = voice;
      utterance.rate = 1.04;
      utterance.pitch = 0.92;
      if (index === sentences.length - 1) utterance.onend = utterance.onerror = function () { S.speaking = false; resumeWake(); };
      synth.speak(utterance);
    });
  }
  // The "you can interrupt it" half of voice: the microphone, Escape, the orb, or a new message silences it at once.
  function interruptSpeech() {
    var was = S.speaking;
    if (synth && (synth.speaking || synth.pending)) synth.cancel();
    S.speaking = false;
    if (was) resumeWake();
  }
  $("speak").checked = S.speak;
  $("speak").addEventListener("change", function () {
    S.speak = $("speak").checked;
    localStorage.setItem("jarvis-speak", S.speak ? "1" : "0");
    if (!S.speak) interruptSpeech();
  });
  if (!synth) { $("speak").disabled = true; }

  // ---- voice: listening (push-to-talk and wake word) -----------------------------------------------------------------
  var Recognition = window.SpeechRecognition || window.webkitSpeechRecognition;
  var recognizer = null;
  var wakeArmedUntil = 0;
  var audio = { context: null, analyser: null, stream: null, data: null };

  function startMeter() {
    if (audio.context || !navigator.mediaDevices) return;
    navigator.mediaDevices.getUserMedia({ audio: true }).then(function (stream) {
      var Context = window.AudioContext || window.webkitAudioContext;
      audio.stream = stream;
      audio.context = new Context();
      audio.analyser = audio.context.createAnalyser();
      audio.analyser.fftSize = 256;
      audio.data = new Uint8Array(audio.analyser.fftSize);
      audio.context.createMediaStreamSource(stream).connect(audio.analyser);
    }).catch(function () {});
  }
  function stopMeter() {
    if (audio.stream) audio.stream.getTracks().forEach(function (track) { track.stop(); });
    if (audio.context) audio.context.close();
    audio = { context: null, analyser: null, stream: null, data: null };
  }
  function readLevel() {
    if (!audio.analyser) { S.level *= 0.9; return; }
    audio.analyser.getByteTimeDomainData(audio.data);
    var sum = 0;
    for (var i = 0; i < audio.data.length; i += 1) { var v = (audio.data[i] - 128) / 128; sum += v * v; }
    S.level += (Math.min(1, Math.sqrt(sum / audio.data.length) * 4) - S.level) * 0.35;
  }

  function say(hint) { $("hint").textContent = hint || ""; }

  // What a spoken utterance means. "stop" is the hands-free kill switch; everything else is a message.
  function command(text) {
    var spoken = text.trim().replace(/[.!?]+$/, "");
    if (!spoken) return;
    if (/^(stop|cancel|abort|kill)( that| it| everything| all| now)?$/i.test(spoken)) { say("stopping everything"); stopEverything(); return; }
    if (/^(quiet|be quiet|silence|shut up|enough)$/i.test(spoken)) { interruptSpeech(); return; }
    send(spoken);
  }

  function onResult(event) {
    var heard = "";
    for (var i = event.resultIndex; i < event.results.length; i += 1) {
      var result = event.results[i];
      if (!result.isFinal) { say("\u201C" + clip(result[0].transcript, 60) + "\u201D"); continue; }
      heard = result[0].transcript;
    }
    if (!heard) return;
    say("");
    if (!S.wake) { stopListening(); command(heard); return; }
    // Wake-word mode: only speech that names JARVIS (or follows it within a few seconds) is acted on.
    var match = /\bjarvis\b[\s,.:;!-]*(.*)$/i.exec(heard);
    if (match) {
      if (match[1].trim()) command(match[1]);
      else wakeArmedUntil = Date.now() + 8000;
    } else if (Date.now() < wakeArmedUntil) {
      wakeArmedUntil = 0;
      command(heard);
    }
  }

  function startListening(continuous) {
    if (!Recognition) { say("this browser has no speech recognition (use Chrome or Edge)"); return; }
    if (recognizer) return;
    interruptSpeech();
    var made = new Recognition();
    recognizer = made;
    made.lang = navigator.language || "en-US";
    made.continuous = !!continuous;
    made.interimResults = true;
    made.onresult = onResult;
    made.onerror = function (event) {
      if (event.error === "not-allowed" || event.error === "service-not-allowed") {
        say("microphone permission was refused");
        S.wake = false;
        $("wake").checked = false;
        stopListening();
      }
    };
    made.onend = function () {
      if (recognizer !== made) return;
      recognizer = null;
      S.listening = false;
      $("mic").classList.remove("on");
      if (S.wake && !S.speaking) setTimeout(function () { startListening(true); }, 250);
      else if (!S.wake) stopMeter();
    };
    try {
      made.start();
      S.listening = true;
      $("mic").classList.add("on");
      startMeter();
    } catch (error) { recognizer = null; }
  }
  function stopListening() {
    var active = recognizer;
    recognizer = null;
    if (active) { active.onend = null; try { active.abort(); } catch (ignored) {} }
    S.listening = false;
    $("mic").classList.remove("on");
    if (!S.wake) stopMeter();
  }
  // Wake-word listening pauses while JARVIS speaks (its own voice would be heard as the user's) and resumes after.
  function resumeWake() {
    if (!S.wake) return;
    setTimeout(function () { if (S.wake && !recognizer && !S.speaking) startListening(true); }, 300);
  }

  $("mic").addEventListener("click", function () {
    if (S.speaking) { interruptSpeech(); return; }
    if (recognizer && !S.wake) { stopListening(); return; }
    if (!recognizer) startListening(false);
  });
  $("wake").addEventListener("change", function () {
    S.wake = $("wake").checked;
    if (S.wake) { stopListening(); startListening(true); say("listening for \u201CJarvis\u201D"); }
    else { stopListening(); say(""); }
  });
  if (!Recognition) {
    $("mic").classList.add("unsupported");
    $("wake").disabled = true;
    say("voice input needs Chrome or Edge");
  } else {
    $("mic").title = "Talk (M). The browser's recognizer may send audio to its vendor's service.";
  }

  // ---- stopping ------------------------------------------------------------------------------------------------
  function stopEverything() {
    interruptSpeech();
    return api("/runs?limit=50").then(function (list) {
      return Promise.all(list.runs.filter(function (run) { return !run.outcome; }).map(function (run) {
        return api("/runs/" + encodeURIComponent(run.run_id) + "/cancel", "POST").catch(function () {});
      }));
    }).then(refresh, refresh);
  }
  $("stop").addEventListener("click", function () {
    $("stop").disabled = true;
    stopEverything().then(function () { $("stop").disabled = false; });
  });

  document.addEventListener("keydown", function (event) {
    var typing = document.activeElement && document.activeElement.tagName === "INPUT" && document.activeElement.type === "text";
    if (event.key === "Escape") { interruptSpeech(); if (recognizer && !S.wake) stopListening(); return; }
    if (typing) return;
    if (event.key === "/") { event.preventDefault(); $("input").focus(); }
    else if (event.key === "m" || event.key === "M") { event.preventDefault(); $("mic").click(); }
  });

  // ---- the side panels -----------------------------------------------------------------------------------------
  function age(iso) {
    var seconds = Math.max(0, (Date.now() - Date.parse(iso)) / 1000);
    if (seconds < 60) return Math.floor(seconds) + "s";
    if (seconds < 3600) return Math.floor(seconds / 60) + "m";
    if (seconds < 86400) return Math.floor(seconds / 3600) + "h";
    return Math.floor(seconds / 86400) + "d";
  }
  function ageSpan(iso, suffix) {
    var span = el("span", "dim", age(iso) + (suffix || ""));
    span.dataset.since = iso;
    span.dataset.suffix = suffix || "";
    return span;
  }
  function tickAges() {
    document.querySelectorAll("[data-since]").forEach(function (span) {
      span.textContent = age(span.dataset.since) + span.dataset.suffix;
    });
  }
  function fill(id, nodes, empty) {
    var host = $(id);
    host.replaceChildren();
    if (nodes.length === 0) host.appendChild(el("div", "none", empty));
    nodes.forEach(function (node) { host.appendChild(node); });
  }
  function stopButton(runId) {
    var button = el("button", "danger", "Stop");
    button.addEventListener("click", function () {
      button.disabled = true;
      api("/runs/" + encodeURIComponent(runId) + "/cancel", "POST").then(refresh, refresh);
    });
    return button;
  }
  function copyButton(text) {
    var button = el("button", "", "Copy");
    button.addEventListener("click", function () {
      if (navigator.clipboard) navigator.clipboard.writeText(text);
      button.textContent = "Copied";
    });
    return button;
  }

  function renderPanels(runs, approvals, schedules) {
    var working = runs.filter(function (run) { return !run.outcome && run.state !== "awaiting_approval"; });
    var parked = runs.filter(function (run) { return run.state === "awaiting_approval"; });
    var finished = runs.filter(function (run) { return run.outcome; }).slice(0, 8);
    var upcoming = schedules.filter(function (schedule) { return schedule.enabled; });
    S.needYou = Math.max(approvals.length, parked.length);
    S.working = working.length;

    fill("waiting", approvals.map(function (approval) {
      var item = el("div", "item");
      var row = el("div", "row");
      row.appendChild(el("span", "tag wait", "risk " + approval.risk_level));
      row.appendChild(el("strong", "", approval.tool));
      item.appendChild(row);
      item.appendChild(el("div", "obj", clip(approval.arguments ? JSON.stringify(approval.arguments) : approval.preview, 400)));
      var text = "jarvis approvals approve " + approval.approval_id;
      var line = el("div", "row cmd");
      line.appendChild(el("code", "", text));
      line.appendChild(copyButton(text));
      item.appendChild(line);
      return item;
    }), "nothing needs you");

    fill("working", working.concat(parked).map(function (run) {
      var item = el("div", "item");
      var row = el("div", "row");
      row.appendChild(el("span", "tag " + (run.state === "awaiting_approval" ? "wait" : "work"), run.state));
      row.appendChild(ageSpan(run.started_at));
      row.appendChild(stopButton(run.run_id));
      item.appendChild(row);
      item.appendChild(el("div", "obj", clip(run.objective, 220)));
      return item;
    }), "nothing running");

    fill("scheduled", upcoming.map(function (schedule) {
      var item = el("div", "item");
      var row = el("div", "row");
      row.appendChild(el("span", "tag", schedule.interval_seconds ? "every " + Math.round(schedule.interval_seconds / 60) + "m" : schedule.cadence));
      if (schedule.next_run_at) row.appendChild(el("span", "dim", "next " + new Date(schedule.next_run_at).toLocaleTimeString()));
      item.appendChild(row);
      item.appendChild(el("div", "obj", clip(schedule.objective, 220)));
      return item;
    }), "nothing scheduled");

    fill("recent", finished.map(function (run) {
      var item = el("div", "item");
      var row = el("div", "row");
      row.appendChild(el("span", "tag " + (run.outcome === "succeeded" ? "ok" : run.outcome === "cancelled" ? "" : "bad"), run.outcome));
      row.appendChild(ageSpan(run.started_at, " ago"));
      item.appendChild(row);
      item.appendChild(el("div", "obj", clip(run.objective, 140)));
      if (run.answer) item.appendChild(el("div", "dim", "=> " + clip(plainLine(run.answer), 300)));
      return item;
    }), "nothing yet");
  }

  // ---- header and caption ---------------------------------------------------------------------------------------
  function uiState() {
    if (S.offline) return "offline";
    if (S.needYou > 0) return "waiting";
    if (S.listening) return "listening";
    if (S.speaking) return "speaking";
    if (S.thinking || S.working > 0) return "working";
    return "idle";
  }
  var captions = {
    offline: "offline", idle: "standing by", working: "working", listening: "listening",
    speaking: "speaking", waiting: "needs you"
  };
  function paintHeader() {
    var mode = uiState();
    var caption = $("caption");
    caption.textContent = captions[mode];
    caption.className = mode === "waiting" ? "wait" : mode === "idle" || mode === "offline" ? "" : "live";
    var link = $("pill-link");
    link.className = "pill " + (S.offline ? "bad" : "on");
    link.lastChild.textContent = S.offline ? "offline" : "linked";
    var active = S.working + (S.thinking && !S.working ? 1 : 0);
    var work = $("pill-work");
    work.className = "pill" + (active > 0 ? " work" : "");
    work.lastChild.textContent = active + " working";
    var wait = $("pill-wait");
    wait.className = "pill" + (S.needYou > 0 ? " wait" : "");
    wait.lastChild.textContent = S.needYou + " waiting";
    $("stop").hidden = active + S.needYou === 0;
    document.title = (S.needYou > 0 ? "(" + S.needYou + ") " : "") + "JARVIS";
  }
  setInterval(function () {
    $("clock").textContent = new Date().toLocaleTimeString([], { hour12: false });
  }, 1000);

  // ---- polling --------------------------------------------------------------------------------------------------
  var lastSignature = "";
  function refresh() {
    if (!token) {
      $("message").textContent = "No credential. Open this page with `jarvis hud`.";
      return Promise.resolve();
    }
    return Promise.all([api("/runs?limit=30"), api("/approvals"), api("/schedules")]).then(function (results) {
      S.offline = false;
      $("message").textContent = "";
      var signature = JSON.stringify([results[0].runs, results[1].approvals, results[2].schedules]);
      if (signature !== lastSignature) {
        lastSignature = signature;
        renderPanels(results[0].runs, results[1].approvals, results[2].schedules);
      }
      tickAges();
      paintHeader();
    }).catch(function (error) {
      lastSignature = "";
      S.offline = true;
      $("message").textContent = error.status === 401
        ? "The daemon rejected the credential. Open this page again with `jarvis hud`."
        : "Cannot reach the daemon.";
      paintHeader();
    });
  }

  // ---- the orb --------------------------------------------------------------------------------------------------
  var canvas = $("orb");
  var ctx = canvas.getContext("2d");
  var W = 0, H = 0;
  var palette = {
    idle: [55, 214, 255], working: [55, 214, 255], listening: [107, 226, 160],
    speaking: [150, 180, 255], waiting: [255, 180, 84], offline: [90, 100, 115]
  };
  var current = palette.idle.slice();
  var energy = 0.12;
  var calm = window.matchMedia && matchMedia("(prefers-reduced-motion: reduce)").matches;

  function resize() {
    var box = canvas.getBoundingClientRect();
    var ratio = Math.min(2, window.devicePixelRatio || 1);
    W = box.width; H = box.height;
    canvas.width = Math.max(1, W * ratio);
    canvas.height = Math.max(1, H * ratio);
    ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
  }
  window.addEventListener("resize", resize);
  resize();

  function targetEnergy(mode, t) {
    switch (mode) {
      case "listening": return 0.3 + S.level * 0.9;
      case "speaking": return 0.38 + 0.3 * Math.abs(Math.sin(t * 7.3) * Math.sin(t * 3.1));
      case "working": return 0.5 + 0.1 * Math.sin(t * 2.2);
      case "waiting": return 0.45 + 0.15 * Math.sin(t * 5);
      case "offline": return 0.03;
      default: return 0.12 + 0.03 * Math.sin(t * 1.2);
    }
  }

  function rgba(alpha) {
    return "rgba(" + Math.round(current[0]) + "," + Math.round(current[1]) + "," + Math.round(current[2]) + "," + alpha + ")";
  }

  function draw(t, mode) {
    ctx.clearRect(0, 0, W, H);
    var cx = W / 2, cy = H / 2 - 6, base = Math.min(W, H) * 0.25;
    var spin = calm ? 0.1 : (mode === "working" ? 1.6 : mode === "waiting" ? 0.9 : 0.45);

    var glow = ctx.createRadialGradient(cx, cy, base * 0.15, cx, cy, base * 2.6);
    glow.addColorStop(0, rgba(0.30 + energy * 0.45));
    glow.addColorStop(0.45, rgba(0.07 + energy * 0.12));
    glow.addColorStop(1, rgba(0));
    ctx.fillStyle = glow;
    ctx.fillRect(0, 0, W, H);

    // Three tilted rings, spinning at different rates.
    ctx.lineWidth = 1.2;
    for (var ring = 0; ring < 3; ring += 1) {
      ctx.save();
      ctx.translate(cx, cy);
      ctx.rotate(t * spin * (ring % 2 ? -1 : 1) * (0.5 + ring * 0.3) + ring * 1.1);
      ctx.strokeStyle = rgba(0.22 + energy * 0.35);
      ctx.beginPath();
      ctx.ellipse(0, 0, base * (1.25 + ring * 0.22), base * (0.55 + ring * 0.12), 0, 0, Math.PI * 2);
      ctx.stroke();
      ctx.restore();
    }

    // The waveform ring: its amplitude is the voice (microphone level, or a synthetic one while speaking).
    var points = 120;
    ctx.beginPath();
    for (var i = 0; i <= points; i += 1) {
      var angle = (i / points) * Math.PI * 2;
      var wobble = Math.sin(angle * 7 + t * 3.2) * Math.sin(angle * 3 - t * 1.7);
      var radius = base * 1.02 + wobble * base * (0.06 + energy * 0.34);
      var x = cx + Math.cos(angle) * radius, y = cy + Math.sin(angle) * radius;
      if (i === 0) ctx.moveTo(x, y); else ctx.lineTo(x, y);
    }
    ctx.closePath();
    ctx.lineWidth = 2;
    ctx.strokeStyle = rgba(0.55 + energy * 0.4);
    ctx.shadowColor = rgba(0.9);
    ctx.shadowBlur = 12 + energy * 22;
    ctx.stroke();
    ctx.shadowBlur = 0;

    // Orbiting motes.
    for (var m = 0; m < 46; m += 1) {
      var a = (m / 46) * Math.PI * 2 + t * spin * 0.35 * (1 + (m % 3) * 0.2);
      var r = base * (1.5 + 0.55 * Math.sin(m * 12.9898 + t * 0.4));
      ctx.fillStyle = rgba(0.15 + 0.5 * ((m % 5) / 5) * (0.4 + energy));
      ctx.beginPath();
      ctx.arc(cx + Math.cos(a) * r, cy + Math.sin(a) * r * 0.62, 1 + (m % 3) * 0.6, 0, Math.PI * 2);
      ctx.fill();
    }

    // The core.
    var core = ctx.createRadialGradient(cx - base * 0.25, cy - base * 0.3, base * 0.05, cx, cy, base * (0.8 + energy * 0.25));
    core.addColorStop(0, "rgba(255,255,255,0.95)");
    core.addColorStop(0.35, rgba(0.95));
    core.addColorStop(1, rgba(0.08));
    ctx.fillStyle = core;
    ctx.beginPath();
    ctx.arc(cx, cy, base * (0.78 + energy * 0.2), 0, Math.PI * 2);
    ctx.fill();
  }

  function frame(stamp) {
    var t = stamp / 1000;
    var mode = uiState();
    var target = palette[mode];
    for (var i = 0; i < 3; i += 1) current[i] += (target[i] - current[i]) * 0.08;
    readLevel();
    energy += (targetEnergy(mode, t) - energy) * 0.12;
    draw(t, mode);
    requestAnimationFrame(frame);
  }
  // The orb is also a button: clicking it silences JARVIS.
  canvas.addEventListener("click", interruptSpeech);
  requestAnimationFrame(frame);

  refresh();
  setInterval(refresh, 1500);
  paintHeader();
})();

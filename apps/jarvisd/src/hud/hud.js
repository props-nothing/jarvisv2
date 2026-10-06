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
    wake: false,
    pending: [], announced: null
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
    empty.appendChild(el("p", "", "Type, press the microphone (M), or enable the wake word and say \u201CJarvis, \u2026\u201D. Answer questions with yes or no; \u201CJarvis, stop\u201D cancels everything running."));
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
        // The same label twice (a stream can repeat it) is one chip.
        var existing = view.chips.children;
        for (var i = 0; i < existing.length; i += 1) { if (existing[i].textContent === label) return; }
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
      var reply;
      try {
        reply = await api("/runs", "POST", body);
      } catch (error) {
        // A conversation that no longer exists (a fresh profile, a cleared database) starts a new one rather than failing.
        if (error.status !== 404 || !S.session) throw error;
        S.session = null;
        sessionStorage.removeItem("jarvis-session");
        delete body.session_id;
        reply = await api("/runs", "POST", body);
      }
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
  function speak(text, after) {
    if (!synth) { if (after) after(); return; }
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
      if (index === sentences.length - 1) {
        utterance.onend = utterance.onerror = function () {
          S.speaking = false;
          resumeWake();
          if (after) after();
        };
      }
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

  // Answering a question: a plain yes or no is an answer, and only while something is actually waiting.
  var YES = /^(yes|yeah|yep|yup|sure|ok|okay|approve|approved|allow|confirm|proceed|go ahead|do it)( please| jarvis)?$/i;
  var NO = /^(no|nope|nah|deny|denied|refuse|reject|don't|do not|don't do it)( please| jarvis)?$/i;

  // One click or one word: record the owner's answer and let the run carry on.
  function decide(approvalId, approve, via) {
    return api("/approvals/" + encodeURIComponent(approvalId) + "/decision", "POST",
      { decision: approve ? "approve" : "deny", resume: true, channel: via || "desktop" })
      .then(function () { say(approve ? "approved" : "denied"); },
            function (error) { say("could not record that (" + (error.status || "no reply") + ")"); })
      .then(refresh);
  }

  function answerPending(approve) {
    if (S.pending.length === 1) { decide(S.pending[0].approval_id, approve, "voice"); return; }
    if (S.pending.length === 0) { say("nothing is waiting for an answer"); return; }
    var message = S.pending.length + " things are waiting, so use the buttons to say which.";
    say(message);
    if (S.speak) speak(message);
  }

  // What a spoken utterance means. "stop" is the hands-free kill switch, a yes or no answers a waiting question, and
  // everything else is a message.
  function command(text) {
    var spoken = text.trim().replace(/[.!?]+$/, "");
    if (!spoken) return;
    if (YES.test(spoken)) { answerPending(true); return; }
    if (NO.test(spoken)) { answerPending(false); return; }
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
    // While something waits for an answer, a bare yes or no needs no wake word.
    if (S.pending.length && (YES.test(heard.trim().replace(/[.!?]+$/, "")) || NO.test(heard.trim().replace(/[.!?]+$/, "")))) {
      command(heard);
      return;
    }
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
  function answerButtons(approvalId) {
    var row = el("div", "row cmd");
    var yes = el("button", "yes", "Approve");
    var no = el("button", "danger", "Deny");
    function answer(approve) {
      yes.disabled = true;
      no.disabled = true;
      decide(approvalId, approve, "desktop");
    }
    yes.addEventListener("click", function () { answer(true); });
    no.addEventListener("click", function () { answer(false); });
    row.appendChild(yes);
    row.appendChild(no);
    return row;
  }

  function paintStats(runs, approvals, schedules) {
    var done = runs.filter(function (run) { return run.outcome === "succeeded"; }).length;
    var failed = runs.filter(function (run) { return run.outcome === "failed"; }).length;
    var rows = [
      ["Active runs", runs.filter(function (run) { return !run.outcome; }).length, false],
      ["Needs you", Math.max(approvals.length, runs.filter(function (run) { return run.state === "awaiting_approval"; }).length), approvals.length > 0],
      ["Scheduled", schedules.filter(function (schedule) { return schedule.enabled; }).length, false],
      ["Completed", done, false],
      ["Failed", failed, failed > 0]
    ];
    var host = $("stats");
    host.replaceChildren();
    rows.forEach(function (row) {
      var line = el("div", "stat" + (row[2] ? " hot" : ""));
      line.appendChild(el("span", "", row[0]));
      line.appendChild(el("b", "", String(row[1])));
      host.appendChild(line);
    });
  }
  function renderPanels(runs, approvals, schedules) {
    var working = runs.filter(function (run) { return !run.outcome && run.state !== "awaiting_approval"; });
    var parked = runs.filter(function (run) { return run.state === "awaiting_approval"; });
    var finished = runs.filter(function (run) { return run.outcome; }).slice(0, 8);
    var upcoming = schedules.filter(function (schedule) { return schedule.enabled; });
    S.needYou = Math.max(approvals.length, parked.length);
    S.working = working.length;
    S.pending = approvals;
    paintStats(runs, approvals, schedules);

    fill("waiting", approvals.map(function (approval) {
      var item = el("div", "item");
      var row = el("div", "row");
      row.appendChild(el("span", "tag wait", "risk " + approval.risk_level));
      row.appendChild(el("strong", "", approval.tool));
      item.appendChild(row);
      item.appendChild(el("div", "obj", clip(approval.arguments ? JSON.stringify(approval.arguments) : approval.preview, 400)));
      item.appendChild(answerButtons(approval.approval_id));
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

  // ---- what it can do: the tools, and which of them it asks about ------------------------------------------------------
  var abilitiesAt = 0;
  function renderAbilities(tools) {
    var host = $("abilities");
    var asks = 0, off = 0;
    tools.forEach(function (tool) {
      if (!tool.callable || tool.denied) off += 1; else if (tool.asks_first) asks += 1;
    });
    host.replaceChildren();
    host.appendChild(el("div", "sum", tools.length + " tools  \u2022  " + asks + " ask first  \u2022  " + off + " off"));
    tools.slice().sort(function (a, b) { return String(a.title).localeCompare(String(b.title)); }).forEach(function (tool) {
      var row = el("div", "ability");
      var blocked = !tool.callable || tool.denied;
      var asking = !blocked && tool.asks_first;
      row.appendChild(el("span", "name", tool.title || tool.id));
      row.appendChild(el("span", "tag " + (blocked ? "bad" : asking ? "wait" : "ok"), blocked ? "off" : asking ? "asks" : "runs"));
      row.title = tool.id;
      host.appendChild(row);
    });
  }
  function refreshAbilities() {
    if (Date.now() - abilitiesAt < 20000) return;
    abilitiesAt = Date.now();
    api("/tools").then(function (result) { renderAbilities(result.tools || []); }, function () { abilitiesAt = 0; });
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

  // ---- asking out loud -------------------------------------------------------------------------------------------
  var WORDS = {
    "jarvis.code.run": "run some code", "jarvis.web.fetch": "fetch a web page",
    "jarvis.agent.delegate": "hand a task to a sub-agent", "jarvis.files.read": "read a file"
  };
  // When something new needs an answer and spoken answers are on, say so and listen for the yes or no, so the whole
  // exchange needs no hands. Approvals already waiting when the page opens are not announced again.
  function announce(approvals) {
    var seen = S.announced;
    S.announced = {};
    approvals.forEach(function (approval) { S.announced[approval.approval_id] = true; });
    if (seen === null || !S.speak) return;
    var fresh = approvals.filter(function (approval) { return !seen[approval.approval_id]; });
    if (fresh.length === 0) return;
    var what = fresh.length === 1 ? (WORDS[fresh[0].tool] || "use " + fresh[0].tool) : fresh.length + " things";
    speak("I need your permission to " + what + ". Say yes to allow it, or no.", function () {
      if (!S.wake && Recognition && S.pending.length) startListening(false);
    });
  }

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
      announce(results[1].approvals);
      refreshAbilities();
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

  // ---- the face -------------------------------------------------------------------------------------------------
  // A film-style interface rather than a blob: concentric rings of ticks and segmented arcs turning against each other,
  // the system's name running round a ring, a radar sweep, a radial spectrum that is the voice (microphone level while
  // listening, a synthetic envelope while speaking), and an arc-reactor core. Colour and tempo carry the state.
  var canvas = $("orb");
  var ctx = canvas.getContext("2d");
  var W = 0, H = 0;
  var palette = {
    idle: [94, 227, 255], working: [94, 227, 255], listening: [107, 226, 160],
    speaking: [159, 184, 255], waiting: [255, 180, 84], offline: [90, 100, 115]
  };
  var current = palette.idle.slice();
  var energy = 0.12;
  var calm = window.matchMedia && matchMedia("(prefers-reduced-motion: reduce)").matches;
  // Where the pointer is, relative to the face, so the eyes and the head turn towards it.
  var gaze = [0, 0], gazeAim = [0, 0];
  window.addEventListener("pointermove", function (event) {
    var box = canvas.getBoundingClientRect();
    gazeAim[0] = Math.max(-1, Math.min(1, (event.clientX - (box.left + box.width / 2)) / (window.innerWidth * 0.45)));
    gazeAim[1] = Math.max(-1, Math.min(1, -(event.clientY - (box.top + box.height / 2)) / (window.innerHeight * 0.45)));
  });
  var NAME = "J.A.R.V.I.S  \u2022  JUST A RATHER VERY INTELLIGENT SYSTEM  \u2022  ";
  var TAU = Math.PI * 2;

  function resize() {
    var box = canvas.getBoundingClientRect();
    var ratio = Math.min(2, window.devicePixelRatio || 1);
    W = box.width; H = box.height;
    canvas.width = Math.max(1, Math.round(W * ratio));
    canvas.height = Math.max(1, Math.round(H * ratio));
    ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
  }
  if (window.ResizeObserver) new ResizeObserver(resize).observe(canvas); else window.addEventListener("resize", resize);
  resize();

  function targetEnergy(mode, t) {
    switch (mode) {
      case "listening": return 0.28 + S.level * 1.1;
      case "speaking": return 0.4 + 0.35 * Math.abs(Math.sin(t * 7.3) * Math.sin(t * 3.1));
      case "working": return 0.45 + 0.12 * Math.sin(t * 2.2);
      case "waiting": return 0.42 + 0.16 * Math.sin(t * 5);
      case "offline": return 0.02;
      default: return 0.13 + 0.03 * Math.sin(t * 1.2);
    }
  }
  function rgba(alpha) {
    return "rgba(" + Math.round(current[0]) + "," + Math.round(current[1]) + "," + Math.round(current[2]) + "," + alpha + ")";
  }

  function ring(cx, cy, radius, from, to, width, alpha) {
    ctx.beginPath();
    ctx.arc(cx, cy, radius, from, to);
    ctx.lineWidth = width;
    ctx.strokeStyle = rgba(alpha);
    ctx.stroke();
  }
  function ticks(cx, cy, inner, outer, count, rotation, every, alpha) {
    ctx.lineWidth = 1;
    for (var i = 0; i < count; i += 1) {
      var angle = rotation + (i / count) * TAU;
      var long = i % every === 0;
      var r1 = long ? inner - (outer - inner) * 0.6 : inner;
      ctx.strokeStyle = rgba(long ? alpha * 1.7 : alpha);
      ctx.beginPath();
      ctx.moveTo(cx + Math.cos(angle) * r1, cy + Math.sin(angle) * r1);
      ctx.lineTo(cx + Math.cos(angle) * outer, cy + Math.sin(angle) * outer);
      ctx.stroke();
    }
  }
  // Text laid along a ring so that it fills exactly one turn: repeated as many whole times as fit, and the
  // remaining slack spread between the letters, so the end meets the beginning instead of overprinting it.
  function arcText(text, cx, cy, radius, start, size, alpha) {
    ctx.font = size + "px Consolas, 'Cascadia Mono', monospace";
    ctx.fillStyle = rgba(alpha);
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    var widths = [], total = 0, i;
    for (i = 0; i < text.length; i += 1) {
      widths.push(ctx.measureText(text[i]).width + 3);
      total += widths[i];
    }
    var turn = total / radius;
    var copies = Math.max(1, Math.floor(TAU / turn));
    var slack = (TAU / copies - turn) / text.length;
    var angle = start;
    for (var c = 0; c < copies; c += 1) {
      for (i = 0; i < text.length; i += 1) {
        var step = widths[i] / radius + slack;
        if (angle - start > TAU) return;
        ctx.save();
        ctx.translate(cx + Math.cos(angle + step / 2) * radius, cy + Math.sin(angle + step / 2) * radius);
        ctx.rotate(angle + step / 2 + Math.PI / 2);
        ctx.fillText(text[i], 0, 0);
        ctx.restore();
        angle += step;
      }
    }
  }
  function bracket(x, y, dx, dy, size) {
    ctx.beginPath();
    ctx.moveTo(x + dx * size, y);
    ctx.lineTo(x, y);
    ctx.lineTo(x, y + dy * size);
    ctx.stroke();
  }

  function draw(t, mode) {
    ctx.clearRect(0, 0, W, H);
    var cx = W / 2, cy = H / 2 - 10, R = Math.min(W * 0.5, (H - 24) * 0.5) * 0.96;
    if (R < 40) return;
    var speed = calm ? 0.06 : (mode === "working" ? 1.7 : mode === "waiting" ? 1.0 : mode === "offline" ? 0.05 : mode === "idle" ? 0.45 : 0.9);
    ctx.lineCap = "butt";

    // halo
    var halo = ctx.createRadialGradient(cx, cy, R * 0.1, cx, cy, R * 1.25);
    halo.addColorStop(0, rgba(0.22 + energy * 0.35));
    halo.addColorStop(0.55, rgba(0.06 + energy * 0.08));
    halo.addColorStop(1, rgba(0));
    ctx.fillStyle = halo;
    ctx.fillRect(0, 0, W, H);

    // horizontal sight lines with end ticks, and corner brackets
    ctx.lineWidth = 1;
    ctx.strokeStyle = rgba(0.22);
    ctx.beginPath();
    ctx.moveTo(8, cy); ctx.lineTo(cx - R * 1.06, cy);
    ctx.moveTo(cx + R * 1.06, cy); ctx.lineTo(W - 8, cy);
    ctx.stroke();
    for (var k = 0; k < 7; k += 1) {
      var gap = 10 + k * 14;
      ctx.beginPath();
      ctx.moveTo(cx - R * 1.06 - gap, cy - (k % 2 ? 3 : 6)); ctx.lineTo(cx - R * 1.06 - gap, cy + (k % 2 ? 3 : 6));
      ctx.moveTo(cx + R * 1.06 + gap, cy - (k % 2 ? 3 : 6)); ctx.lineTo(cx + R * 1.06 + gap, cy + (k % 2 ? 3 : 6));
      ctx.stroke();
    }
    ctx.strokeStyle = rgba(0.5);
    ctx.lineWidth = 1.5;
    bracket(6, 6, 1, 1, 16); bracket(W - 6, 6, -1, 1, 16); bracket(6, H - 6, 1, -1, 16); bracket(W - 6, H - 6, -1, -1, 16);
    ctx.font = "10px Consolas, monospace";
    ctx.fillStyle = rgba(0.55);
    ctx.textBaseline = "alphabetic";
    ctx.textAlign = "left";
    ctx.fillText(S.session ? "SESSION " + String(S.session).slice(-6).toUpperCase() : "NO SESSION", 22, 24);
    ctx.textAlign = "right";
    ctx.fillText(S.working + (S.thinking && !S.working ? 1 : 0) + " ACTIVE  \u2022  " + S.needYou + " WAITING", W - 22, 24);

    // outer ticked ring (fine ticks, a long one every 10)
    ring(cx, cy, R, 0, TAU, 1.5, 0.55 + energy * 0.3);
    ticks(cx, cy, R * 0.955, R * 0.99, 180, t * speed * 0.05, 10, 0.32);

    // segmented ring turning the other way: eighteen arcs of changing length
    var seg = R * 0.9;
    for (var a = 0; a < 18; a += 1) {
      var start = -t * speed * 0.16 + (a / 18) * TAU;
      var length = (TAU / 18) * (0.35 + 0.5 * Math.abs(Math.sin(a * 2.399)));
      ring(cx, cy, seg, start, start + length, 5, 0.18 + 0.4 * Math.abs(Math.sin(a * 1.7 + t * 0.4)));
    }

    // the system's name, running round
    arcText(NAME, cx, cy, R * 0.815, t * speed * 0.07, Math.max(9, R * 0.045), 0.55 + energy * 0.25);
    ring(cx, cy, R * 0.77, 0, TAU, 1, 0.3);

    // three heavy partial arcs, turning at different rates (faster while working)
    for (var h = 0; h < 3; h += 1) {
      var base = t * speed * (h % 2 ? -0.7 : 0.5) * (1 + h * 0.35) + h * 2.1;
      var rad = R * (0.72 - h * 0.055);
      ring(cx, cy, rad, base, base + 1.15 + 0.5 * Math.sin(t * 0.6 + h), 4 - h * 0.7, 0.6 + energy * 0.3);
      ring(cx, cy, rad, base + 3.3, base + 3.3 + 0.55, 2, 0.5);
    }

    // the thin ring and ticks that frame the head, then the voice as a ring of bars just inside the arcs
    ring(cx, cy, R * 0.58, 0, TAU, 1, 0.3);
    ticks(cx, cy, R * 0.545, R * 0.575, 72, -t * speed * 0.08, 6, 0.3);
    var bars = 96, inner = R * 0.6;
    for (var b = 0; b < bars; b += 1) {
      var theta = (b / bars) * TAU - Math.PI / 2;
      var mirror = Math.min(b, bars - b);
      var shape = Math.abs(Math.sin(mirror * 0.9 + t * 3.3)) * (0.55 + 0.45 * Math.sin(mirror * 0.31 - t * 2.1));
      var len = R * (0.012 + energy * 0.1 * (0.25 + shape));
      ctx.beginPath();
      ctx.moveTo(cx + Math.cos(theta) * inner, cy + Math.sin(theta) * inner);
      ctx.lineTo(cx + Math.cos(theta) * (inner + len), cy + Math.sin(theta) * (inner + len));
      ctx.lineWidth = Math.max(1.2, R * 0.01);
      ctx.strokeStyle = rgba(0.4 + energy * 0.5);
      ctx.stroke();
    }

    // the head: a real face mesh, whose mouth follows the voice and whose eyes follow the pointer
    if (window.JarvisHead) {
      var mouth = 0;
      if (mode === "speaking") mouth = 0.12 + 0.88 * Math.pow(Math.abs(Math.sin(t * 9.3) * Math.sin(t * 3.7 + 0.8)), 0.7);
      JarvisHead.draw(ctx, cx, cy, R * 1.42, {
        rgb: current, energy: energy, mode: mode, mouth: mouth, look: gaze, t: t, calm: calm
      });
    }
    // markers riding the outer rings
    for (var m = 0; m < 5; m += 1) {
      var ma = t * speed * (0.12 + m * 0.03) * (m % 2 ? -1 : 1) + m * 1.3;
      var mr = R * (m % 2 ? 0.9 : 0.99);
      ctx.fillStyle = rgba(0.95);
      ctx.beginPath();
      ctx.arc(cx + Math.cos(ma) * mr, cy + Math.sin(ma) * mr, 2.6, 0, TAU);
      ctx.fill();
    }
  }

  function frame(stamp) {
    var t = stamp / 1000;
    var mode = uiState();
    var target = palette[mode];
    for (var i = 0; i < 3; i += 1) current[i] += (target[i] - current[i]) * 0.08;
    readLevel();
    energy += (targetEnergy(mode, t) - energy) * 0.12;
    gaze[0] += (gazeAim[0] - gaze[0]) * 0.06;
    gaze[1] += (gazeAim[1] - gaze[1]) * 0.06;
    draw(t, mode);
    requestAnimationFrame(frame);
  }
  // The face is also a button: clicking it silences JARVIS.
  canvas.addEventListener("click", interruptSpeech);
  requestAnimationFrame(frame);

  refresh();
  setInterval(refresh, 1500);
  paintHeader();
})();
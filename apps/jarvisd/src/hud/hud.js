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
    session: null, activity: "",
    offline: true, needYou: 0, working: 0, thinking: 0,
    listening: false, speaking: false, voiceBusy: false, preparing: false, voiceKind: "browser", level: 0, voiceLevel: 0,
    speak: localStorage.getItem("jarvis-speak") === "1",
    wake: false,
    pending: [], announced: null
  };
  var projects = [], projectsAt = 0;

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
    var re = /(`[^`]+`|\*\*[^*]+\*\*|\[[^\]]+\]\(https?:\/\/[^\s)]+\)|https?:\/\/[^\s<>)\]]+)/g, last = 0, match;
    while ((match = re.exec(text))) {
      if (match.index > last) parent.appendChild(document.createTextNode(text.slice(last, match.index)));
      var piece = match[0];
      if (piece[0] === "`") parent.appendChild(el("code", "", piece.slice(1, -1)));
      else if (piece[0] === "*") parent.appendChild(el("strong", "", piece.slice(2, -2)));
      else {
        // A link in an answer is model text: only a plain web address is ever offered, in a new tab, with no opener or referrer.
        var inner = piece[0] === "[" ? /^\[([^\]]+)\]\((.*)\)$/.exec(piece) : null;
        var href = window.JarvisMission ? JarvisMission.safeHref(inner ? inner[2] : piece.replace(/[.,;:!?]+$/, "")) : null;
        if (href) {
          var anchor = el("a", "alink", inner ? inner[1] : piece.replace(/[.,;:!?]+$/, ""));
          anchor.href = href;
          anchor.target = "_blank";
          anchor.rel = "noopener noreferrer";
          parent.appendChild(anchor);
          if (!inner) parent.appendChild(document.createTextNode(piece.slice(piece.replace(/[.,;:!?]+$/, "").length)));
        } else parent.appendChild(document.createTextNode(piece));
      }
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
  // Conversations are kept in this browser (not in a tab), so closing the page and coming back continues where you were, and an
  // earlier conversation can be reopened. Each one remembers its daemon session, so reopening it carries on with the same context.
  var CHATS_KEY = "jarvis-chats", CURRENT_KEY = "jarvis-chat-current", MAX_CHATS = 30;
  var chats = [];
  try { chats = JSON.parse(localStorage.getItem(CHATS_KEY) || "[]"); } catch (ignored) { chats = []; }
  if (!Array.isArray(chats)) chats = [];
  function newChatRecord() {
    return { id: Date.now().toString(36) + Math.random().toString(36).slice(2, 6), session: null, project: null, title: "", updated: Date.now(), transcript: [] };
  }
  var currentChat = chats.filter(function (item) { return item.id === localStorage.getItem(CURRENT_KEY); })[0] || chats[0] || null;
  if (!currentChat) { currentChat = newChatRecord(); chats.unshift(currentChat); }
  var transcript = currentChat.transcript || [];
  S.session = currentChat.session || null;
  function saveChats() {
    try {
      chats = chats.filter(function (item) { return item === currentChat || (item.transcript && item.transcript.length); })
        .sort(function (a, b) { return b.updated - a.updated; }).slice(0, MAX_CHATS);
      localStorage.setItem(CHATS_KEY, JSON.stringify(chats));
      localStorage.setItem(CURRENT_KEY, currentChat.id);
    } catch (ignored) { /* storage full or blocked: the conversation still works, it is just not kept */ }
  }
  function remember(role, text) {
    transcript.push({ role: role, text: text });
    transcript = transcript.slice(-60);
    currentChat.transcript = transcript;
    if (role === "user" && !currentChat.title) currentChat.title = clip(text, 56);
    currentChat.updated = Date.now();
    saveChats();
  }
  function setSession(id) {
    S.session = id;
    currentChat.session = id;
    saveChats();
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
    var ideas = currentChat.project
      ? ["Where does this project stand?", "Plan the next steps and start on the first", "Research this and give me sources"]
      : ["What can you do for me?", "Research the latest news on AI agents and give me sources", "Help me plan a project"];
    var row = el("div", "suggest");
    ideas.forEach(function (idea) {
      var button = el("button", "", idea);
      button.type = "button";
      button.addEventListener("click", function () { send(idea); });
      row.appendChild(button);
    });
    empty.appendChild(row);
    chat.appendChild(empty);
  }
  function renderChat() {
    chat.replaceChildren();
    if (transcript.length === 0) renderEmpty();
    else transcript.forEach(function (entry) { show(entry.role, entry.text); });
    scroll();
  }
  renderChat();

  // ---- new chat, and the earlier ones ------------------------------------------------------------------------------
  function switchChat(target) {
    // The answer on screen belongs to the conversation that asked for it, so wait for it (or Stop) before moving away.
    if (busy) { say("one moment: it is still answering. Press Stop to cut it short."); return false; }
    interruptSpeech();
    currentChat = target;
    transcript = target.transcript || [];
    S.session = target.session || null;
    saveChats();
    renderChat();
    closeChats();
    paintProjectPick();
    return true;
  }
  function newChat() {
    // An empty conversation is already a new one.
    if (!transcript.length) { closeChats(); $("input").focus(); return; }
    var fresh = newChatRecord();
    chats.unshift(fresh);
    if (!switchChat(fresh)) { chats.shift(); return; }
    feel("attentive", 1.4, "nod");
    $("input").focus();
  }
  function closeChats() { $("chats").hidden = true; }
  function renderChats() {
    var host = $("chats");
    host.replaceChildren();
    var shown = chats.filter(function (item) { return item.transcript && item.transcript.length; });
    if (!shown.length) host.appendChild(el("div", "none", "no earlier conversations"));
    shown.forEach(function (item) {
      var row = el("div", "chatrow" + (item === currentChat ? " on" : ""));
      var open = el("button", "chatopen", (item.project ? "[" + item.project + "] " : "") + (item.title || "(untitled)"));
      open.addEventListener("click", function () { switchChat(item); });
      row.appendChild(open);
      row.appendChild(el("span", "dim", age(new Date(item.updated).toISOString())));
      var drop = el("button", "chatdrop", "\u00D7");
      drop.title = "Remove this conversation from this browser";
      drop.addEventListener("click", function () {
        if (item === currentChat) { if (busy) { say("one moment: it is still answering."); return; } }
        chats = chats.filter(function (other) { return other !== item; });
        if (item === currentChat) { var fresh = newChatRecord(); chats.unshift(fresh); currentChat = fresh; transcript = []; S.session = null; renderChat(); paintProjectPick(); }
        saveChats();
        renderChats();
      });
      row.appendChild(drop);
      host.appendChild(row);
    });
  }
  $("new-chat").addEventListener("click", newChat);
  $("chats-btn").addEventListener("click", function () {
    var host = $("chats");
    if (host.hidden) { renderChats(); host.hidden = false; } else closeChats();
  });

  // What a tool call means in words, for the progress line: "writing app/page.tsx" says more than "Requested jarvis.files.write".
  var VERBS = {
    "jarvis.files.write": "writing", "jarvis.files.edit": "editing", "jarvis.files.read": "reading", "jarvis.files.list": "listing", "jarvis.files.search": "searching files",
    "jarvis.web.fetch": "fetching a page", "jarvis.agent.delegate": "handing off a task", "jarvis.agent.result": "collecting a result",
    "jarvis.memory.propose": "noting a memory", "jarvis.code.run": "running code", "jarvis.command.run": "running",
    "jarvis.web.search": "searching the web", "jarvis.memory.search": "checking memory", "jarvis.gmail.search": "searching your mail", "jarvis.gmail.read": "reading an email", "jarvis.calendar.events": "checking your calendar", "jarvis.gmail.send": "sending an email", "jarvis.calendar.create": "adding a calendar event", "jarvis.project.note": "writing a project note", "jarvis.project.list": "checking the projects", "jarvis.memory.correct": "correcting a memory", "jarvis.memory.forget": "forgetting a memory", "jarvis.project.use": "joining a project", "jarvis.project.create": "creating a project", "jarvis.project.update": "updating a project", "jarvis.project.assign_schedule": "filing a schedule under a project", "jarvis.schedule.pause": "pausing a scheduled task", "jarvis.schedule.resume": "resuming a scheduled task", "jarvis.schedule.add": "scheduling a task", "jarvis.schedule.list": "checking the schedule", "jarvis.schedule.remove": "removing a scheduled task"
  };
  function describeCall(tool, target) {
    var verb = VERBS[tool] || String(tool || "working").replace(/^jarvis\./, "");
    return clip(target ? verb + " " + target : verb, 48);
  }
  function duration(seconds) {
    if (seconds < 60) return seconds + "s";
    return Math.floor(seconds / 60) + "m " + (seconds % 60) + "s";
  }
  // Why a run stopped, in words a person can act on. The daemon sends a code and a message; the message alone ("the run
  // failed") says nothing.
  function explainFailure(code, message) {
    if (code === "too_many_tool_calls" || code === "too_many_model_calls") return "It reached its safety ceiling for one task (a very large number of steps). Say \u201Ccontinue\u201D and it picks up where it stopped.";
    if (code === "repeating_the_same_call") return "It got stuck repeating the same step, so I stopped it. Say \u201Ccontinue\u201D to try a different way.";
    return message || "the run failed";
  }

  function pendingAnswer() {
    var view = bubble("jarvis", "jarvis");
    view.body.classList.add("cursor");
    var text = "";
    var queued = false;
    var done = false;
    // A line that keeps you informed during a long task: the step it is on, what it is doing, and how long it has been.
    var status = el("div", "progress", "");
    status.hidden = true;
    view.node.insertBefore(status, view.chips);
    var began = Date.now(), steps = 0, doing = "thinking";
    // A long quiet stretch is spoken about (when the voice is on), so you are not left listening to nothing: at most once a minute.
    var lastSignal = began, lastSpoken = 0, quietHook = null;
    function tick() {
      if (done) return;
      var seconds = Math.round((Date.now() - began) / 1000);
      var quiet = Date.now() - Math.max(lastSignal, lastSpoken);
      if (quietHook && quiet > 45000) { lastSpoken = Date.now(); quietHook(seconds); }
      status.hidden = steps === 0 && seconds < 8;
      status.textContent = (steps ? "step " + steps + " \u00B7 " : "") + doing + " \u00B7 " + duration(seconds);
    }
    var timer = setInterval(function () { if (done) clearInterval(timer); else tick(); }, 1000);
    function paint() {
      queued = false;
      if (done) return;
      view.body.replaceChildren(markdown(text));
      view.body.classList.add("cursor");
      scroll();
    }
    return {
      onQuiet: function (hook) { quietHook = hook; },
      add: function (piece) {
        lastSignal = Date.now();
        text += piece;
        if (!queued) { queued = true; requestAnimationFrame(paint); }
      },
      activity: function (label, isStep) {
        if (isStep) { steps += 1; lastSignal = Date.now(); }
        doing = label;
        S.activity = label;
        tick();
      },
      chip: function (label, waiting) {
        // The same label twice (a stream can repeat it) is one chip.
        var existing = view.chips.children;
        for (var i = 0; i < existing.length; i += 1) { if (existing[i].textContent === label) return; }
        view.chips.hidden = false;
        view.chips.appendChild(el("span", "chip" + (waiting ? " wait" : ""), label));
        // A big task makes dozens of these; the latest few are shown and the rest are counted.
        var all = [].filter.call(view.chips.children, function (chip) { return !chip.classList.contains("more"); });
        var hidden = Math.max(0, all.length - 6);
        all.forEach(function (chip, index) { chip.hidden = index < hidden; });
        var more = view.chips.querySelector(".more");
        if (hidden > 0) {
          if (!more) { more = el("span", "chip more", ""); view.chips.insertBefore(more, view.chips.firstChild); }
          more.textContent = "+" + hidden + " earlier";
        }
      },
      sources: function (links) {
        if (!links.length) return;
        var strip = el("div", "asources");
        strip.appendChild(el("span", "dim", "sources"));
        links.slice(0, 8).forEach(function (link) {
          var node = JarvisMission.chip(link, "mchip");
          if (node) strip.appendChild(node);
        });
        view.node.appendChild(strip);
        scroll();
      },
      finish: function (final) {
        done = true;
        clearInterval(timer);
        S.activity = "";
        if (steps > 0) { status.hidden = false; status.textContent = "done \u00B7 " + steps + " step" + (steps === 1 ? "" : "s") + " \u00B7 " + duration(Math.round((Date.now() - began) / 1000)); }
        else status.hidden = true;
        if (typeof final === "string" && final.length) text = final;
        view.body.classList.remove("cursor");
        view.body.replaceChildren(markdown(text || "(no answer)"));
        remember("jarvis", text || "(no answer)");
        scroll();
        return text;
      },
      fail: function (message, onContinue) {
        done = true;
        clearInterval(timer);
        S.activity = "";
        status.hidden = steps === 0;
        if (steps > 0) status.textContent = "stopped after " + steps + " step" + (steps === 1 ? "" : "s") + " \u00B7 " + duration(Math.round((Date.now() - began) / 1000));
        view.node.classList.add("error");
        view.body.classList.remove("cursor");
        view.body.replaceChildren(document.createTextNode(message));
        if (onContinue) {
          var again = el("button", "yes", "Continue");
          again.addEventListener("click", function () { again.disabled = true; onContinue(); });
          view.body.appendChild(document.createElement("br"));
          view.body.appendChild(again);
        }
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

  async function follow(runId, view, talker) {
    var last = 0, finished = false, attempts = 0, finalText = null, failure = null, failedCode = "";
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
            if (window.JarvisMission) JarvisMission.event(runId, "", frame);
            if (frame.event === "output_delta") { view.add(payload.text || ""); if (talker) feed(talker, payload.text || ""); }
            else if (frame.event === "output_completed") finalText = payload.text;
            else if (frame.event === "tool_requested") {
              var what = describeCall(payload.tool, payload.target);
              view.chip(payload.target ? what : (summary || "using a tool"));
              view.activity(what, true);
            }
            else if (frame.event === "activity_updated" && payload.phase) {
              // The model is working but has said nothing yet (reasoning, or writing a large tool call): show that it is alive.
              if (payload.phase === "reasoning_limit") { view.activity("it was thinking too long, so I asked it to act", false); continue; }
              var size = Math.round((payload.chars || 0) / 4);
              var amount = size >= 1000 ? (size / 1000).toFixed(1) + "k" : String(size);
              view.activity((payload.phase === "reasoning" ? "thinking" : "writing a large call") + " (~" + amount + " tokens)", false);
            }
            else if (frame.event === "state_changed" && summary === "interpreting the tool result") view.activity("checking the result", false);
            else if (frame.event === "state_changed" && summary === "calling the model") view.activity("thinking", false);
            else if (frame.event === "approval_requested") view.chip("waiting for your approval", true);
            else if (frame.event === "run_failed") { failure = explainFailure(payload.error_code, payload.message || summary); failedCode = payload.error_code || ""; }
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
    if (failure) {
      var resumable = failure !== "stopped";
      view.fail(failure === "stopped" ? "Stopped." : "That did not complete: " + failure,
        resumable ? function () { send("Continue where you left off."); } : null);
      if (failure !== "stopped") feel("concerned", 3.5);
      if (talker) interruptSpeech();
      return null;
    }
    var finished = view.finish(finalText);
    if (window.JarvisMission) view.sources(JarvisMission.sourcesOf(runId));
    return finished;
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
    // With the neural voice, speech starts while the answer is still being written (see "the neural voice, streamed").
    var talker = S.speak && neural.enabled ? neuralSession() : null;
    if (talker) {
      view.onQuiet(function (seconds) {
        var minutes = Math.floor(seconds / 60);
        var said = minutes < 1 ? "Still working on it." : "Still working. About " + minutes + (minutes > 1 ? " minutes" : " minute") + " in.";
        // Newlines on both sides make it a sentence of its own even if the answer was mid-sentence.
        feed(talker, "\n" + said + "\n");
      });
    }
    feel("attentive", 1.2, "nod");
    if (window.JarvisMission) JarvisMission.begin();
    busy = true;
    S.thinking = 1;
    try {
      var body = { objective: text };
      if (S.session) body.session_id = S.session;
      if (currentChat.project) body.project_id = currentChat.project;
      var reply;
      try {
        reply = await api("/runs", "POST", body);
      } catch (error) {
        // A conversation that no longer exists (a fresh profile, a cleared database) starts a new one rather than failing.
        if (error.status !== 404 || !(S.session || body.project_id)) throw error;
        if (S.session) { setSession(null); delete body.session_id; }
        try {
          reply = await api("/runs", "POST", body);
        } catch (second) {
          // A project that was deleted: carry on without it rather than failing.
          if (second.status !== 404 || !body.project_id) throw second;
          currentChat.project = null;
          delete body.project_id;
          saveChats();
          paintProjectPick();
          notice("That project no longer exists, so this conversation continues without it.");
          reply = await api("/runs", "POST", body);
        }
      }
      setSession(reply.session_id);
      S.runId = reply.run_id;
      var answer = await follow(reply.run_id, view, talker);
      if (answer) react(answer, !!talker);
      if (talker) { if (answer) endFeed(talker, answer); else interruptSpeech(); }
      else if (answer && S.speak) speak(answer);
    } catch (error) {
      if (talker) interruptSpeech();
      view.fail(error.status === 401 ? "The daemon rejected the credential. Open this page with `jarvis hud`."
        : error.status === 409 ? "This conversation already belongs to another project. Start a new chat to use a different one."
        : "Could not reach the assistant.");
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
  // Two voices. When the daemon has a speech provider configured (ADR-0138) the answer is spoken in that neural voice:
  // the page asks the daemon for audio (the provider's key never leaves the daemon) and plays it, making the next
  // sentence group while the current one plays. Otherwise, or if that fails, the browser's own synthesis is used,
  // choosing the best voice it has. Either way the head's mouth follows what is actually being said.
  var synth = window.speechSynthesis;
  var neural = { enabled: false, voice: "", warnedBasic: false };
  var playing = { token: 0, element: null, controller: null, session: null, context: null, analyser: null, data: null };

  function plain(text) {
    return String(text).replace(/```[\s\S]*?```/g, " code omitted. ").replace(/[`*_#>]/g, "").replace(/\s+/g, " ").trim();
  }

  // Browser voices: a "Natural" or "Online" voice (Edge ships good ones) beats a Google voice, which beats the legacy
  // desktop voices that sound robotic; British male first, because that is the voice this assistant is meant to have.
  function rankVoice(voice) {
    if (!/^en/i.test(voice.lang)) return -1;
    var score = 0;
    if (/natural|neural|online/i.test(voice.name)) score += 100;
    if (/Google/i.test(voice.name)) score += 30;
    if (/en[-_]GB|UK|British/i.test(voice.name + " " + voice.lang)) score += 20;
    if (/Ryan|George|Thomas|Daniel|Oliver|Guy|Davis|Andrew|Brian|Christopher|Eric|Male/i.test(voice.name)) score += 15;
    return score;
  }
  function pickVoice() {
    if (!synth) return null;
    var best = null, top = -1;
    synth.getVoices().forEach(function (voice) {
      var score = rankVoice(voice);
      if (score > top) { top = score; best = voice; }
    });
    if (best && top < 100 && !neural.warnedBasic) {
      neural.warnedBasic = true;
      say("This browser only has a basic voice. For a natural one: use Edge, or add an ElevenLabs key (see getting started).");
    }
    return best;
  }

  // Sentences grouped so the first group is short (it starts playing sooner) and later ones are longer (fewer requests).
  function groups(text) {
    var sentences = text.match(/[^.!?]+[.!?]*\s*/g) || [text];
    var out = [], current = "";
    sentences.forEach(function (sentence) {
      var limit = out.length === 0 ? 140 : 500;
      if (current && (current + sentence).length > limit) { out.push(current.trim()); current = ""; }
      current += sentence;
    });
    if (current.trim()) out.push(current.trim());
    return out;
  }

  function finishSpeaking(after) {
    S.speaking = false;
    resumeWake();
    if (after) after();
  }

  function speakBrowser(text, after) {
    if (!synth) { finishSpeaking(after); return; }
    var sentences = text.slice(0, 1800).match(/[^.!?]+[.!?]*/g) || [];
    if (!sentences.length) { S.speaking = false; return; }
    S.voiceKind = "browser";
    var voice = pickVoice();
    sentences.forEach(function (sentence, index) {
      var utterance = new SpeechSynthesisUtterance(sentence.trim());
      if (voice) { utterance.voice = voice; utterance.lang = voice.lang; }
      utterance.rate = 1;
      utterance.pitch = 1;
      if (index === 0) utterance.onstart = function () { S.speaking = true; };
      if (index === sentences.length - 1) {
        utterance.onend = utterance.onerror = function () { finishSpeaking(after); };
      }
      synth.speak(utterance);
    });
  }

  // ---- the neural voice, streamed ----------------------------------------------------------------------------------
  // Speech starts while the answer is still being written, not after it: each sentence is sent to the daemon as soon as it is
  // complete, the daemon streams the provider's audio straight back, and the page plays it as it arrives (Media Source), with
  // the next sentence already on its way. So the first words are heard about a quarter of a second after the first sentence
  // exists, instead of after the whole answer and the whole first group have been made. S.speaking is true only while sound is
  // actually playing, so the face is not "speaking" while it is still waiting for its voice (S.preparing says that instead).
  var HAS_MSE = !!(window.MediaSource && MediaSource.isTypeSupported && MediaSource.isTypeSupported("audio/mpeg"));
  var MIN_PART = 25, MAX_PART = 1400, BATCH_CHARS = 160, GAP_GRACE_MS = 450, LOOKAHEAD = 2;

  function attachAnalyser(element) {
    try {
      var Context = window.AudioContext || window.webkitAudioContext;
      if (!playing.context) playing.context = new Context();
      playing.analyser = playing.context.createAnalyser();
      playing.analyser.fftSize = 256;
      playing.data = new Uint8Array(playing.analyser.fftSize);
      var source = playing.context.createMediaElementSource(element);
      source.connect(playing.analyser);
      playing.analyser.connect(playing.context.destination);
      if (playing.context.state === "suspended") playing.context.resume();
    } catch (ignored) { playing.analyser = null; }
  }

  // Feeds a streaming response into an <audio> element, resolving once the first audio is buffered (so it can start at once).
  function streamInto(response, element) {
    return new Promise(function (resolve, reject) {
      var source = new MediaSource();
      var url = URL.createObjectURL(source);
      var reader = response.body.getReader();
      var buffer = null, queue = [], finished = false, started = false;
      function pump() {
        if (!buffer || buffer.updating) return;
        if (queue.length) {
          try { buffer.appendBuffer(queue.shift()); } catch (error) { fail(error); }
          return;
        }
        if (finished && source.readyState === "open") { try { source.endOfStream(); } catch (ignored) { /* already ended */ } }
      }
      function fail(error) {
        URL.revokeObjectURL(url);
        if (!started) reject(error); else element.dispatchEvent(new Event("error"));
      }
      source.addEventListener("sourceopen", function () {
        try {
          buffer = source.addSourceBuffer("audio/mpeg");
        } catch (error) { reject(error); return; }
        buffer.addEventListener("updateend", function () {
          if (!started) { started = true; resolve(url); }
          pump();
        });
        (function read() {
          reader.read().then(function (chunk) {
            if (chunk.done) { finished = true; pump(); return; }
            queue.push(chunk.value);
            pump();
            read();
          }, fail);
        })();
      }, { once: true });
      element.src = url;
    });
  }

  // One request for one piece of speech; resolves with an element that is ready to play.
  function openSpeech(session, item) {
    var body = { text: item.text };
    if (item.previous) body.previous_text = item.previous;
    return fetch("/api/v1/speech", {
      method: "POST", cache: "no-store", signal: session.controller.signal,
      headers: { Authorization: "Bearer " + token, "Content-Type": "application/json" },
      body: JSON.stringify(body)
    }).then(function (response) {
      if (!response.ok) { var error = new Error(String(response.status)); error.status = response.status; throw error; }
      var element = new Audio();
      element.preload = "auto";
      if (HAS_MSE && response.body && response.body.getReader) {
        return streamInto(response, element).then(function (url) { return { element: element, url: url }; });
      }
      return response.blob().then(function (blob) {
        var url = URL.createObjectURL(blob);
        element.src = url;
        return { element: element, url: url };
      });
    });
  }

  function alive(session) { return session.turn === playing.token; }

  function neuralSession(after) {
    interruptSpeech();
    stopListening();
    var session = {
      turn: playing.token, controller: new AbortController(), items: [], current: -1, playingItem: null,
      fed: "", consumed: 0, ended: false, after: after, gap: null
    };
    playing.controller = session.controller;
    playing.session = session;
    S.voiceBusy = true;
    S.preparing = true;
    S.voiceKind = "neural";
    return session;
  }

  function endSession(session) {
    if (!alive(session)) return;
    playing.session = null;
    S.voiceBusy = false;
    S.preparing = false;
    S.speaking = false;
    resumeWake();
    if (session.after) session.after();
  }

  // The neural voice failed (key, quota, network, playback): say the rest with the browser's voice, so the answer is still heard.
  function fallbackToBrowser(session, error, rest) {
    if (!alive(session)) return;
    if (error && error.status === 404) neural.enabled = false;
    say("The neural voice is unavailable (" + ((error && error.status) || "no reply") + "), using the browser's voice.");
    var after = session.after;
    playing.token += 1;
    session.controller.abort();
    playing.session = null;
    S.voiceBusy = false;
    S.preparing = false;
    S.voiceKind = "browser";
    var text = plain(rest);
    if (text) speakBrowser(text, after); else { S.speaking = false; if (after) after(); }
  }

  function remainingText(session, from) {
    var parts = session.items.slice(from).map(function (item) { return item.text; });
    parts.push(plain(session.fed.slice(session.consumed)));
    return parts.join(" ");
  }

  // Keeps the next few pieces loading, and starts the next one playing when its turn comes.
  function drive(session) {
    if (!alive(session)) return;
    var i;
    for (i = Math.max(0, session.current + 1); i < session.items.length && i <= session.current + 1 + LOOKAHEAD; i += 1) {
      (function (item, index) {
        if (item.state !== "idle") return;
        item.state = "loading";
        openSpeech(session, item).then(function (handle) {
          if (!alive(session)) { URL.revokeObjectURL(handle.url); return; }
          item.handle = handle;
          item.state = "ready";
          drive(session);
        }, function (error) {
          if (!alive(session)) return;
          // A first piece that never arrives is a failure to speak at all; later ones fall back from where they were.
          item.state = "failed";
          item.error = error;
          drive(session);
        });
      })(session.items[i], i);
    }
    if (session.playingItem !== null) return;
    var next = session.items[session.current + 1];
    if (!next) {
      if (session.ended) endSession(session);
      return;
    }
    if (next.state === "failed") { fallbackToBrowser(session, next.error, remainingText(session, session.current + 1)); return; }
    if (next.state !== "ready") return;
    play(session, session.current + 1);
  }

  function play(session, index) {
    var item = session.items[index];
    var element = item.handle.element;
    clearTimeout(session.gap);
    session.current = index;
    session.playingItem = index;
    item.state = "playing";
    playing.element = element;
    attachAnalyser(element);
    function done(error) {
      URL.revokeObjectURL(item.handle.url);
      if (playing.element === element) playing.element = null;
      if (!alive(session)) return;
      session.playingItem = null;
      item.state = "done";
      if (error) { fallbackToBrowser(session, null, remainingText(session, index + 1)); return; }
      // A short silence between pieces is not "stopped speaking": the face holds its pose for a moment while the next arrives.
      session.gap = setTimeout(function () {
        if (!alive(session) || session.playingItem !== null) return;
        S.speaking = false;
        S.preparing = !session.ended || session.items.length > session.current + 1;
      }, GAP_GRACE_MS);
      drive(session);
    }
    element.onplaying = function () { if (alive(session)) { S.speaking = true; S.preparing = false; } };
    element.onended = function () { done(null); };
    element.onerror = function () { done(new Error("playback")); };
    var started = element.play();
    if (started && started.catch) started.catch(function () { done(new Error("playback")); });
  }

  function enqueue(session, raw) {
    var spoken = plain(raw);
    if (!spoken) return;
    // The face takes the feeling of what it is about to say (worry for an apology, pleasure for good news).
    if (window.JarvisHead) JarvisHead.speakMood(JarvisHead.mood(spoken));
    var pieces = spoken.length > MAX_PART ? groups(spoken) : [spoken];
    pieces.forEach(function (piece) {
      var last = session.items[session.items.length - 1];
      session.items.push({ text: piece, previous: last ? last.text : "", state: "idle", handle: null, error: null });
    });
    drive(session);
  }

  // Text arrives as the answer is written. A sentence is spoken as soon as it is complete (a full stop followed by a space or
  // a line break), unless it is inside a code block, which is not read out.
  function feed(session, delta) {
    if (!alive(session) || !delta) return;
    session.fed += delta;
    for (;;) {
      var pending = session.fed.slice(session.consumed);
      if ((pending.match(/```/g) || []).length % 2 === 1) return;
      var match = /([.!?]+["')\]]*\s+|\n+)/.exec(pending);
      if (!match) return;
      var cut = match.index + match[0].length;
      var candidate = pending.slice(0, cut);
      var queuedAhead = session.items.length - 1 - session.current;
      // Speak at once when nothing is waiting to be played; otherwise let a few sentences gather into one request.
      if (plain(candidate).length < MIN_PART) {
        var more = /([.!?]+["')\]]*\s+|\n+)/.exec(pending.slice(cut));
        if (!more) return;
        cut += more.index + more[0].length;
        candidate = pending.slice(0, cut);
      }
      if (queuedAhead > 0 && plain(candidate).length < BATCH_CHARS) {
        var next = /([.!?]+["')\]]*\s+|\n+)/.exec(pending.slice(cut));
        if (!next) return;
        cut += next.index + next[0].length;
        candidate = pending.slice(0, cut);
      }
      session.consumed += cut;
      enqueue(session, candidate);
    }
  }

  // The answer is complete: speak whatever is left, and finish when it has been played.
  function endFeed(session, finalText) {
    if (!alive(session)) return;
    var head = session.fed.slice(0, session.consumed);
    var tail = typeof finalText === "string" && finalText.slice(0, session.consumed) === head
      ? finalText.slice(session.consumed) : session.fed.slice(session.consumed);
    session.fed = head + tail;
    session.consumed = session.fed.length;
    session.ended = true;
    if (tail.trim()) enqueue(session, tail); else drive(session);
  }

  function speakNeural(text, after) {
    var session = neuralSession(after);
    feed(session, text.slice(0, 4000));
    endFeed(session, null);
  }
  function speak(text, after) {
    interruptSpeech();
    var spoken = plain(text);
    if (!spoken) return;
    stopListening();
    if (neural.enabled) speakNeural(spoken, after); else speakBrowser(spoken, after);
  }

  // How loud the voice is right now, from the audio actually playing; the head's mouth follows it.
  function readVoiceLevel() {
    var level = 0;
    if (playing.analyser && playing.element && !playing.element.paused) {
      playing.analyser.getByteTimeDomainData(playing.data);
      var sum = 0;
      for (var i = 0; i < playing.data.length; i += 1) { var v = (playing.data[i] - 128) / 128; sum += v * v; }
      level = Math.min(1, Math.sqrt(sum / playing.data.length) * 5);
    }
    S.voiceLevel += (level - S.voiceLevel) * 0.5;
  }

  // The "you can interrupt it" half of voice: the microphone, Escape, the orb, or a new message silences it at once.
  function interruptSpeech() {
    var was = S.speaking || S.voiceBusy;
    playing.token += 1;
    if (playing.session) {
      clearTimeout(playing.session.gap);
      playing.session.items.forEach(function (item) { if (item.handle) URL.revokeObjectURL(item.handle.url); });
      playing.session = null;
    }
    if (playing.controller) { playing.controller.abort(); playing.controller = null; }
    if (playing.element) { playing.element.onended = playing.element.onerror = playing.element.onplaying = null; playing.element.pause(); playing.element = null; }
    if (synth && (synth.speaking || synth.pending)) synth.cancel();
    S.speaking = false;
    S.voiceBusy = false;
    S.preparing = false;
    if (was) resumeWake();
  }
  // Whether the daemon has a speech provider; asked once the credential is known.
  function learnVoice() {
    if (!token) return;
    api("/speech").then(function (reply) {
      neural.enabled = !!(reply && reply.enabled);
      if (neural.enabled) neural.voice = reply.voice || "";
      $("speak").disabled = !(neural.enabled || synth);
    }, function () {});
  }  $("speak").checked = S.speak;
  $("speak").addEventListener("change", function () {
    S.speak = $("speak").checked;
    localStorage.setItem("jarvis-speak", S.speak ? "1" : "0");
    if (!S.speak) interruptSpeech();
  });
  if (!synth) { $("speak").disabled = true; }
  learnVoice();

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
  // Tell the face what just happened; it decides how to look about it. Absent when the face script did not load.
  function feel(name, seconds, gesture) {
    if (!window.JarvisHead) return;
    if (name) JarvisHead.emote(name, seconds);
    if (gesture === "nod") JarvisHead.nod();
    else if (gesture === "shake") JarvisHead.shake();
  }
  // An answer carries a feeling (sorry, glad, a question), which the face holds while it speaks and shows briefly if it does not.
  function react(text, streamed) {
    if (!window.JarvisHead) return;
    var mood = JarvisHead.mood(text);
    // A streamed answer sets the mood sentence by sentence as it is spoken; this is for a voice that speaks it whole.
    if (!streamed) JarvisHead.speakMood(mood);
    if (!S.speak) JarvisHead.emote(mood, 3);
    if (mood === "pleased" || mood === "amused") JarvisHead.nod();
  }
  // A line in the conversation itself, because the hint under the box is easy to miss and an answer you gave (a yes, a
  // Keep) should visibly land. Kept in the transcript so it survives a reload.
  function notice(text) {
    show("system", text);
    remember("system", text);
    scroll();
  }

  // Answering a question: a plain yes or no is an answer, and only while something is actually waiting.
  var YES = /^(yes|yeah|yep|yup|sure|ok|okay|approve|approved|allow|confirm|proceed|go ahead|do it)( please| jarvis)?$/i;
  var NEWCHAT = /^(new (chat|conversation|session)|start (a )?new (chat|conversation|session)|start over|clear (the )?chat)( please| jarvis)?$/i;
  var NO = /^(no|nope|nah|deny|denied|refuse|reject|don't|do not|don't do it)( please| jarvis)?$/i;

  // One click or one word: record the owner's answer and let the run carry on.
  function decide(approvalId, approve, via) {
    var asked = S.pending.filter(function (approval) { return approval.approval_id === approvalId; })[0];
    var what = asked && asked.tool ? asked.tool : "that";
    return api("/approvals/" + encodeURIComponent(approvalId) + "/decision", "POST",
      { decision: approve ? "approve" : "deny", resume: true, channel: via || "desktop" })
      .then(function () {
        say(approve ? "approved" : "denied");
        // Only when this was the one thing waiting, so a second pending question keeps its own chip.
        if (S.pending.length <= 1) document.querySelectorAll(".chip.wait").forEach(function (chip) {
          chip.textContent = approve ? "approved" : "denied";
          chip.className = "chip " + (approve ? "done" : "refused");
        });
        notice(approve ? "Approved " + what + ". Carrying on." : "Denied " + what + ". It was not done.");
        if (approve) feel("pleased", 2.2, "nod"); else feel("neutral", 1.8, "shake");
      },
      function (error) {
        say("could not record that (" + (error.status || "no reply") + ")");
        notice("Could not record that answer (" + (error.status || "no reply") + "). It is still waiting.");
      })
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
    if (NEWCHAT.test(spoken)) { newChat(); return; }
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
    feel("alert", 1.6, "shake");
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
    if (event.key === "Escape") {
      if (!$("ops").hidden) { setOps(false); return; }
      if (!$("chats").hidden) { closeChats(); return; }
      interruptSpeech(); if (recognizer && !S.wake) stopListening(); return;
    }
    if (typing) return;
    if (event.key === "o" || event.key === "O") { event.preventDefault(); setOps($("ops").hidden); }
    else if (event.key === "n" || event.key === "N") { event.preventDefault(); newChat(); }
    else if (event.key === "/") { event.preventDefault(); $("input").focus(); }
    else if (event.key === "m" || event.key === "M") { event.preventDefault(); $("mic").click(); }
  });

  // ---- the Ops page: everything that is not the face or the conversation --------------------------------------------
  function setOps(open) {
    $("ops").hidden = !open;
    if (open) { abilitiesAt = 0; if (typeof refreshAbilities === "function") refreshAbilities(); loadProjects(); }
  }
  $("open-ops").addEventListener("click", function () { setOps($("ops").hidden); });
  $("ops-close").addEventListener("click", function () { setOps(false); });
  $("ops").addEventListener("click", function (event) { if (event.target === $("ops")) setOps(false); });

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
    document.querySelectorAll("[data-until]").forEach(function (span) { span.textContent = until(span.dataset.until); });
    document.querySelectorAll("[data-now]").forEach(function (line) {
      var doing = window.JarvisMission ? JarvisMission.nowFor(line.dataset.now) : "";
      line.textContent = doing;
      line.hidden = !doing;
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
  function answerButtons(approvalId, tool) {
    var row = el("div", "row cmd");
    var yes = el("button", "yes", "Approve");
    var no = el("button", "danger", "Deny");
    function answer(approve) {
      yes.disabled = true;
      no.disabled = true;
      if (always) always.disabled = true;
      decide(approvalId, approve, "desktop");
    }
    // "Always": the owner's own click trusts this tool from now on (the same setting as Settings, Permissions), then approves.
    // It applies at once; anything that talks to other people still asks, whatever is chosen.
    var always = tool ? el("button", "", "Always allow") : null;
    if (always) {
      always.title = "Approve, and stop asking for " + tool + ". Change it any time in Settings, Permissions.";
      always.addEventListener("click", function () {
        yes.disabled = true;
        no.disabled = true;
        always.disabled = true;
        putJson("/settings/tools/" + encodeURIComponent(tool), "PUT", { posture: "trusted" }).then(function (reply) {
          notice(tool + " will no longer ask" + (reply.applied ? "." : " after a restart.") + " Change it in Settings, Permissions.");
          decide(approvalId, true, "desktop");
        }, function () {
          notice("Could not change the permission for " + tool + ". Nothing was approved.");
          yes.disabled = false;
          no.disabled = false;
          always.disabled = false;
        });
      });
    }
    yes.addEventListener("click", function () { answer(true); });
    no.addEventListener("click", function () { answer(false); });
    row.appendChild(yes);
    if (always) row.appendChild(always);
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
      ["Projects", projects.filter(function (project) { return project.status === "active"; }).length, false],
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
  // ---- the cards on the Ops page ------------------------------------------------------------------------------------
  function plainObjective(text) { return String(text || "").replace(/^\[(scheduled|sub-agent)\]\s*/, ""); }
  function span(from, to) {
    var seconds = Math.max(0, Math.round((Date.parse(to) - Date.parse(from)) / 1000));
    return duration(seconds);
  }
  function until(iso) {
    var seconds = Math.round((Date.parse(iso) - Date.now()) / 1000);
    if (seconds <= 0) return "due now";
    if (seconds < 90) return "in " + seconds + "s";
    if (seconds < 5400) return "in " + Math.round(seconds / 60) + "m";
    if (seconds < 172800) return "in " + Math.round(seconds / 3600) + "h";
    return "in " + Math.round(seconds / 86400) + "d";
  }
  function every(seconds) {
    if (seconds % 86400 === 0) return "every " + seconds / 86400 + "d";
    if (seconds % 3600 === 0) return "every " + seconds / 3600 + "h";
    if (seconds % 60 === 0) return "every " + seconds / 60 + "m";
    return "every " + seconds + "s";
  }
  function scheduleCard(schedule) {
    var item = el("div", "item");
    var row = el("div", "row");
    row.appendChild(el("span", "tag" + (schedule.enabled ? " work" : ""), schedule.interval_seconds ? every(schedule.interval_seconds) : schedule.cadence));
    if (!schedule.enabled) row.appendChild(el("span", "tag", schedule.fire_count > 0 && schedule.cadence === "once" ? "done" : "paused"));
    if (schedule.project) row.appendChild(el("span", "tag ok", schedule.project));
    if (schedule.next_run_at) {
      var next = el("span", "dim", until(schedule.next_run_at));
      next.dataset.until = schedule.next_run_at;
      row.appendChild(next);
    }
    item.appendChild(row);
    item.appendChild(el("div", "obj", clip(schedule.objective, 220)));
    var meta = el("div", "meta");
    meta.appendChild(el("span", "dim", "ran " + schedule.fire_count + (schedule.fire_count === 1 ? " time" : " times")
      + (schedule.skipped_count ? ", skipped " + schedule.skipped_count : "")));
    if (schedule.last_fired_at) meta.appendChild(ageSpan(schedule.last_fired_at, " ago"));
    item.appendChild(meta);
    var acts = el("div", "sched-act");
    var filed = projects.filter(function (project) { return project.status !== "done" || project.name === schedule.project; });
    if (filed.length) {
      var pick = el("select", "sched-project");
      pick.title = "The project this task belongs to: its runs are told the project's goal and guidance";
      var none = el("option", "", "No project");
      none.value = "";
      pick.appendChild(none);
      filed.forEach(function (project) {
        var option = el("option", "", project.name);
        option.value = project.name;
        pick.appendChild(option);
      });
      pick.value = schedule.project || "";
      pick.addEventListener("change", function () {
        pick.disabled = true;
        putJson("/schedules/" + encodeURIComponent(schedule.schedule_id) + "/project", "POST", pick.value ? { project_id: pick.value } : {}).then(function () { lastSignature = ""; refresh(); },
          function () { pick.disabled = false; pick.value = schedule.project || ""; notice("Could not move that task to the project."); });
      });
      acts.appendChild(pick);
    }
    if (schedule.enabled || schedule.cadence !== "once") {
      var toggle = el("button", "", schedule.enabled ? "Pause" : "Resume");
      toggle.addEventListener("click", function () {
        toggle.disabled = true;
        api("/schedules/" + encodeURIComponent(schedule.schedule_id) + "/" + (schedule.enabled ? "pause" : "resume"), "POST").then(refresh, refresh);
      });
      acts.appendChild(toggle);
    }
    var drop = el("button", "danger", "Remove");
    drop.addEventListener("click", function () {
      if (drop.textContent === "Remove") { drop.textContent = "Really remove?"; setTimeout(function () { drop.textContent = "Remove"; }, 4000); return; }
      drop.disabled = true;
      api("/schedules/" + encodeURIComponent(schedule.schedule_id), "DELETE").then(refresh, refresh);
    });
    acts.appendChild(drop);
    item.appendChild(acts);
    return item;
  }
  var openRuns = {}, showFinished = false;
  function recentCard(run) {
    var item = el("div", "item");
    var row = el("div", "row");
    row.appendChild(el("span", "tag " + (run.outcome === "succeeded" ? "ok" : run.outcome === "cancelled" ? "" : "bad"), run.outcome));
    if (tagOf(run.objective)) row.appendChild(el("span", "tag", tagOf(run.objective)));
    row.appendChild(ageSpan(run.started_at, " ago"));
    if (run.completed_at) row.appendChild(el("span", "dim", "took " + span(run.started_at, run.completed_at)));
    item.appendChild(row);
    item.appendChild(el("div", "obj", clip(plainObjective(run.objective), 140)));
    if (!run.answer) return item;
    var preview = el("div", "dim", "=> " + clip(plainLine(run.answer), 300));
    item.appendChild(preview);
    item.dataset.open = "1";
    item.title = "Click to read the whole answer";
    var full = null;
    function toggle(open) {
      if (open && !full) {
        full = el("div", "full");
        full.appendChild(markdown(run.answer));
        item.appendChild(full);
      } else if (!open && full) {
        full.remove();
        full = null;
      }
      preview.hidden = open;
      item.classList.toggle("open", open);
      openRuns[run.run_id] = open;
    }
    item.addEventListener("click", function (event) {
      if (event.target.closest("a, button")) return;
      toggle(!full);
    });
    if (openRuns[run.run_id]) toggle(true);
    return item;
  }
  function renderPanels(runs, approvals, schedules) {
    var working = runs.filter(function (run) { return !run.outcome && run.state !== "awaiting_approval"; });
    var parked = runs.filter(function (run) { return run.state === "awaiting_approval"; });
    var finished = runs.filter(function (run) { return run.outcome; }).slice(0, 8);
    S.needYou = Math.max(approvals.length, parked.length);
    S.working = working.length;
    S.pending = approvals;
    paintStats(runs, approvals, schedules);

    function approvalCard(approval) {
      var item = el("div", "item");
      var row = el("div", "row");
      row.appendChild(el("span", "tag wait", "risk " + approval.risk_level));
      row.appendChild(el("strong", "", approval.tool));
      item.appendChild(row);
      item.appendChild(el("div", "obj", clip(approval.arguments ? JSON.stringify(approval.arguments) : approval.preview, 400)));
      item.appendChild(answerButtons(approval.approval_id, approval.tool));
      return item;
    }
    fill("waiting", approvals.map(approvalCard), "nothing needs you");
    // The same questions sit on the face page too, so nothing waits on you out of sight.
    $("tray-waiting").replaceChildren.apply($("tray-waiting"), approvals.map(approvalCard));
    var needs = Math.max(approvals.length, parked.length);
    var badge = $("ops-count");
    badge.hidden = needs === 0;
    badge.textContent = String(needs);

    fill("working", working.concat(parked).map(function (run) {
      var item = el("div", "item");
      var row = el("div", "row");
      row.appendChild(el("span", "tag " + (run.state === "awaiting_approval" ? "wait" : "work"), run.state === "awaiting_approval" ? "needs you" : "working"));
      if (tagOf(run.objective)) row.appendChild(el("span", "tag", tagOf(run.objective)));
      row.appendChild(ageSpan(run.started_at));
      row.appendChild(el("span", "spacer"));
      row.appendChild(stopButton(run.run_id));
      item.appendChild(row);
      item.appendChild(el("div", "obj", clip(plainObjective(run.objective), 220)));
      // What it is doing this second, kept fresh by the page (see tickAges).
      var now = el("div", "nowline", "");
      now.dataset.now = run.run_id;
      item.appendChild(now);
      return item;
    }), "nothing running");

    // Tasks that will run come first; one-offs that already ran are put away behind a toggle so they do not push the rest down.
    var recurring = schedules.filter(function (schedule) { return schedule.enabled || schedule.cadence !== "once"; })
      .sort(function (a, b) { return (b.enabled ? 1 : 0) - (a.enabled ? 1 : 0); });
    var finishedOnce = schedules.filter(function (schedule) { return !schedule.enabled && schedule.cadence === "once"; });
    fill("scheduled", recurring.concat(showFinished ? finishedOnce : []).map(scheduleCard), "nothing scheduled: ask JARVIS to \u201Cdo this every morning\u201D");
    if (finishedOnce.length) {
      var more = el("button", "", showFinished ? "Hide finished one-off tasks" : "Show " + finishedOnce.length + " finished one-off task" + (finishedOnce.length === 1 ? "" : "s"));
      more.addEventListener("click", function () { showFinished = !showFinished; lastSignature = ""; refresh(); });
      $("scheduled").appendChild(more);
    }

    fill("recent", finished.map(recentCard), "nothing yet");
  }

  // ---- projects (ADR-0151) ----------------------------------------------------------------------------------------------
  // A project is long-running work with a goal, the owner's standing guidance, a working folder and a journal. A conversation that
  // belongs to one is told all of that on every run, so it is written once here instead of pasted into each request. A project
  // changes what JARVIS is told, never what it may do: approvals and permissions are unchanged.
  function projectNamed(name) {
    return projects.filter(function (project) { return project.name === name || project.project_id === name; })[0] || null;
  }
  function loadProjects() {
    projectsAt = Date.now();
    return api("/projects").then(function (reply) {
      projects = reply.projects || [];
      lastSignature = "";
      paintProjects();
      paintProjectPick();
    }, function () {});
  }
  function paintProjectPick() {
    var pick = $("project-pick");
    var wanted = currentChat.project || "";
    pick.replaceChildren();
    var none = el("option", "", "No project");
    none.value = "";
    pick.appendChild(none);
    var names = projects.filter(function (project) { return project.status !== "done" || project.name === wanted; })
      .map(function (project) { return project.name; });
    if (wanted && names.indexOf(wanted) < 0) names.push(wanted);
    names.forEach(function (name) {
      var option = el("option", "", name);
      option.value = name;
      pick.appendChild(option);
    });
    pick.value = wanted;
    pick.hidden = projects.length === 0 && !wanted;
  }
  function startProjectChat(name) {
    if (busy) { say("one moment: it is still answering. Press Stop to cut it short."); return; }
    setOps(false);
    $("project-sheet").hidden = true;
    if (transcript.length) newChat();
    currentChat.project = name;
    saveChats();
    paintProjectPick();
    $("input").focus();
    say("this conversation belongs to " + name);
  }
  $("project-pick").addEventListener("change", function () {
    var name = $("project-pick").value;
    if (busy) { paintProjectPick(); say("one moment: it is still answering."); return; }
    // A conversation already bound to a project cannot move to another, so choosing one starts a fresh conversation.
    if (transcript.length) newChat();
    currentChat.project = name || null;
    saveChats();
    paintProjectPick();
    say(name ? "new messages here belong to " + name : "no project");
  });
  function projectCard(project) {
    var item = el("div", "item");
    var row = el("div", "row");
    row.appendChild(el("span", "tag " + (project.status === "active" ? "work" : project.status === "done" ? "ok" : ""), project.status));
    row.appendChild(el("strong", "", project.name));
    var open = el("button", "", "Open");
    open.addEventListener("click", function () { openProject(project.project_id); });
    var talk = el("button", "", "Chat");
    talk.title = "Start a conversation in this project";
    talk.addEventListener("click", function () { startProjectChat(project.name); });
    row.appendChild(open);
    row.appendChild(talk);
    item.appendChild(row);
    if (project.goal) item.appendChild(el("div", "obj", clip(project.goal, 160)));
    return item;
  }
  function paintProjects() {
    fill("projects", projects.map(projectCard), "no projects yet: press New project to give JARVIS a goal, guidance and a journal");
  }
  function projectNote(text, bad) {
    var line = $("project-note");
    line.textContent = text || "";
    line.className = bad ? "bad" : "ok";
  }
  function closeProject() { $("project-sheet").hidden = true; }
  function formRow(label, field, help) {
    var row = el("div", "set-row");
    row.appendChild(el("span", "name", label));
    row.appendChild(field);
    if (help) row.appendChild(el("span", "help", help));
    return row;
  }
  function textField(value, placeholder) {
    var field = el("input");
    field.type = "text";
    field.value = value || "";
    field.placeholder = placeholder || "";
    return field;
  }
  function areaField(value, rows, placeholder) {
    var field = el("textarea");
    field.rows = rows;
    field.value = value || "";
    field.placeholder = placeholder || "";
    return field;
  }
  function openProject(id) {
    $("project-sheet").hidden = false;
    projectNote("");
    if (!id) { paintProjectSheet(null, []); return; }
    api("/projects/" + encodeURIComponent(id)).then(function (detail) { paintProjectSheet(detail.project, detail.notes || []); },
      function () { projectNote("Could not open that project.", true); });
  }
  function paintProjectSheet(project, notes) {
    var body = $("project-body");
    body.replaceChildren();
    $("project-title").textContent = project ? project.name : "New project";
    var name = textField(project && project.name, "e.g. Prospecting");
    var goal = areaField(project && project.goal, 3, "What done looks like, in a sentence or two.");
    var folder = textField(project && project.folder, "e.g. sales (inside a folder JARVIS may use)");
    var guidance = areaField(project && project.guidance, 9, "How you want the work done: tone, language, limits, who it may contact, what to avoid. Every run of this project is told this.");
    body.appendChild(formRow("Name", name));
    body.appendChild(formRow("Goal", goal));
    body.appendChild(formRow("Folder", folder, "Where its files live, relative to a folder you granted in Settings, Files. Empty means that folder itself."));
    body.appendChild(formRow("Guidance", guidance, "Your standing instructions. The project cannot widen what JARVIS may do: approvals and permissions stay as they are."));
    var status = null;
    if (project) {
      status = el("select");
      ["active", "paused", "done"].forEach(function (value) {
        var option = el("option", "", value);
        option.value = value;
        status.appendChild(option);
      });
      status.value = project.status;
      body.appendChild(formRow("Status", status, "Paused or done: its scheduled tasks stop firing. You can still chat in it."));
    }
    var acts = el("div", "acts");
    var save = el("button", "yes", project ? "Save" : "Create");
    save.addEventListener("click", function () {
      var fields = { name: name.value, goal: goal.value, folder: folder.value, guidance: guidance.value };
      if (status) fields.status = status.value;
      save.disabled = true;
      putJson(project ? "/projects/" + encodeURIComponent(project.project_id) : "/projects", project ? "PATCH" : "POST", fields).then(function (saved) {
        projectNote(project ? "Saved." : "Created.");
        loadProjects();
        openProject(saved.project_id);
        projectNote(project ? "Saved." : "Created.");
      }, function (error) { projectNote(explain(error, "That was refused."), true); save.disabled = false; });
    });
    acts.appendChild(save);
    if (project) {
      var talk = el("button", "", "Chat in it");
      talk.addEventListener("click", function () { startProjectChat(project.name); });
      acts.appendChild(talk);
      var drop = el("button", "danger", "Delete");
      drop.addEventListener("click", function () {
        if (drop.textContent === "Delete") { drop.textContent = "Really delete?"; return; }
        api("/projects/" + encodeURIComponent(project.project_id), "DELETE").then(function () { closeProject(); loadProjects(); },
          function () { projectNote("Could not delete it.", true); });
      });
      acts.appendChild(drop);
    }
    body.appendChild(el("div", "set-row")).appendChild(acts);
    if (!project) return;
    var card = el("div", "card");
    card.appendChild(el("h4", "", "Journal"));
    card.appendChild(el("p", "dim", "What happened, written by JARVIS as it works and by you. The newest entries are shown to every run."));
    if (!notes.length) card.appendChild(el("div", "none", "no entries yet"));
    notes.forEach(function (entry) {
      var line = el("div", "jline");
      line.appendChild(el("span", "tag", entry.kind));
      line.appendChild(el("span", "dim", entry.created_at.slice(0, 10)));
      line.appendChild(el("span", "jtext", entry.text));
      card.appendChild(line);
    });
    var text = textField("", "Add an entry for JARVIS to read next time");
    var kind = el("select");
    ["owner", "decision", "blocker", "next"].forEach(function (value) {
      var option = el("option", "", value);
      option.value = value;
      kind.appendChild(option);
    });
    var add = el("button", "", "Add");
    add.addEventListener("click", function () {
      if (!text.value.trim()) { projectNote("Write the entry first.", true); return; }
      putJson("/projects/" + encodeURIComponent(project.project_id) + "/notes", "POST", { text: text.value, kind: kind.value }).then(function () {
        openProject(project.project_id);
      }, function (error) { projectNote(explain(error, "That entry was refused."), true); });
    });
    var write = el("div", "jwrite");
    write.appendChild(kind);
    write.appendChild(text);
    write.appendChild(add);
    card.appendChild(write);
    body.appendChild(card);
  }
  $("project-new").addEventListener("click", function () { openProject(null); });
  $("project-close").addEventListener("click", closeProject);
  $("project-sheet").addEventListener("click", function (event) { if (event.target === $("project-sheet")) closeProject(); });
  document.addEventListener("keydown", function (event) {
    if (event.key === "Escape" && !$("project-sheet").hidden) { closeProject(); event.stopPropagation(); }
  }, true);
  loadProjects();

  // ---- things JARVIS offers to remember --------------------------------------------------------------------------------
  // The model may only *propose* a memory (a page it read could say "remember this"), so a proposal waits here until you keep
  // it. Keeping files it again as your own statement, which is what lets it shape later answers; dismissing forgets it.
  var proposalsAt = 0;
  function proposalCard(memory) {
    var item = el("div", "item");
    var row = el("div", "row");
    row.appendChild(el("span", "tag wait", "remember?"));
    item.appendChild(row);
    item.appendChild(el("div", "obj", clip(memory.content, 300)));
    var buttons = el("div", "row cmd");
    var keep = el("button", "yes", "Keep");
    var drop = el("button", "danger", "Dismiss");
    function settle(keepIt) {
      keep.disabled = true;
      drop.disabled = true;
      // The proposal goes first, and on Keep without the block that stops a forgotten claim returning: the same words filed
      // again as the person's own statement would otherwise be recognised as the claim already held and nothing would be saved.
      api("/memories/" + encodeURIComponent(memory.memory_id) + "/forget", "POST", { expected_version: memory.version, allow_relearn: keepIt })
        .then(function () {
          return keepIt
            ? api("/memories", "POST", { content: memory.content, memory_type: memory.memory_type, source_kind: "user_statement" })
            : null;
        })
        .then(function () {
          notice(keepIt ? "Remembered: " + clip(memory.content, 140) : "Dismissed. I will not offer that again.");
          if (keepIt) feel("pleased", 2.2, "nod");
          proposalsAt = 0;
          refreshProposals();
        }, function (error) {
          keep.disabled = false;
          drop.disabled = false;
          proposalsAt = 0;
          notice("Could not " + (keepIt ? "keep" : "dismiss") + " that (" + (error.status || "no reply") + ").");
        });
    }
    keep.addEventListener("click", function () { settle(true); });
    drop.addEventListener("click", function () { settle(false); });
    buttons.appendChild(keep);
    buttons.appendChild(drop);
    item.appendChild(buttons);
    return item;
  }
  function refreshProposals() {
    if (Date.now() - proposalsAt < 8000) return;
    proposalsAt = Date.now();
    api("/memories?limit=50").then(function (list) {
      var proposed = (list.memories || []).filter(function (memory) { return memory.status === "proposed"; }).slice(0, 5);
      return Promise.all(proposed.map(function (memory) {
        return api("/memories/" + encodeURIComponent(memory.memory_id)).then(function (detail) {
          return { memory_id: memory.memory_id, memory_type: memory.memory_type, version: detail.version, content: detail.content };
        });
      }));
    }).then(function (cards) {
      $("remember").replaceChildren.apply($("remember"), cards.map(proposalCard));
      $("tray-remember").replaceChildren.apply($("tray-remember"), cards.map(proposalCard));
      S.proposals = cards.length;
    }, function () { proposalsAt = 0; });
  }

  // ---- what it can do: the tools, and which of them it asks about ------------------------------------------------------
  var abilitiesAt = 0;
  // Tools in families, so sixteen of them read as eight things: the same order and colours on the Ops page and in Settings.
  var FAMILIES = [
    ["Web", /^jarvis\.web\./, "94,227,255"], ["Files", /^jarvis\.files\./, "107,226,160"], ["Memory", /^jarvis\.memory\./, "176,150,255"],
    ["Sub-agents", /^jarvis\.agent\./, "255,120,214"], ["Commands & code", /^jarvis\.(command|code)\./, "255,138,92"],
    ["Mail & calendar", /^jarvis\.(gmail|calendar)\./, "255,180,84"], ["Schedule", /^jarvis\.schedule\./, "120,240,214"],
    ["Projects", /^jarvis\.project\./, "150,200,255"]
  ];
  function families(tools) {
    var groups = FAMILIES.map(function (family) { return { name: family[0], rgb: family[2], tools: [] }; });
    var other = { name: "Other", rgb: "170,200,220", tools: [] };
    tools.forEach(function (tool) {
      var at = -1;
      FAMILIES.forEach(function (family, index) { if (at < 0 && family[1].test(String(tool.id))) at = index; });
      (at < 0 ? other : groups[at]).tools.push(tool);
    });
    groups.push(other);
    groups.forEach(function (group) { group.tools.sort(function (a, b) { return String(a.title).localeCompare(String(b.title)); }); });
    return groups.filter(function (group) { return group.tools.length; });
  }
  function familyLabel(group) {
    var label = el("div", "glabel");
    var dot = el("i", "gdot");
    dot.style.background = "rgb(" + group.rgb + ")";
    label.appendChild(dot);
    label.appendChild(el("span", "", group.name));
    return label;
  }
  function renderAbilities(tools) {
    var host = $("abilities");
    var asks = 0, off = 0;
    tools.forEach(function (tool) {
      if (!tool.callable || tool.denied) off += 1; else if (tool.asks_first) asks += 1;
    });
    host.replaceChildren();
    host.appendChild(el("div", "sum", tools.length + " tools  \u2022  " + asks + " ask first  \u2022  " + off + " off"));
    families(tools).forEach(function (group) {
      host.appendChild(familyLabel(group));
      group.tools.forEach(function (tool) {
        var row = el("div", "ability");
        var blocked = !tool.callable || tool.denied;
        var asking = !blocked && tool.asks_first;
        row.appendChild(el("span", "name", tool.title || tool.id));
        row.appendChild(el("span", "tag " + (blocked ? "bad" : asking ? "wait" : "ok"), blocked ? "off" : asking ? "asks" : "runs"));
        row.title = tool.id;
        host.appendChild(row);
      });
    });
  }
  function refreshAbilities() {
    if (Date.now() - abilitiesAt < 20000) return;
    abilitiesAt = Date.now();
    api("/tools").then(function (result) { renderAbilities(result.tools || []); }, function () { abilitiesAt = 0; });
  }

  // ---- settings: five tabs, each saying what is set, what the default is, and why a setting is off -------------------------
  // The same checks as `jarvis config` and `jarvis keys` (one code path in the daemon). A key is typed into a password field,
  // sent once, and the field is cleared; the daemon never sends it back, only whether it is set.
  var TABS = [
    ["brain", "Brain", "Which model JARVIS thinks with, and the key for it."],
    ["voice", "Voice", "How JARVIS sounds. Without a voice key it speaks with your browser's own voice."],
    ["files", "Folders & code", "What JARVIS may touch. Both are off until you turn them on, so nothing is reachable by accident."],
    ["permissions", "Permissions", "For each tool: let the rules decide, always ask, run without asking, or switch it off."],
    ["google", "Google", "Sign in with your Google account so JARVIS can read your mail and calendar. Read-only: it can never send, change or delete anything."],
    ["advanced", "Advanced", "Ports. The defaults are right for almost everyone."]
  ];
  var sview = { reply: null, tools: [], tab: "brain" };
  function settingsNote(text, bad) {
    var note = $("settings-note");
    note.textContent = text || "";
    note.className = bad ? "bad" : "ok";
  }
  function failure(error) {
    settingsNote("Could not do that (" + (error.status || "no reply") + ")", true);
  }
  function explain(error, fallback) {
    // The daemon explains a refusal in its error body; show that rather than a status number.
    return error && error.body ? error.body : fallback;
  }
  function putJson(path, method, body) {
    var init = { method: method, cache: "no-store", headers: { Authorization: "Bearer " + token, "Content-Type": "application/json" } };
    if (body !== undefined) init.body = JSON.stringify(body);
    return fetch("/api/v1" + path, init).then(function (response) {
      if (response.ok) return response.json();
      return response.json().catch(function () { return {}; }).then(function (reply) {
        var error = new Error(String(response.status));
        error.status = response.status;
        error.body = reply && reply.message ? reply.message : null;
        throw error;
      });
    });
  }
  function saved(what) {
    settingsNote(what + " saved. Restart to apply.");
    $("settings-restart").className = "hot";
    loadSettings();
  }
  function valuesFor(kind, text) {
    var t = text.trim();
    if (kind === "folders") return t.split(";").map(function (part) { return part.trim(); }).filter(Boolean);
    if (kind === "tools") return t.split(/[\s,]+/).filter(Boolean);
    return [t];
  }
  function showFor(setting) {
    if (setting.value === null || setting.value === undefined) return "";
    // The daemon lists a list value with ", "; folders are shown separated by ";" and a command by plain spaces.
    if (setting.kind === "folders") return String(setting.value).split(", ").join("; ");
    if (setting.kind === "words") return String(setting.value).split(", ").join(" ");
    return String(setting.value);
  }
  function byKey(name) {
    return ((sview.reply && sview.reply.settings) || []).filter(function (s) { return s.key === name; })[0];
  }
  function stateBadge(setting) {
    var set = setting.value !== null && setting.value !== undefined;
    var atDefault = set && setting.default && String(setting.value) === String(setting.default);
    return el("span", "badge " + (set && !atDefault ? "on" : "off"), atDefault ? "default" : set ? "set" : setting.default ? "default" : "off");
  }
  // One setting: its field, Save (and Unset when set), and either its help (when set) or what "unset" means (when not).
  var LABELS = {
    "daemon.executor_model_name": "Model", "daemon.executor_base_url": "Model server", "daemon.executor_reasoning_effort": "Thinking effort",
    "daemon.tool_workspace_roots": "Folders", "daemon.code_sandbox_image": "Sandbox image", "daemon.code_sandbox_interpreter": "Interpreter",
    "daemon.speech_voice_id": "Voice", "daemon.speech_model": "Voice model", "daemon.http_port": "Console port",
    "daemon.mcp_serve_port": "MCP port", "daemon.notifications": "Notifications", "daemon.google_client_id": "Client ID",
    "daemon.google_actions": "Send mail & events", "policy.trust": "Runs without asking"
  };
  function settingRow(setting, label) {
    var row = el("div", "set-row");
    var name = el("span", "name", label && label !== "folders" ? label : LABELS[setting.key] || setting.key.replace(/^(daemon|policy)\./, ""));
    name.title = setting.key;
    name.appendChild(stateBadge(setting));
    var field = el("input");
    field.type = "text";
    field.value = showFor(setting);
    field.placeholder = setting.default ? "default: " + setting.default : (setting.example ? "e.g. " + setting.example : "(not set)");
    var acts = el("div", "acts");
    var save = el("button", "", "Save");
    save.addEventListener("click", function () {
      var values = valuesFor(setting.kind, field.value);
      if (!values.length || !values[0]) { settingsNote("Enter a value, or use Unset.", true); return; }
      putJson("/settings/" + encodeURIComponent(setting.key), "PUT", { values: values }).then(function () {
        saved(setting.key);
      }, function (error) { settingsNote(explain(error, "That value was refused."), true); });
    });
    acts.appendChild(save);
    if (setting.value !== null && setting.value !== undefined) {
      var unset = el("button", "", "Unset");
      unset.addEventListener("click", function () {
        putJson("/settings/" + encodeURIComponent(setting.key), "DELETE").then(function () { saved(setting.key + " removed;"); },
          function (error) { settingsNote(explain(error, "That cannot be removed on its own."), true); });
      });
      acts.appendChild(unset);
    }
    row.appendChild(name);
    row.appendChild(field);
    row.appendChild(acts);
    var set = setting.value !== null && setting.value !== undefined;
    row.appendChild(el("span", "help", set ? setting.help : setting.unset_means));
    return row;
  }
  function keyRow(which, label) {
    var state = sview.reply && sview.reply.keys && sview.reply.keys[which] ? sview.reply.keys[which].state : "unknown";
    var row = el("div", "set-row");
    var name = el("span", "name", label);
    name.appendChild(el("span", "badge " + (state === "set" ? "on" : "off"), state));
    var field = el("input");
    field.type = "password";
    field.autocomplete = "off";
    field.placeholder = state === "set" ? "set; paste a new key to replace it" : "paste a key";
    var acts = el("div", "acts");
    var save = el("button", "", "Save key");
    save.addEventListener("click", function () {
      var value = field.value;
      field.value = "";
      if (!value.trim()) { settingsNote("Paste a key first.", true); return; }
      putJson("/settings/keys/" + which, "PUT", { key: value }).then(function () { saved(label); },
        function (error) { settingsNote(explain(error, "That key was refused."), true); });
    });
    acts.appendChild(save);
    if ((which === "voice" || which === "search") && state === "set") {
      var remove = el("button", "", "Remove");
      remove.addEventListener("click", function () {
        putJson("/settings/keys/" + which, "DELETE").then(function () { saved(label + " removed;"); },
          function (error) { settingsNote(explain(error, "Could not remove it."), true); });
      });
      acts.appendChild(remove);
    }
    row.appendChild(name);
    row.appendChild(field);
    row.appendChild(acts);
    var helps = {
      model: "Stored in a private file on this machine and never shown again. Local Ollama needs no real key.",
      google: "The client secret of your Google OAuth Desktop client. Stored in a private file and never shown again.",
      voice: "An ElevenLabs API key. Stored in a private file and never shown again; what JARVIS says is sent to ElevenLabs to be spoken.",
      search: "An Ollama API key (a free ollama.com account makes one) that turns on web search. Stored in a private file and never shown again; what JARVIS searches for is sent to ollama.com. Check it with `jarvis keys test search`."
    };
    row.appendChild(el("span", "help", helps[which] || ""));
    return row;
  }
  // Two settings that only make sense together are saved together.
  function codeCard() {
    var image = byKey("daemon.code_sandbox_image"), command = byKey("daemon.code_sandbox_interpreter");
    var card = el("div", "card");
    var title = el("h4", "", "Code sandbox");
    title.appendChild(el("span", "badge " + (image && image.value ? "on" : "off"), image && image.value ? "on" : "off"));
    card.appendChild(title);
    card.appendChild(el("p", "lead", image && image.value ? "JARVIS can run code in a disposable container with no network."
      : image.unset_means));
    var row = el("div", "set-row");
    row.appendChild(el("span", "name", "image"));
    var imageField = el("input");
    imageField.type = "text"; imageField.value = showFor(image); imageField.placeholder = "e.g. " + image.example;
    row.appendChild(imageField);
    row.appendChild(el("span"));
    var row2 = el("div", "set-row");
    row2.appendChild(el("span", "name", "command"));
    var commandField = el("input");
    commandField.type = "text"; commandField.value = showFor(command); commandField.placeholder = "e.g. " + command.example;
    row2.appendChild(commandField);
    var acts = el("div", "acts");
    var save = el("button", "", "Save sandbox");
    save.addEventListener("click", function () {
      if (!imageField.value.trim() || !commandField.value.trim()) { settingsNote("Give both the image and the command.", true); return; }
      putJson("/settings", "PUT", { changes: [
        { key: "daemon.code_sandbox_image", values: [imageField.value.trim()] },
        { key: "daemon.code_sandbox_interpreter", values: [commandField.value.trim()] }
      ] }).then(function () { saved("Code sandbox"); }, function (error) { settingsNote(explain(error, "That was refused."), true); });
    });
    acts.appendChild(save);
    if (image && image.value) {
      var off = el("button", "", "Turn off");
      off.addEventListener("click", function () {
        putJson("/settings", "PUT", { changes: [
          { key: "daemon.code_sandbox_image", values: null }, { key: "daemon.code_sandbox_interpreter", values: null }
        ] }).then(function () { saved("Code sandbox turned off;"); }, function (error) { settingsNote(explain(error, "Could not turn it off."), true); });
      });
      acts.appendChild(off);
    }
    row2.appendChild(acts);
    card.appendChild(row);
    card.appendChild(row2);
    return card;
  }
  var POSTURES = [["default", "Default (the rules decide)"], ["ask", "Always ask first"], ["trusted", "Run without asking"], ["off", "Off"]];
  function permissionsPanel(into) {
    var postures = (sview.reply && sview.reply.postures) || {};
    if (!sview.tools.length) { into.appendChild(el("p", "lead", "No tools are available to configure yet.")); return; }
    families(sview.tools).forEach(function (group) {
      into.appendChild(familyLabel(group));
      group.tools.forEach(function (tool) { into.appendChild(permissionRow(tool, postures)); });
    });
    into.appendChild(el("p", "lead", "Anything that talks to other people always asks, and nothing runs above the workspace ceiling, whatever is chosen here."));
  }
  function permissionRow(tool, postures) {
    var row = el("div", "perm");
    var label = el("div");
    label.appendChild(el("div", "tname", tool.title || tool.id));
    label.appendChild(el("div", "tid", tool.id + "  \u2022  risk " + tool.risk + (tool.asks_first ? "  \u2022  asks by default" : "  \u2022  runs by default")));
    var select = el("select");
    POSTURES.forEach(function (option) {
      var node = el("option", "", option[1]);
      node.value = option[0];
      select.appendChild(node);
    });
    select.value = postures[tool.id] || "default";
    select.addEventListener("change", function () {
      putJson("/settings/tools/" + encodeURIComponent(tool.id), "PUT", { posture: select.value }).then(function (reply) { if (reply && reply.applied) { settingsNote(tool.title + ": saved and applied now."); loadSettings(); } else saved(tool.title + ":"); },
        function (error) { settingsNote(explain(error, "That was refused."), true); loadSettings(); });
    });
    row.appendChild(label);
    row.appendChild(select);
    return row;
  }
  // ---- Google: set up once, then one button -----------------------------------------------------------------------------------
  // Google needs an OAuth client the owner makes in their own Google Cloud project (a few minutes, once). After that, signing in is
  // a button: the browser goes to Google, Google returns to this daemon, and the status below turns to "Connected".
  var googlePoll = 0;
  function googlePanel(body) {
    var idSetting = byKey("daemon.google_client_id");
    var help = el("div", "card");
    help.appendChild(el("h4", "", "One-time setup"));
    var steps = el("ol");
    ["Open console.cloud.google.com, create a project, and enable the Gmail API and the Google Calendar API.",
     "In Google Auth platform, set up the consent screen (External is fine), add your own Google address as a test user, and press Publish app (otherwise Google ends the sign-in after 7 days).",
     "Create credentials, OAuth client ID, application type Desktop app. Copy the client ID and the client secret into the two fields below.",
     "Press Sign in with Google. Google warns that the app is unverified: that is expected for your own client; choose Continue."].forEach(function (text) {
      steps.appendChild(el("li", "", text));
    });
    help.appendChild(steps);
    help.appendChild(el("p", "lead", "Details and the reasoning are in docs/user/google.md. Nothing is sent to Google until you press Sign in."));
    body.appendChild(help);
    if (idSetting) body.appendChild(settingRow(idSetting));
    body.appendChild(keyRow("google", "Client secret"));
    var actionsSetting = byKey("daemon.google_actions");
    if (actionsSetting) body.appendChild(settingRow(actionsSetting));
    var card = el("div", "card");
    card.appendChild(el("h4", "", "Account"));
    var line = el("p", "lead", "Checking...");
    var acts = el("div", "acts");
    card.appendChild(line);
    card.appendChild(acts);
    body.appendChild(card);
    function paint(status) {
      acts.replaceChildren();
      if (status.connected) {
        line.textContent = "Connected as " + (status.email || "your Google account") + ". Restart once if the mail and calendar tools are not listed yet."
          + (status.actions && !status.can_act ? " Sending and calendar changes are switched on but not granted yet: sign in again." : "")
          + (status.actions && status.can_act ? " JARVIS may send mail and add events, asking you every time." : "");
        if (status.actions && !status.can_act) {
          var again = el("button", "yes", "Sign in again to grant it");
          again.addEventListener("click", function () {
            again.disabled = true;
            putJson("/google/connect", "POST").then(function (reply) { window.open(reply.auth_url, "_blank", "noopener"); waitForGoogle(); },
              function (error) { settingsNote(explain(error, "Could not start the sign-in."), true); again.disabled = false; });
          });
          acts.appendChild(again);
        }
        var out = el("button", "danger", "Disconnect");
        out.addEventListener("click", function () {
          out.disabled = true;
          putJson("/google/disconnect", "POST").then(function () { settingsNote("Google disconnected."); loadGoogle(); },
            function (error) { settingsNote(explain(error, "Could not disconnect."), true); out.disabled = false; });
        });
        acts.appendChild(out);
      } else if (!status.configured) {
        line.textContent = "Not set up yet: save the client ID and client secret above first.";
      } else {
        line.textContent = status.pending ? "Waiting for you to finish in the Google tab..." : "Not connected.";
        var go = el("button", "yes", status.pending ? "Open Google again" : "Sign in with Google");
        go.addEventListener("click", function () {
          go.disabled = true;
          putJson("/google/connect", "POST").then(function (reply) {
            window.open(reply.auth_url, "_blank", "noopener");
            settingsNote("Finish signing in in the Google tab. This page notices by itself.");
            waitForGoogle();
          }, function (error) { settingsNote(explain(error, "Could not start the sign-in."), true); go.disabled = false; });
        });
        acts.appendChild(go);
      }
    }
    function loadGoogle() {
      api("/google").then(paint, function () { line.textContent = "Google sign-in is not available on this daemon."; });
    }
    function waitForGoogle() {
      clearInterval(googlePoll);
      var tries = 0;
      googlePoll = setInterval(function () {
        tries += 1;
        if (tries > 90 || $("settings").hidden || sview.tab !== "google") { clearInterval(googlePoll); return; }
        api("/google").then(function (status) {
          if (status.connected) { clearInterval(googlePoll); settingsNote("Google is connected."); paint(status); }
          else paint(status);
        }, function () {});
      }, 2000);
    }
    loadGoogle();
  }
  function renderSettings() {
    var tabs = $("settings-tabs"), body = $("settings-body");
    tabs.replaceChildren();
    body.replaceChildren();
    TABS.forEach(function (tab) {
      var button = el("button", sview.tab === tab[0] ? "on" : "", tab[1]);
      button.addEventListener("click", function () { sview.tab = tab[0]; settingsNote(""); renderSettings(); });
      tabs.appendChild(button);
    });
    var current = TABS.filter(function (tab) { return tab[0] === sview.tab; })[0];
    body.appendChild(el("p", "lead", current[2]));
    if (!sview.reply) return;
    var settings = sview.reply.settings || [];
    var inGroup = function (name) { return settings.filter(function (s) { return s.group === name && !/(_api_key_ref|_secret_ref)$/.test(s.key); }); };
    if (sview.tab === "brain") {
      body.appendChild(keyRow("model", "Model key"));
      body.appendChild(keyRow("search", "Search key"));
      inGroup("brain").forEach(function (s) { body.appendChild(settingRow(s)); });
    } else if (sview.tab === "voice") {
      var voiceKey = sview.reply.keys && sview.reply.keys.voice ? sview.reply.keys.voice.state : "";
      body.appendChild(el("p", "lead", voiceKey === "set" ? "Using the ElevenLabs voice below (after a restart)." : "Using your browser's own voice. Add an ElevenLabs key for a natural one."));
      body.appendChild(keyRow("voice", "Voice key"));
      inGroup("voice").forEach(function (s) { body.appendChild(settingRow(s)); });
    } else if (sview.tab === "files") {
      inGroup("files").filter(function (s) { return !/code_sandbox/.test(s.key); }).forEach(function (s) { body.appendChild(settingRow(s, "folders")); });
      if (byKey("daemon.code_sandbox_image")) body.appendChild(codeCard());
    } else if (sview.tab === "permissions") {
      permissionsPanel(body);
    } else if (sview.tab === "google") {
      googlePanel(body);
    } else {
      inGroup("advanced").forEach(function (s) { body.appendChild(settingRow(s)); });
    }
  }
  function loadSettings() {
    api("/settings").then(function (reply) { sview.reply = reply; renderSettings(); }, failure);
    api("/tools").then(function (reply) { sview.tools = reply.tools || []; if (sview.tab === "permissions") renderSettings(); }, function () { sview.tools = []; });
  }
  function openSettings() {
    $("settings").hidden = false;
    settingsNote("");
    renderSettings();
    loadSettings();
  }
  function closeSettings() { $("settings").hidden = true; }
  $("open-settings").addEventListener("click", openSettings);
  $("settings-close").addEventListener("click", closeSettings);
  $("settings").addEventListener("click", function (event) { if (event.target === $("settings")) closeSettings(); });
  document.addEventListener("keydown", function (event) {
    if (event.key === "Escape" && !$("settings").hidden) { closeSettings(); event.stopPropagation(); }
  }, true);
  // A restart that would interrupt working tasks is refused by the daemon, which says how many; the button then offers to do it anyway.
  var restartForce = false;
  $("settings-restart").addEventListener("click", function () {
    var button = $("settings-restart");
    settingsNote("Restarting. This page reconnects by itself in a few seconds.");
    putJson("/restart" + (restartForce ? "?force=true" : ""), "POST").then(function () {
      button.className = "";
      restartForce = false;
      button.textContent = "Restart to apply";
      setTimeout(closeSettings, 1500);
    }, function (error) {
      if (error.status === 409) {
        restartForce = true;
        button.textContent = "Restart anyway";
        settingsNote((error.body || "Tasks are still working") + ". Wait for them to finish, or press Restart anyway to interrupt them.", true);
      } else {
        settingsNote(explain(error, "Could not restart from here; run `jarvis restart`."), true);
      }
    });
  });
  // ---- header and caption ---------------------------------------------------------------------------------------
  function uiState() {
    if (S.offline) return "offline";
    if (S.needYou > 0) return "waiting";
    if (S.listening) return "listening";
    if (S.speaking) return "speaking";
    // Waiting for the voice is still working: the face only "speaks" while there is sound.
    if (S.thinking || S.working > 0 || S.voiceBusy) return "working";
    return "idle";
  }
  var captions = {
    offline: "offline", idle: "standing by", working: "working", listening: "listening",
    speaking: "speaking", waiting: "needs you"
  };
  function paintHeader() {
    var mode = uiState();
    var caption = $("caption");
    var preparing = S.voiceBusy && !S.speaking && !S.thinking && S.working === 0;
    var doing = window.JarvisMission ? JarvisMission.headline() : "";
    caption.textContent = preparing ? "preparing voice" : mode === "working" && (doing || S.activity) ? "working \u00B7 " + (doing || S.activity) : captions[mode];
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

  // ---- scheduled work reports back ---------------------------------------------------------------------------------
  // A task scheduled to run while you are away is only useful if you hear what it found. When a schedule fires (its last run
  // changes) and that run finishes, the answer is put in the conversation and, with spoken answers on, said out loud. What was
  // already fired when the page opened is not announced again.
  var firedSeen = null;
  var awaitingResults = {};
  function watchSchedules(schedules) {
    var now = {};
    schedules.forEach(function (schedule) { now[schedule.schedule_id] = schedule.last_run_id || ""; });
    if (firedSeen !== null) {
      schedules.forEach(function (schedule) {
        if (schedule.last_run_id && firedSeen[schedule.schedule_id] !== schedule.last_run_id) awaitingResults[schedule.last_run_id] = schedule.objective;
      });
    }
    firedSeen = now;
    if (Object.keys(awaitingResults).length === 0) return;
    api("/runs?limit=30").then(function (reply) {
      (reply.runs || []).forEach(function (run) {
        if (!awaitingResults[run.run_id] || !run.outcome) return;
        var objective = awaitingResults[run.run_id];
        delete awaitingResults[run.run_id];
        var text = run.outcome === "succeeded" && run.answer ? run.answer : "A scheduled task did not finish: " + objective;
        notice("Scheduled: " + objective);
        show("jarvis", text);
        remember("jarvis", text);
        scroll();
        feel("pleased", 2, "nod");
        if (S.speak) speak(text);
      });
    }, function () {});
  }

  // ---- watching the work you did not start from here ---------------------------------------------------------------------
  // A scheduled task or a sub-agent has no conversation on this page, but its run still has a stream. Following it feeds the live view
  // (mission.js) so you see what it is doing, with the same links. At most three at once, none for a run waiting on an answer (its
  // stream would sit open), and each stops when the run leaves the working list.
  var observers = {};
  function tagOf(objective) {
    var text = String(objective || "");
    if (text.indexOf("[scheduled]") === 0) return "scheduled";
    if (text.indexOf("[sub-agent]") === 0) return "sub-agent";
    return "";
  }
  async function observe(run) {
    var controller = new AbortController();
    observers[run.run_id] = controller;
    var tag = tagOf(run.objective);
    try {
      var response = await fetch("/api/v1/runs/" + encodeURIComponent(run.run_id) + "/stream", { headers: { Authorization: "Bearer " + token }, cache: "no-store", signal: controller.signal });
      if (!response.ok) return;
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
          JarvisMission.event(run.run_id, tag, frame);
          if (TERMINAL[frame.event]) return;
        }
      }
    } catch (ignored) { /* the next poll starts it again if the run is still working */ }
    finally { if (observers[run.run_id] === controller) delete observers[run.run_id]; }
  }
  function observeRuns(runs) {
    if (!window.JarvisMission) return;
    var wanted = {};
    runs.filter(function (run) { return !run.outcome && run.state !== "awaiting_approval" && run.run_id !== S.runId; })
      .slice(0, 3).forEach(function (run) { wanted[run.run_id] = run; });
    Object.keys(observers).forEach(function (id) { if (!wanted[id]) { observers[id].abort(); delete observers[id]; } });
    Object.keys(wanted).forEach(function (id) { if (!observers[id]) observe(wanted[id]); });
  }
  if (window.JarvisMission) {
    // The face takes an interest in what is being done.
    JarvisMission.onStart = function (item) {
      feel(item.kind === "web" || item.kind === "memory" ? "curious" : item.kind === "agent" ? "attentive" : "focused", 1.8);
    };
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
      observeRuns(results[0].runs);
      watchSchedules(results[2].schedules);
      refreshAbilities();
      refreshProposals();
      if (Date.now() - projectsAt > 15000) loadProjects();
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
  // The face is not steered by the pointer. It has a mind of its own (head.js): it looks about, changes expression, blinks and
  // reacts to what is happening. The console only tells it what happened (JarvisHead.emote, nod, shake, mood).

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
      case "speaking": return neural.enabled && S.voiceLevel > 0.01 ? 0.3 + S.voiceLevel * 0.9 : 0.4 + 0.35 * Math.abs(Math.sin(t * 7.3) * Math.sin(t * 3.1));
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
    // While the live view is up, the face makes room for it and moves to the right.
    var room = window.JarvisMission ? Math.min(350, W * 0.38) * JarvisMission.dock(W) : 0;
    var cx = room + (W - room) / 2, cy = H / 2 - 10, R = Math.min((W - room) * 0.5, (H - 24) * 0.5) * 0.96;
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

    // the head: a real face mesh, whose mouth follows the voice and whose eyes, brows and head move on their own
    if (window.JarvisHead) {
      var mouth = 0;
      if (mode === "speaking") {
        // the real audio level when the neural voice is playing; a synthetic syllable rhythm for the browser voice
        // The neural voice drives the mouth only from the sound actually playing; the synthetic rhythm is for the browser voice.
        mouth = S.voiceKind === "neural"
          ? Math.min(1, S.voiceLevel * 1.6)
          : 0.12 + 0.88 * Math.pow(Math.abs(Math.sin(t * 9.3) * Math.sin(t * 3.7 + 0.8)), 0.7);
      }
      JarvisHead.draw(ctx, cx, cy, R * 1.62, {
        rgb: current, energy: energy, mode: mode, mouth: mouth, t: t, calm: calm
      });
    }
    // what JARVIS is doing right now: satellites, beams and the effect for each kind of work (mission.js)
    if (window.JarvisMission) JarvisMission.draw(ctx, cx, cy, R, t, current, calm, W, H, room);
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
    readVoiceLevel();
    energy += (targetEnergy(mode, t) - energy) * 0.12;
    draw(t, mode);
    requestAnimationFrame(frame);
  }
  // The face is also a button: clicking it silences JARVIS.
  canvas.addEventListener("click", interruptSpeech);
  requestAnimationFrame(frame);

  refresh();
  setInterval(refresh, 1500);
  paintHeader();
  // a greeting when the console opens: a smile and a nod, then it carries on with its own business
  setTimeout(function () { feel("pleased", 2.4, "nod"); }, 900);
})();
"use strict";
// The console's settings: the five tabs, the permissions and Google sign-in (split out of hud.js).
//
// `JarvisSettings(context)` is called once by hud.js at load. It takes the few helpers it shares with the console and wires the
// settings dialog itself; nothing else in the console calls into it. The same security rules as hud.js hold: every value from the
// daemon is inserted as text, never as markup, and a key is sent once from a password field and never read back.
window.JarvisSettings = function (context) {
  var $ = context.$, el = context.el, api = context.api, putJson = context.putJson, explain = context.explain;
  // Called when needed, not captured now: the tools list helpers are declared in hud.js after the point this is created.
  function families(tools) { return context.families(tools); }
  function familyLabel(group) { return context.familyLabel(group); }
    // ---- settings: five tabs, each saying what is set, what the default is, and why a setting is off -------------------------
    // The same checks as `jarvis config` and `jarvis keys` (one code path in the daemon). A key is typed into a password field,
    // sent once, and the field is cleared; the daemon never sends it back, only whether it is set.
    var TABS = [
      ["brain", "Brain", "Which model JARVIS thinks with, and a second one for when the first is limited or down."],
      ["search", "Web search", "Lets JARVIS search the web for you. Uses an Ollama key (a free ollama.com account makes one)."],
      ["voice", "Voice", "How JARVIS sounds. Without a voice key it speaks with your browser's own voice."],
      ["files", "Folders & code", "What JARVIS may touch. Both are off until you turn them on, so nothing is reachable by accident."],
      ["permissions", "Permissions", "For each tool: let the rules decide, always ask, run without asking, or switch it off."],
      ["google", "Google", "Sign in with your Google account so JARVIS can read your mail and calendar. Reading is on by default. Sending mail and adding events need the option below and ask you every time; drafts are saved without asking."],
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
      if ((which === "voice" || which === "search" || which === "fallback") && state === "set") {
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
        fallback: "The key of the different provider you gave above for the fallback model. Stored in a private file and never shown again; when the main model is limited, what the run is saying is sent to that provider. Check it with `jarvis keys test fallback`.",
        search: "An Ollama API key (a free ollama.com account makes one) that turns on web search. Stored in a private file and never shown again; what JARVIS searches for is sent to ollama.com. Check it with `jarvis keys test search`."
      };
      row.appendChild(el("span", "help", helps[which] || ""));
      return row;
    }
    // Two settings that only make sense together are saved together.
    // ---- the model providers: pick one, load its models, test it, save ---------------------------------------------------------
    // The presets come from the daemon (with the settings list) and only fill in an address. Every request to a provider is made by the
    // daemon, so a stored key never reaches this page; the key typed here is sent once, with the request that needs it.
    function normUrl(url) { return String(url || "").trim().replace(/\/+$/, "").toLowerCase(); }
    function providerFor(url) {
      var all = (sview.reply && sview.reply.providers) || [];
      var found = all.filter(function (p) { return p.url && normUrl(p.url) === normUrl(url); })[0];
      return found || all.filter(function (p) { return p.id === "custom"; })[0];
    }
    function providerRow(label, control, acts, help) {
      var row = el("div", "set-row");
      row.appendChild(el("span", "name", label));
      row.appendChild(control);
      row.appendChild(acts || el("span"));
      if (help) row.appendChild(help);
      return row;
    }
    // A filterable dropdown for the model box. The browser's own <datalist> only offers what matches the text already in the box (so a
    // saved model name hides every other choice) and cannot be styled, so this draws its own: it opens on click or with the arrow,
    // lists every model, narrows as you type, and takes the keyboard. Typing any name that is not listed is still allowed.
    var openPicker = null;
    function modelPicker(input) {
      var wrap = el("div", "combo");
      var toggle = el("button", "combo-toggle", "\u25BE");
      toggle.type = "button";
      toggle.title = "Show the models";
      var menu = el("ul", "combo-list");
      menu.hidden = true;
      wrap.appendChild(input);
      wrap.appendChild(toggle);
      wrap.appendChild(menu);
      var models = [], active = -1, typed = false;
      function visible() {
        var needle = typed ? input.value.trim().toLowerCase() : "";
        return models.filter(function (m) { return !needle || m.id.toLowerCase().indexOf(needle) >= 0; }).slice(0, 300);
      }
      function draw() {
        var shown = visible();
        menu.replaceChildren();
        if (!models.length) {
          menu.appendChild(el("li", "note", "Press Load models to see what this provider offers, or type a model name."));
        } else if (!shown.length) {
          menu.appendChild(el("li", "note", "No model matches. Keep typing to use that name as it is."));
        }
        shown.forEach(function (m, index) {
          var item = el("li", (m.likely_chat ? "" : "dim ") + (m.id === input.value ? "current " : "") + (index === active ? "active" : ""), m.id + (m.likely_chat ? "" : "   (probably not a chat model)"));
          item.addEventListener("mousedown", function (event) { event.preventDefault(); choose(m.id); });
          menu.appendChild(item);
          if (index === active) item.scrollIntoView({ block: "nearest" });
        });
      }
      function open() {
        if (openPicker && openPicker !== api2) openPicker.close();
        openPicker = api2;
        menu.hidden = false;
        draw();
      }
      function close() {
        menu.hidden = true;
        active = -1;
        typed = false;
        if (openPicker === api2) openPicker = null;
      }
      function choose(id) { input.value = id; close(); input.dispatchEvent(new Event("change")); }
      toggle.addEventListener("mousedown", function (event) { event.preventDefault(); if (menu.hidden) { input.focus(); open(); } else close(); });
      input.addEventListener("focus", function () { typed = false; active = -1; open(); });
      input.addEventListener("click", function () { if (menu.hidden) open(); });
      input.addEventListener("input", function () { typed = true; active = -1; if (menu.hidden) open(); else draw(); });
      input.addEventListener("keydown", function (event) {
        var shown = visible();
        if (event.key === "ArrowDown" || event.key === "ArrowUp") {
          event.preventDefault();
          if (menu.hidden) open();
          if (!shown.length) return;
          active = (active + (event.key === "ArrowDown" ? 1 : -1) + shown.length) % shown.length;
          draw();
        } else if (event.key === "Enter" && !menu.hidden && active >= 0 && shown[active]) {
          event.preventDefault();
          choose(shown[active].id);
        } else if (event.key === "Tab") {
          close();
        }
      });
      document.addEventListener("mousedown", function (event) { if (!menu.hidden && !wrap.contains(event.target)) close(); });
      var api2 = {
        element: wrap,
        close: close,
        // Sets the models on offer; `show` opens the list so the person sees it arrive.
        setModels: function (list, show) {
          models = list || [];
          if (show && models.length) { input.focus(); open(); } else if (!menu.hidden) draw();
        }
      };
      return api2;
    }    function providerCard(slot) {
      var fallback = slot === "fallback";
      var urlKey = fallback ? "daemon.executor_fallback_base_url" : "daemon.executor_base_url";
      var modelKey = fallback ? "daemon.executor_fallback_model_name" : "daemon.executor_model_name";
      var which = fallback ? "fallback" : "model";
      var urlSetting = byKey(urlKey), modelSetting = byKey(modelKey);
      var keyState = sview.reply.keys && sview.reply.keys[which] ? sview.reply.keys[which].state : "unknown";
      var savedUrl = urlSetting && urlSetting.value ? String(urlSetting.value) : "";
      var presets = (sview.reply.providers || []).slice();
      if (fallback) presets.unshift({ id: "same", name: "Same provider as the main model", url: "", needs_key: false,
        help: "The fallback model is asked at the main provider: a different model there, for when this one is limited." });
      var card = el("div", "card");
      var title = el("h4", "", fallback ? "Fallback model" : "Main model");
      var isSet = modelSetting && modelSetting.value;
      title.appendChild(el("span", "badge " + (isSet ? "on" : "off"), isSet ? "set" : "off"));
      card.appendChild(title);
      card.appendChild(el("p", "lead", fallback
        ? "Asked instead when the main model is rate limited, overloaded, down or out of credit. A different provider keeps JARVIS working through an outage; what a run is saying is then sent to that provider."
        : "The model JARVIS thinks with. Pick a provider, paste its key, load the list of models, and test before saving."));

      var select = el("select");
      presets.forEach(function (p) { var option = el("option", "", p.name); option.value = p.id; select.appendChild(option); });
      var address = el("input");
      address.type = "text";
      var keyBox = el("input");
      keyBox.type = "password";
      keyBox.autocomplete = "off";
      var model = el("input");
      model.type = "text";
      model.value = modelSetting && modelSetting.value ? String(modelSetting.value) : "";
      model.placeholder = "choose from the list, or type a model name";
      var picker = modelPicker(model);
      var providerHelp = el("span", "help");
      var status = el("div", "help");

      var current = savedUrl ? providerFor(savedUrl) : (fallback ? presets[0] : providerFor(""));
      select.value = current.id;
      address.value = savedUrl;
      function chosen() { return presets.filter(function (p) { return p.id === select.value; })[0]; }
      function isSame() { return select.value === "same"; }
      function addressChanged() { return normUrl(address.value) !== normUrl(savedUrl); }
      function refreshFields() {
        var p = chosen();
        address.disabled = isSame();
        address.placeholder = p.id === "custom" ? "the provider's address, ending in /v1" : "";
        keyBox.disabled = isSame() || !p.needs_key;
        keyBox.placeholder = isSame() ? "uses the main key" : !p.needs_key ? "no key needed" :
          (keyState === "set" && !addressChanged() ? "set; paste a new key to replace it" : "paste the key for this provider");
        providerHelp.textContent = p.help || "";
      }
      function say(text, bad) { status.textContent = text; status.className = "help " + (bad ? "bad" : ""); }
      select.addEventListener("change", function () {
        var p = chosen();
        if (p.id !== "custom") address.value = p.url;
        if (p.id === "custom" && !addressChanged()) address.value = "";
        picker.setModels([], false);
        say("");
        refreshFields();
      });
      address.addEventListener("input", function () {
        var p = providerFor(address.value);
        if (!isSame() && p) select.value = p.id;
        refreshFields();
      });
      refreshFields();

      function ask(test) {
        var body = { slot: slot };
        if (!isSame()) body.base_url = address.value.trim();
        if (keyBox.value.trim()) body.key = keyBox.value.trim();
        if (test) {
          if (!model.value.trim()) { say("Choose or type a model to test.", true); return; }
          body.test_model = model.value.trim();
        }
        say(test ? "Asking the model..." : "Asking the provider for its models...");
        putJson("/settings/models", "POST", body).then(function (reply) {
          picker.setModels(reply.models || [], true);
          var parts = [];
          if ((reply.models || []).length) parts.push((reply.models.length) + " models found: choose one from the list in the model box.");
          else if (reply.list_problem) parts.push("No list: " + reply.list_problem + " You can still type a model name.");
          if (reply.test) parts.push((reply.test.ok ? "Test passed in " + (reply.test.millis / 1000).toFixed(1) + " s. " : "Test failed: ") + reply.test.message);
          say(parts.join(" "), (reply.test && !reply.test.ok) || (!reply.models.length && !!reply.list_problem && !reply.test));
        }, function (error) { say(explain(error, "Could not ask the provider."), true); });
      }
      var load = el("button", "", "Load models");
      load.addEventListener("click", function () { ask(false); });
      var test = el("button", "", "Test");
      test.addEventListener("click", function () { ask(true); });
      var save = el("button", "", "Save");
      save.addEventListener("click", function () {
        var p = chosen(), typed = keyBox.value.trim(), name = model.value.trim();
        if (!name) { say("Choose or type a model first.", true); return; }
        var changes = [{ key: modelKey, values: [name] }];
        if (isSame()) { if (savedUrl) changes.push({ key: urlKey, values: null }); }
        else {
          if (!address.value.trim()) { say("Give the provider's address first.", true); return; }
          changes.push({ key: urlKey, values: [address.value.trim()] });
          // The key on file belongs to the address it was saved for: never leave it standing for a different provider.
          if (p.needs_key && !typed && (addressChanged() || keyState !== "set")) { say("Paste the key for " + p.name + " first.", true); return; }
        }
        var keyStep = function () {
          if (typed && !isSame() && p.needs_key) return putJson("/settings/keys/" + which, "PUT", { key: typed });
          if (!isSame() && !p.needs_key && (addressChanged() || keyState !== "set")) return putJson("/settings/keys/" + which, "PUT", { key: "ollama" });
          if (isSame() && fallback && keyState === "set") return putJson("/settings/keys/fallback", "DELETE");
          return Promise.resolve();
        };
        putJson("/settings", "PUT", { changes: changes }).then(keyStep).then(function () {
          keyBox.value = "";
          saved(fallback ? "Fallback model" : "Main model");
        }, function (error) { say(explain(error, "That was refused."), true); });
      });
      var acts = el("div", "acts");
      acts.appendChild(load); acts.appendChild(test); acts.appendChild(save);

      card.appendChild(providerRow("Provider", select, null, providerHelp));
      card.appendChild(providerRow("Address", address));
      card.appendChild(providerRow("Key" + (keyState === "set" ? " (set)" : ""), keyBox));
      card.appendChild(providerRow("Model", picker.element, acts, status));
      return card;
    }
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
        body.appendChild(providerCard("main"));
        body.appendChild(providerCard("fallback"));
        var effort = byKey("daemon.executor_reasoning_effort");
        if (effort) body.appendChild(settingRow(effort, "Thinking effort"));
      } else if (sview.tab === "search") {
        body.appendChild(keyRow("search", "Search key"));
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
      if (event.key === "Escape" && !$("settings").hidden) {
        // Escape closes an open model list first, and the dialog only when none is open.
        if (openPicker) openPicker.close(); else closeSettings();
        event.stopPropagation();
      }
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
};

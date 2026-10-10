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
        body.appendChild(keyRow("fallback", "Fallback key"));
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
};

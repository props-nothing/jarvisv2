# Quick start

For someone who wants to use JARVIS, not build it. Contributors: see [development/getting-started.md](../development/getting-started.md).
The target experience, and what is still missing from it, is in [product/onboarding.md](../product/onboarding.md).

## What you need

- A model. The easiest is [Ollama](https://ollama.com) running on this machine (local models, or Ollama Cloud models through it).
  Any OpenAI-compatible server works too.
- Optionally Docker, if you want the assistant to run code in a throwaway container.
- One program, `jarvis` (the assistant it runs in the background is the same file, started as `jarvis daemon`). Either unpack a release archive (CI builds one per platform as
  a workflow artifact, or a draft release for a tagged build; there is no public download until a licence is chosen), or build it with a Rust toolchain:
  `cargo build --release -p jarvis-cli`, then take it from `target/release/`.

## Installing it

`jarvis install` copies the program to your own folder (`%LOCALAPPDATA%\Programs\JARVIS` on Windows, `~/.local/share/jarvis` elsewhere), puts it on your PATH so `jarvis` works in any new terminal,
and with `--service` starts it at login. It needs no administrator rights and never touches your settings or memory; `jarvis uninstall` takes it out again.

## First run

```text
jarvis
```

That is the whole thing. The first time, it asks which model to use (it lists what your Ollama has) and which folder, if any,
the assistant may read and write; then it starts in the background and opens the console in your browser, already signed in.
Later runs skip the questions, make sure it is running, and open the console. `jarvis --no-open` starts it without opening a
window.

Setting up without questions (scripts, other providers):

```text
jarvis init --base-url https://api.example.com/v1 --model NAME --api-key-file C:/path/to/key.txt --workspace C:/Users/me/notes
jarvis start
```

`--workspace` may be repeated. The assistant can read, list and create files in those folders, search their text, and run commands there (such as a build or the tests) after you approve each one; changing an existing file is
asked about first. Nothing outside them is reachable.

## Keys

| Key | What it is for | How to give it today |
| --- | --- | --- |
| Model key | A hosted model provider (not needed for local Ollama) | `jarvis keys set model --from-env NAME` (or `--file PATH`, or pipe it in), or `--api-key-file PATH` at `init`. |
| Voice key | A natural voice (ElevenLabs) | `jarvis keys set voice --from-env ELEVENLABS_API_KEY`, then `jarvis restart`. Without it the console speaks with your browser's own voice. |

```text
jarvis keys status               # which keys are set (never their values)
jarvis keys set voice --from-env ELEVENLABS_API_KEY
jarvis keys test model           # asks the model server to accept the key
jarvis keys test voice           # asks the running daemon to make a sentence of audio
jarvis keys remove voice
```

Keys are written to a private file and the configuration holds only its path; they are never printed, logged or sent to the
browser, and never taken as a command-line value (it would stay in your shell history). With no flag and a terminal, `keys set`
asks you to paste it (visible as you type; a hidden prompt is `P9-013`).

## Changing settings

```text
jarvis config set code_sandbox node:22-alpine node -e    # the code container and its command are set together
jarvis config show               # every setting you can change, and whether key files are present
jarvis config get executor_model_name
jarvis config set executor_model_name deepseek-v4-flash:cloud
jarvis config set tool_workspace_roots C:/Users/me/notes C:/Users/me/projects    # replaces the list
jarvis config set policy.trust jarvis.files.edit        # tools allowed to run without asking
jarvis config unset speech_model
jarvis restart                   # apply
```

Each change is checked by the same rules the daemon starts with and refused, leaving the file untouched, if it would not
start (for example a voice id without a voice key). Changing one setting changes only that one: your folders, trust list and
voice stay as they were. The same settings, and the keys, are in the console: press **Settings** in the header. It has five tabs (Brain, Voice, Folders & code,
Permissions, Advanced); each setting shows its default or why it is off. Keys are password fields and are never shown again;
**Restart to apply** restarts JARVIS with what you saved. [settings.md](settings.md) explains every setting and why a few are off by
default.

## Using it

- **The console** (opened for you; `jarvis hud` opens it again) is a face with a conversation beside it. The face is the page: it has a
  mind of its own (it looks about, changes expression, blinks, nods off after a long quiet) and reacts to what is happening: it
  listens, thinks, speaks with the voice, looks worried when something fails and pleased when you approve. Type or talk in the panel on
  the right; **New chat** (or `N`, or say "new chat") starts a fresh conversation with none of the old context, and **History** reopens an
  earlier one where you left off. Conversations are kept in your browser, so closing the page does not lose them. Anything that needs a yes or no appears at the bottom left of the face, with Approve and Deny. **Ops** (or press `O`) opens
  the operations page: what is running, scheduled and recent, and which tools it can use and which it asks about first. **Stop everything**
  is always in the header.
- **Voice:** the microphone button (or `M`) to talk; turn on **Speak answers**; enable the **wake word** and say "Jarvis, ...".
  "Jarvis, stop" cancels everything. Voice recognition is your browser's (Chrome or Edge).
- **The terminal:** `jarvis chat`, `jarvis ask "..."`, `jarvis watch`, `jarvis approvals`, `jarvis cancel --all`, `jarvis runs`,
  `jarvis schedule` (or just ask: "remind me tomorrow at nine" schedules it after you approve), `jarvis memory`, `jarvis tools list`. Every command has `--json` where it prints data.
- **Watching it work:** while JARVIS works, the left of the face lists each action as it happens (searching, reading a page, running a command, a sub-agent) with the pages it found as links you can open, and each action also appears as a node round
  the face and stays there while the task goes on: point at one to see what it was, press it to jump to its step. After a minute and a half of quiet the list folds to one line. Scheduled tasks and sub-agents appear there as well. Finished answers list their sources. Nothing is sent anywhere for this; it is what the run already reports.
- **Long-running work:** give it a project (`jarvis project add`, or Ops → Projects): a goal, your standing guidance and a journal that every run of it is told, so a recurring task
  continues instead of restarting. See `docs/user/projects.md`.
- **While you are away:** `jarvis digest` says what it did (runs, outcomes, what waits for you, tokens, what each project decided). A task a restart cut off is continued once, and a model call the provider rate limits is waited out (up to about five minutes) rather than failing the task. Set a `push_topic` (Settings → Advanced, then `jarvis push test`)
  to hear on your phone, with no content, when an approval is waiting or a scheduled task finished.
- **Long tasks.** While it works you see a live line (the step it is on, what it is doing, how long, and "thinking (~11k tokens)" during a
  silent think), and the model is asked to tell you what it is about to do and to say when each stage is done. If a task stops, it says why
  and offers **Continue**. A task may take hundreds of steps; **Stop everything** is always there.
- **What it can do on a fresh install:** read web pages, hand work to helper agents, and offer to remember things. Files need a folder
  you grant (Settings, Folders & code) and running code needs Docker. More tools (search what it remembers, the clock, scheduling from
  conversation, mail and calendar) are on the roadmap (`P9-024`).
- **Reading documents.** With a folder granted, `jarvis.files.read_document` reads PDF, Word (`.docx`), spreadsheet (`.xlsx`) and slide (`.pptx`) files in it, a few thousand characters at a time, and treats their text as data to read, never as instructions. It also reads a Word file's headers, footers, footnotes and endnotes. It also reads a photo or screenshot (`.png`, `.jpg`, `.bmp`) and a scanned PDF as text, if you install the free **tesseract** program (Windows installer from github.com/UB-Mannheim/tesseract/wiki; macOS `brew install tesseract`; Linux the `tesseract-ocr` package, plus the language data for anything but English); the answer is marked as read from a picture, so expect small mistakes. It cannot read password-protected files or the old `.doc`/`.xls` formats. Together with `jarvis.gmail.save_attachment`, a PDF that arrives by mail can be saved and read.
- **Long pages and PDFs on the web.** `jarvis.web.fetch` reads a long page a part at a time (it is told where the next part starts) and can read a PDF at a link, up to 5 MB. What it reads is data, never instructions, and private addresses are still refused.
- **Organising files.** `jarvis.files.move` moves or renames a file or folder inside a granted folder (missing folders are created). It never replaces anything: if the new name exists it refuses. It runs without asking, like creating a file.
- **Remembering.** Say "remember that I like short answers". JARVIS offers it in **Waiting for you** as a "remember?" card; **Keep**
  makes it shape later answers, **Dismiss** forgets it. It asks because a web page it read could otherwise tell it what to remember.
- **Approvals are a yes or no.** Anything it asks about appears with Approve and Deny; you can also say "yes" or "no", or use
  `jarvis approvals approve|deny`.

## `jarvis` is not found?

A built or unpacked JARVIS is a folder, and nothing puts that folder on your PATH, so a terminal says `jarvis: command not found`
(or "not recognized"). Run it once by its path: `jarvis path install` from inside the folder (for example
`.\target\release\jarvis.exe path install`). On Windows it adds the folder to your user PATH; on Linux and macOS it links `jarvis`
into `~/.local/bin`. Open a new terminal and `jarvis` works anywhere. `jarvis path status` says whether it does, and
`jarvis path uninstall` undoes it. `jarvis start` mentions this when it notices.

## Running it all the time, and on other machines

- `jarvis service install` makes JARVIS start when you log in (a systemd user unit on Linux, a launchd agent on macOS, a login entry
  on Windows) and `jarvis service uninstall` undoes it.
- On a server with no screen, `jarvis hud --print-url` prints the console address (with your credential) to open through an SSH
  tunnel. JARVIS only listens on `127.0.0.1`.
- [platforms.md](platforms.md) says what is verified on Linux and macOS and what is not, and has the headless-server walkthrough.

## Stopping and troubleshooting

- `jarvis stop` ends the background program gracefully; `jarvis restart` stops it (if it is running) and starts it again, for example
  after you change a setting. Both wait until it has really finished.
- `jarvis doctor --live` checks the things that actually go wrong, each with its fix: the model server unreachable or rejecting the key,
  a granted folder gone, Docker not running for the code tool, JARVIS not started. `jarvis doctor` alone diagnoses the installation; `jarvis logs --lines 50` shows what the daemon said; `jarvis status` says whether it
  is running.
- "No credential": open the console with `jarvis hud`, not by typing the address.
- A portable, throwaway profile: add `--root DIR` (an existing folder) to any command.
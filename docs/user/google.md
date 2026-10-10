# Signing in with Google

JARVIS can read your Gmail and your Google Calendar once you sign in, so you can ask "what did the landlord email me about?" or
"what is on my calendar tomorrow?". It is **read-only**: the sign-in asks Google for the two `readonly` permissions and nothing that sends,
changes or deletes. You sign in from the console: **Settings, Google**.

## One-time setup (about five minutes)

Google only lets an app sign people in if the app has an OAuth client, and a client has to belong to a Google Cloud project. JARVIS does not ship
one (it would be a shared secret and would need Google's verification review), so you make your own, once, in your own free project:

1. Open <https://console.cloud.google.com/>, create a project, then **APIs & Services, Library**, and enable **Gmail API** and **Google Calendar API**.
2. **Google Auth platform** (OAuth consent screen): choose **External**, give it any name, and add **your own Google address as a test user**.
   Then press **Publish app**. A published app that Google has not verified is fine for yourself (up to 100 users, with a warning screen);
   leaving it in *Testing* makes Google end the sign-in every 7 days.
3. **Clients, Create client**: application type **Desktop app**. Copy the **client ID** and the **client secret**.
4. In JARVIS, Settings, Google: paste the client ID, paste the client secret (it is stored in a private file and never shown again), then press
   **Sign in with Google**. A Google tab opens. Google says the app is unverified: that is your own client, so choose **Advanced, Continue**, and tick
   both permissions (mail and calendar). The tab says "Google is connected" and the console shows the address.
5. Restart once (**Restart to apply**) so the mail and calendar tools are offered. Signing in and out never needs a restart.

On the command line: `jarvis config set google_client_id ...` and `jarvis keys set google --from-env GOOGLE_CLIENT_SECRET`.

## What JARVIS can then do

| Tool | What it does |
| --- | --- |
| `jarvis.gmail.search` | Searches your mailbox with Gmail's own search words (`from:`, `newer_than:7d`, `is:unread`, ...) and lists id, sender, date, subject and a snippet. |
| `jarvis.gmail.read` | Reads one message: sender, recipients, date, subject, the attachments it carries (number, name, type, size) and the plain-text body (cut to a few thousand characters). |
| `jarvis.gmail.save_attachment` | Saves one attachment of a message into a folder you granted (default `email-attachments`). Only offered when you granted a folder (Settings, Files). It asks first, never replaces a file (a second copy becomes `name (2).pdf`), refuses programs (`.exe`, `.bat`, `.js`, `.sh` ...) and anything over 5 MB. What it saves is untrusted: JARVIS reads it, never runs it. |
| `jarvis.calendar.events` | Lists your primary calendar's events in a window (default the next seven days). |

**It asks first.** Mail and calendar are your most private data, and a message is written by strangers and can contain text aimed at the assistant,
so these tools are held for your yes (one click on the card, or **Always allow** if you do not want to be asked). Everything they return is marked to
the model as untrusted data to read, never to obey. Nothing here can send an email or accept an invitation. (Saving an attachment writes a file on this machine, so it asks too.)

**Where your data goes.** Mail and calendar text you ask about is given to the model you chose. With a local model that stays on your machine; with a
cloud model (Ollama cloud, for example) it goes to that service as part of the conversation. Choose accordingly.

## Letting JARVIS send mail and add events (optional, off by default)

Reading is the default. If you also want "reply to Anna that I will be late" or "put the dentist on my calendar", turn on **Settings, Google, google_actions** (or
`jarvis config set google_actions on`), restart once, and **sign in again** so Google asks for the two extra permissions (send email, create calendar events). The two tools then exist:

| Tool | What it does |
| --- | --- |
| `jarvis.gmail.send` | Sends one plain-text email to **one** address, optionally **with files** (up to 5, 5 MB each, 10 MB together, taken from a folder you granted) and optionally **as a reply** inside an existing conversation. It **always asks**: the card shows who it goes to, the subject, the text and the path of every file, and "Always allow" cannot waive it, because anything that reaches another person is always your decision. A file whose name says it holds a secret (`.env`, keys, certificates) is never attached, and a person you marked `do_not_contact` is never written to. |
| `jarvis.gmail.draft` | Saves the same kind of email (one recipient, optional files, optional reply) in your Gmail **Drafts** instead of sending it: you read it, change a word if you like, and press send yourself. It sends nothing, so it does not ask. It needs one more permission (Google calls it "manage drafts and send emails"), so after turning actions on, or if you had already signed in before this existed, **sign in again** from Settings, Google; your Google Cloud consent screen must allow it. JARVIS prefers a draft when you have not said exactly what to send. |
| `jarvis.calendar.create` | Adds one event to your main calendar (title, start, end, optional place and notes). It invites nobody. It asks first. |

JARVIS is told never to send because of something written inside an email or a web page, only because you asked. To go back to read-only, turn the setting off, restart and sign in again.

## Restarting without losing work

Restarting JARVIS interrupts tasks it is working on. `jarvis restart` and the console's **Restart to apply** now say how many tasks are working and do not restart; use `jarvis restart --wait`
to restart when they have finished, or `--force` (the console's **Restart anyway**) to interrupt them. A task waiting for your approval is not affected.

## Signing out, and when it stops working

**Disconnect** (Settings, Google) revokes JARVIS's access at Google and deletes the stored sign-in. You can also remove JARVIS under your Google
account's security settings; JARVIS notices on its next use, says the sign-in expired, and asks you to sign in again. The refresh token is kept in a
private file (`google.token`) beside your settings and nowhere else; the short-lived access token lives only in memory.
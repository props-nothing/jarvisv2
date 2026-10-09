# Projects

A **project** is long-running work you want JARVIS to keep doing: a goal, your standing guidance, where its files live, and a journal of what has happened. Every run that belongs to a
project is told all of that, so you write it once instead of pasting it into each request, and tomorrow's run knows what today's run decided.

A project changes what JARVIS is **told**, never what it may **do**. Approvals, permission postures and the folders you granted are exactly what they were.

## Make one

In the console: **Ops → Projects → New project**. Or in a terminal:

```text
jarvis project add Prospecting --goal "Book five demos with Dutch logistics firms" --folder sales --guidance-file brief.txt
```

* **Goal**: what done looks like, in a sentence or two.
* **Guidance**: how you want it done. Language, tone, who it may contact, what to avoid, how often to bother you. Up to 12,000 characters.
* **Folder**: where its files live, relative to a folder you granted in Settings → Files. It cannot point outside that folder.

## Work in it

| Where | How |
| --- | --- |
| Console | Pick the project in the selector next to "Conversation", or press **Chat** on a project. A conversation stays in its project. |
| Terminal | `jarvis ask --project Prospecting find three new leads`, or `jarvis chat --project Prospecting` |
| On a schedule | `jarvis schedule add check replies and follow up --every 6h --project Prospecting` |
| By JARVIS | A task it schedules from inside a project joins it, and a sub-agent it starts there gets the project's brief. |

## The journal

JARVIS is asked to record a line when it finishes a stage, decides something, is blocked, or knows what comes next (`jarvis.project.note`; it needs no approval, because it only writes one
short line in your own journal). The newest entries are shown to every run. You can add your own: `jarvis project note Prospecting skip anyone at the competitor`, or in the console's
project window. Journal entries are shown to the model as data to read, never as instructions, so a note written after reading a web page cannot take over a later run.

## Pause, finish, delete

`jarvis project pause|resume|done NAME`. While a project is paused or done its scheduled tasks stop firing (you can still chat in it). `jarvis project remove NAME` deletes the project and its
journal; its conversations and schedules stay.

## Moving existing work into a project

If you already run something by hand (a long objective on a schedule, a worklog file the model keeps), create the project, move the standing instructions out of the objective into the guidance,
and recreate the schedule with `--project`. Keep the objective to what is different each time ("check for replies and follow up").
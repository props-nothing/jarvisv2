# ADR-0014: A Conversation Is Runs Sharing A Session, And The Session Is A Trust Boundary

- Status: Accepted
- Date: 2026-09-23

## Context

`P2-008` named `jarvis ask` and `jarvis chat`; only `ask` shipped, and `chat` was deferred because
nothing answered a question yet. `P2-009` supplied the executor and the transcript writer, so the loop
became buildable — and building it raised two questions that are durable rather than coding detail.

**1. What is a conversation, in the data model?** Three shapes were available:

- a long-lived run that receives more input over time;
- a session per turn, with the client holding the history;
- many runs in one session, with the daemon holding the history.

The first contradicts the state machine: a run settles once, and a settled run provably emits no
further events — `P2-007b` enforces exactly that, and `P2-010` depends on it. The second makes the
conversation a client artefact, so a second client, a restarted CLI, or a voice session would each
have a different memory of the same conversation, and a conversation the server cannot see is one it
cannot enforce policy over or audit.

**2. Who may continue a conversation?** A continuation names a session, and a session identifier is a
UUID that a client supplies. If the daemon accepts one and appends a run to it, then any caller that
can guess or observe an identifier can attach work to another workspace's conversation — and the
transcript, the model context, and the resulting answer would all be built from a history it was never
granted. The identifier is not a capability, and treating it as one is the same mistake ADR-0012
refuses for the credential and `identity-and-workspaces.md` refuses for a client-supplied workspace.

## Decision

### A conversation is a sequence of runs that share a session

`jarvis chat` starts a **new run per turn** in the session the daemon issued. Nothing about the run
model changes: each turn is a run that walks the documented state machine, settles once, and emits a
closed stream. The daemon, not the client, replays the conversation into the model call.

`StartRunRequest.session_id` is optional, and its presence means "continue this conversation". Absent
means a new one, which is the ordinary single-turn case — requiring a session identifier would make
every caller create a session before it could ask a question.

### The session's workspace and user are part of the guard, not a check afterwards

Attaching a run to a session is **one statement** whose predicate includes `workspace_id`, `user_id`,
and `status = 'active'`, for the reason the run-event repository records: a separate read takes a
snapshot another writer can invalidate. The three failure causes are resolved from a read on the failure
path:

- **no such session** → `SessionNotFound`;
- **a session of another workspace or user** → `SessionNotFound`, deliberately. Reporting "not yours"
  would confirm that somebody else's conversation exists, which is the disclosure the check exists to
  prevent;
- **an archived session** → `SessionNotWritable`, because this one *is* actionable: the conversation
  exists and is closed to new work, so the caller starts another rather than retrying.

The client never invents a session identifier. `chat` prints the one the daemon returned and sends that
back, so a daemon that started a different session than requested cannot leave the client addressing
one that does not exist.

### The window is chosen from the end, and the assembler decides what fits

History is read with `read_recent_messages`, which selects the newest N and returns them
oldest-first. The alternative — read the first N and reverse — returns the *oldest* turns and discards
the ones a follow-up depends on.

Turns are offered to `assemble_context` rather than added to the request directly, so the manifest is
the record of what the model was given and a turn the budget cannot hold is **excluded with a recorded
reason**. Conversation is `Optional` priority rather than `Required`: `RequiredExceedsBudget` is an
error by design, and "your conversation is too long" is not a reason to refuse a question.

### Order comes from the conversation; membership comes from the manifest

These are two questions, and the first implementation conflated them — running the test found it. The
assembler orders by **budget tier**, so the assembled list was policy, current question, earlier
question, earlier answer: correct for budgeting and wrong for a conversation, because a model reading
the earlier exchange *after* the current question reads it as a continuation of the prompt rather than
as context for it. So the manifest decides **what may be sent** and the replay decides **in what
order** — policy, then replayed turns oldest-first, then the question.

## Consequences

- A conversation survives a client restart, a second client, and a future voice channel, because the
  transcript belongs to the daemon. That is what makes policy over the conversation possible at all.
- Cross-workspace continuation is refused by the write, and the refusal does not disclose the other
  session's existence. A test seeds a *real* second workspace and user, so `LocalIdentityMissing`
  cannot be the cause and the session predicate is the only thing being tested; removing the predicate
  makes that test fail with a run written into the foreign session.
- The transcript is now read by a second consumer (the executor) as well as written. The stored
  message is the single source of history: the model sees what a client replaying the same session
  would see, with no second copy that could drift.
- Tool results are **skipped** rather than replayed, because a tool message must carry the call it
  answers and nothing writes one yet. Fabricating a call identifier would be a claim the transcript
  does not support.
- A conversation is bounded to the newest turns. Longer conversations need summarisation, which is
  `P4-007`; the manifest's exclusion reason is what makes the truncation visible until then.
- `jarvis chat` cannot resume by identifier. Continuing a stored conversation from a new process needs
  the inspect and export surface of `P4-008`, and claiming otherwise would misrepresent what a user
  can do with the identifier the CLI prints.

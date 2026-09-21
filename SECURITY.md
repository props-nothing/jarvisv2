# Security Policy

JARVIS is not yet a released product. Until the Phase 3 policy and approval gates are implemented and audited, do not expose it to the public internet or entrust it with production credentials, destructive tools, financial authority, or sensitive communications.

## Reporting A Vulnerability

Use the repository host's private security-advisory mechanism. Do not open a public issue containing exploit details, secrets, personal data, or unredacted logs. If no private channel exists yet, withhold details until the maintainer configures one.

Include:

- affected revision and operating system
- required configuration and trust assumptions
- minimal reproduction
- expected and observed behavior
- impact and whether credentials or personal data may be exposed
- suggested mitigation, if known

## Trust Model

Treat all of the following as untrusted:

- model output and model-supplied tool arguments
- retrieved documents, email, web pages, and tool output
- prompts or instructions embedded in external content
- MCP servers, plugins, external runtimes, and browser pages
- webhook bodies before signature and replay verification
- caller identity and client-supplied workspace identifiers
- paths, URLs, redirects, filenames, archives, and media metadata

Only deterministic JARVIS code may grant authority. A successful model response is never authorization.

## Required Security Properties

- Bind locally by default. Remote access requires explicit configuration, authentication, authorization, TLS, and an exposure check.
- Separate user, workspace, client, runtime, connector-account, and tool-call identities.
- Enforce least privilege at the final execution boundary, not only during tool discovery.
- Keep secrets out of prompts, model-visible tool results, URLs where avoidable, logs, traces, crash reports, and database rows that only need a `SecretRef`.
- Use OS keychains for local secrets and pluggable secret managers for server mode.
- Require user-interface or authenticated-channel evidence for approvals. The model cannot approve its own action.
- Fail closed when policy, identity, approval UI, secret resolution, or audit persistence is unavailable.
- Sandbox code execution and constrain filesystem/network/process resources.
- Validate webhook signatures over the raw request body, enforce timestamp/replay windows, persist dedupe keys, and acknowledge only after durable admission.
- Store immutable audit decision receipts with actor, policy version, request hash, result, and correlation IDs.
- Make memory provenance, correction, retention, export, and deletion user-visible.

## High-Risk Effects

The following require explicit policy and normally fresh approval:

- external communication sent as the user
- destructive or irreversible changes
- credential, identity, or permission changes
- code execution outside a sandbox
- financial transactions or purchases
- deployment or production-data mutation
- mass communication
- initiating a phone call, except for a narrowly pre-authorized workflow

Approval does not remove validation, scope, idempotency, or audit requirements.

## Voice And Telephony

- A phone number or caller-provided identifier is evidence, not identity proof.
- Bind calls to short-lived scoped sessions and restrict voice clients to the minimum tool set.
- Make recording, transcription, disclosure, consent, and retention configurable by jurisdiction and workspace policy.
- Verify provider callbacks; never accept a transcript or call status solely because it names a known conversation ID.
- Keep emergency, medical, financial, and safety-critical actions outside autonomous voice authority by default.

## Prototype Warning

The [example](example/readme.md) stores configuration and API keys in a JSON file and executes many actions inside one Python process. It is a reference prototype, not a secure deployment baseline. The root [.gitignore](.gitignore) excludes its known local secret and memory paths, but ignored files that were previously committed would still require history cleanup and credential rotation.

See [the detailed threat model](docs/architecture/security.md) for assets, boundaries, abuse cases, and required controls.
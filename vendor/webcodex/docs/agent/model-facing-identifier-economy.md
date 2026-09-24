# Model-facing identifier economy

Working inventory, based on `19ec97fa` (before edits). This is a representation
change; execution, routing authority, credentials and lifecycle remain owned by
their existing domains. Sizes below count ASCII characters, not tokenizer tokens.

## Rules

First remove unnecessary transmission, duplicated parent identity and proof used
only for error classification. Then choose strength, then encoding. Non-secret
random identities use 12 CSPRNG bytes (16 base64url characters), with collision
handling at the owning collection/transaction. Possession and replay proofs use
at least 16 CSPRNG bytes (22 characters). Semantic fences use 128 digest bits;
authenticated cursors retain 128 MAC bits. Deterministic idempotency digests keep
their full existing strength. Credentials are outside this change.

## Inventory and intended formats

`B64(n)` means canonical URL-safe Base64 without padding of n bytes. For rows
with several prefixes, old/new totals are prefix length plus suffix length.
Only Workflow Session and Session Message identities require persisted legacy
compatibility. Other formats below switch directly; existing files are not rewritten.

| Identity / semantic class | Before → intended after (chars) | Generation / validation / persistence owner | Model projection / repeated parent |
| --- | --- | --- | --- |
| Ordinary Job, random identity | UUID (36) → `wc_job_` + B64(12) (23) | runner-registry/job_updates; registry `jobs_by_id`; Runner Job records | Job results and observe/write/stop inputs; independently required |
| Detached Job, deterministic idempotency | `detached_` + SHA256 hex (73) → B64(32) (52) | runner-registry/job_updates; detached receipts and Runner supervisor | same Job projections; retain full digest |
| Job cursor | `wj2a:job:epoch:rev:out:err` (typically 84+) → `wj3_` + binding + base36 state | core/job_observation; registry frozen log projection; ephemeral | observe_jobs supplies Job separately; 96-bit Job+epoch digest binding replaces both |
| Managed routing name | basename + full UUID → basename + escalating 8/12/16/24/32/64 digest slug | runner/projects/managed_worktree; Project TOML and Git worktree | auto Project ID in coding inputs; exact operation UUID stays internal for recovery |
| Workflow Session identity | `wc_sess_` + 32 hex (40) → B64(12) (24) | workflow-session/store, events, persistence; session ledger | Session inputs, message authors, Goal correlations; accept old lower-case hex |
| Session Message identity | `wc_msg_` + 32 hex (39) → B64(12) (23) | workflow-session/store, persistence; retained messages and cross-message links | messages, reply/resolve/supersede inputs; accept old lower-case hex |
| Session message cursor | `wsm1_` + B64(40) (59) → `wsm2_` + B64(24) (37) | workflow-session/messages; ephemeral | Session supplied separately; remove independent binding, retain tag |
| Assignment semantic fence | `wsa1_` + B64(32) (48) → `wsa2_` + B64(16) (27) | workflow-session/assignment; completion fingerprints persisted separately | assignment read/complete; scoped Session+todo+semantic state digest |
| CodingAgent authenticated cursor | `wcar2_` + B64(56) (81) → `wcar3_` + B64(36) (54) | tool_runtime/coding_agent; persistent MAC key, ephemeral epoch | run observation; raw 12-byte epoch + masked sequence + 16-byte HMAC |
| Durable Agent, Endpoint, AgentTask, TaskAttempt, Wake, WakeAttempt, AgentWait, Goal | domain prefix + 32 hex → B64(12) | store/communication, agent_task, agent_wake, agent_wait, goal; SQLite primary keys | tool-contracts input/output schemas, runtime and MCP Apps; independent identities |
| Conversation, participant, conversation Message, Delivery, attention event | domain prefix + 32 hex → B64(12) | store/communication, agent_attention; SQLite primary keys | communication/task continuation projections |
| Task attempt fence, Wake claim/consume proof | domain prefix + 32 hex → B64(16) | store/agent_task and agent_wake; active attempt/replay records | explicit active-turn proof; never ordinary 96-bit identity |
| Plugin binding | `wc_pbind_` + 32 hex (41) → B64(16) (31) | plugin_gateway bindings map; canonical plugin schemas | model repeats exact Runner/provider/catalog binding |
| SSH binding | `wc_sbind_` + 32 hex (41) → B64(16) (31) | ssh_resource_gateway bindings map and schemas | exact provider/resource instance binding |
| Skill identity, deterministic scoped digest | `wc_skill_` + 128-bit hex (41) → B64(16) (31) | runner/skill_store, configured_skills, tool_runtime/skills; catalog | skill lookup/read inputs; retain existing 128-bit digest strength |
| Memory identity | `wc_mem_` + 32 hex (39) → B64(12) (23) | store/memory primary key and schemas | memory lookup/update |
| Computer application/display/surface/element identities | domain prefix + 32 hex → B64(12) | computer registry / platform accessibility collections | computer tools; exact process/snapshot routing remains |
| Persistent shell identity | `wc_shell_` + 32 hex → B64(12) | tool_runtime/session_shell map | explicit shell calls |
| Checkpoint identity | domain prefix + 32 hex → B64(12) | tool_runtime/checkpoint; atomic file publication owns collision retry | model-facing checkpoint inputs/outputs |

## Deliberate exclusions

- PAT, OAuth, client/account/agent authentication and other bearer credentials.
- Internal transport/request/audit/Session event/call IDs, file staging names,
  provider instances, process birth/recovery IDs and managed operation UUIDs.
- `wcdh2` Git continuation and numeric read revisions/generations.
- Cryptographic content/scope digests that have independently required strength.

Phase validation, exact payload measurements, remaining exceptions and final
review are recorded here as implementation completes.

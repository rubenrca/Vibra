# Agent chat: ACP spike findings (issue #36, Phase 0)

Measured on this Mac on 2026-10-12 with the real agents and the user's accounts. Probe: `target/agent-chat-work/spike/acp-probe.mjs` (throwaway; not part of the app). Raw logs: `target/agent-chat-work/spike/logs/`.

Versions: grok 1.0.52 · Claude Code CLI 2.1.296 · claude-agent-acp 0.89.1 (Agent SDK 0.3.293) · codex-cli 0.162.1 · codex-acp 2.2.2 (bundles `@openai/codex` 0.160.1) · Node 22.23.1.

## Launch commands that worked

| Agent | Command (cwd = project directory) |
|---|---|
| Grok | `grok agent stdio` |
| Claude | `CLAUDE_CODE_EXECUTABLE=/Users/rubencollao/.local/bin/claude npx -y @agentclientprotocol/claude-agent-acp@0.89.1` |
| Codex | `CODEX_PATH=/Users/rubencollao/.local/bin/codex npx -y @agentclientprotocol/codex-acp@2.2.2` |

Handshake: `initialize` with `protocolVersion: 1` and `clientCapabilities: { terminal: true }`, then `session/new` with `{ cwd, mcpServers: [] }`, then `session/prompt`.

Both adapters are Node packages, so `node` must be on PATH for the `npx` child (see `launch.rs`).

## 1. Handoff (session on disk, ids, load)

- **Grok:** `session/new` writes `~/.grok/sessions/<percent-encoded cwd>/<id>/` immediately (`summary.json`, `chat_history.jsonl`, `updates.jsonl`). The ACP id is the directory name. `grok -r <id> -p "..."` resumed the ACP-created session and answered (exit 0). `grok sessions list` does not list ACP-created sessions (index not updated), so do not use it for discovery. `session/load` of the id replays the history.
- **Claude:** `session/new` writes nothing. After one prompt, `~/.claude/projects/-Users-rubencollao-Dev-Vibra-target-agent-chat-work-spike-ws/<id>.jsonl` exists and its `sessionId` equals the ACP id. `claude --resume <id>` is the documented resume form (not run headless: prompt budget). `session/load` replays user, tool and assistant messages. `session/list` (cwd-scoped) returns the session with a generated title.
- **Codex:** `session/new` writes `~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<id>.jsonl` immediately (`session_meta`). The ACP id is the file's id suffix. `session/load` replays the history. `codex resume <id>` is the documented form (not run: prompt budget).

Same id everywhere: the ACP `sessionId` is the native id that `grok -r`, `claude --resume` and `codex resume` take.

## 2. Terminals

- **Grok:** uses the client terminal. It sent `terminal/create` (`command: "/bin/bash -lc 'echo hi'"`), then `terminal/wait_for_exit`, `terminal/output` and `terminal/release`. Vibra must implement these when it advertises `terminal: true`.
- **Claude:** runs Bash itself. The adapter never sends `terminal/create`. The tool call is titled with the command ("Terminal", then "echo hi") and streams as `tool_call_update`.
- **Codex:** runs `exec_command` itself. No client terminal requests. The tool call carries `content: [{type: "terminal", terminalId}]` with codex's own id, and output arrives in `_meta.terminal_output_delta`. Vibra can show it, but cannot route it into a Vibra PTY.

Consequence: only Grok can put its command output into a Vibra-owned terminal (phase 4). For Claude and Codex, inline output comes from `tool_call_update`.

## 3. Configuration

- **Grok:** uses `~/.grok/config.toml` (`[models] default = grok-4.7`, `reasoning xhigh`). `session/new` reports `configOptions` `model` (grok-4.7) and `reasoning_effort` (xhigh).
- **Claude:** uses `~/.claude/settings.json`. `configOptions`: `model = opus` and `effort = high` (both match the user's settings). The `agent` option lists the user's plugin agent `codex:codex-rescue`. `mode` defaults to `default` ("Manual").
- **Codex:** the rollout records `model: gpt-6.1-sol`, which matches `~/.codex/config.toml`. `session/new` lists the models but does not flag the current one.

Caveat: the Claude SDK loads the user's settings, so the user's hooks run in ACP sessions too. `~/.claude/settings.json` has Vibra's `SessionStart`, `SessionEnd` and `PermissionRequest` hooks (`~/.vibra/agent-hooks/vibra-claude.sh`), so they should fire for chat sessions as well (not observed in the spike). Vibra must not bind those events to a pane: confirm that the chat process has no `VIBRA_PANE_ID`, or the binding logic will be confused.

## 4. Approvals

Grok's user config sets `permission_mode = "always-approve"`, so no request was seen. Codex's user config did not prompt for an in-workspace file write. Claude did. Real payload (long fields dropped):

```json
{"sessionId":"4be429f7-82f3-46c5-834a-c100c2c9d827",
 "toolCall":{"toolCallId":"toolu_011Ss59xeYqiHmKBwswSDvgn","name":"Write","status":"pending",
   "title":"Write hello-claude.txt","kind":"edit",
   "rawInput":{"file_path":".../spike/ws/hello-claude.txt","content":"ok"},
   "content":[{"type":"diff","path":".../hello-claude.txt","oldText":null,"newText":"ok"}],
   "locations":[{"path":".../hello-claude.txt"}]},
 "options":[{"optionId":"allow-once","name":"Yes","kind":"allow_once"},
            {"optionId":"allow-with-updates","name":"Yes, allow all edits during this session","kind":"allow_always"},
            {"optionId":"reject","name":"No","kind":"reject_once"}]}
```

Reply with `{"outcome":{"outcome":"selected","optionId":"allow-once"}}`. The option ids are not `allow_once`; reply with `optionId`, never with the kind or name. The payload is enough for a native prompt: title, kind, a diff, locations and the option list.

## 5. Codex

- `codex-acp` with `CODEX_PATH` runs the user's codex. The rollout written by the spike has `cli_version: 0.162.1`.
- Without `CODEX_PATH` it runs its bundled `@openai/codex` (0.160.1 in the npx cache). Use `CODEX_PATH` so the app uses the user's version.
- `codex-acp` is good enough for phase 2-3. Plan B (native `codex app-server`) is not needed on current evidence.

## Grok: finding a TUI session id

Grok has no hooks Vibra can use (`~/.grok/hooks` is empty, `[compat.claude] hooks = false`). Options for a pane running `grok` in a TUI:

- Watch `~/.grok/sessions/<encoded cwd>/`: the newest directory whose `updates.jsonl` changed since the pane started. Works, but ambiguous with several Grok TUIs in one cwd.
- ACP `session/list` with `cwd` (needs a Grok process): returns the sessions, sorted by `updatedAt`. It does not identify the TUI's session.
- `~/.grok/active_sessions.json` exists but was `[]` when checked (no TUI was running then); not verified with a running TUI.

Recommendation: bind Grok panes by the directory-watch method, and always confirm with the user before handing off.

## Not verified

- Interactive TUI resume (`grok -r`, `claude --resume`, `codex resume`) was not launched interactively. Grok was verified headless with `-p`.
- Codex out-of-sandbox approvals and Grok approvals were not triggered.
- MCP servers were not exercised. No sign-in was performed; all three used existing logins.

## Session controls

Captured with `initialize` and `session/new` only (no prompts), then `session/set_mode`, `session/set_config_option` and `session/set_model` calls that re-sent each agent's current value, so nothing changed. Probe: `target/agent-chat-work/r2a/probe.py` and `probe_set.py`; raw messages in `target/agent-chat-work/r2a/out/`. `session/load` results carry the same fields for all three (spike logs).

| Agent | Modes (`modes`) | Model | Thought level | Other options |
|---|---|---|---|---|
| Grok 1.0.52 | none | `configOptions` `model` (4 values), plus `models` | `reasoning_effort`, category `thought_level` (4 values) | none |
| Claude 0.89.1 | 5: Manual, Accept edits, Plan, Auto, Bypass permissions | `configOptions` `model` (13 values, current `opus`); no `models` | `effort`, category `thought_level` (6 values) | `mode` (mirrors `modes`), `fast` (`model_config`), `agent` (no category; the user's plugin agents) |
| Codex 2.2.2 | 4: Read-only, Workspace access, Auto review, Full access | `configOptions` `model` (7 values), plus `models` (40 ids such as `gpt-6.1-sol[xhigh]`) | `reasoning_effort`, category `thought_level` (6 values) | `mode` (mirrors `modes`), `collaboration_mode` (Default, Plan), `fast-mode` (`model_config`) |

Shapes, trimmed from the real responses:

```json
{"sessionId": "…",
 "modes": {"currentModeId": "default", "availableModes": [{"id": "default", "name": "Manual", "description": "Always ask before making changes"}]},
 "configOptions": [{"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "opus",
   "options": [{"value": "opus", "name": "Opus 5.5", "description": "For complex work and everyday tasks"}]}]}
```

Grok's `models` has the same shape with `currentModelId` and `availableModels[{modelId, name, description, _meta}]`. Codex's `models` is the same shape and each id bundles the effort (`gpt-6.1-sol[xhigh]`), so Vibra uses the `model` and `reasoning_effort` options instead.

Answers to the set calls:

- `session/set_mode` returns `{}` (Claude, Codex). Grok has no modes.
- `session/set_config_option` returns `{configOptions}` (all three).
- `session/set_model` returns `{}` (Codex) or `{_meta: …}` (Grok). Neither returns options, so the value is applied locally. Claude has no `models` and changes the model with `set_config_option`.

Notifications seen:

- `available_commands_update` after setup (all three). Grok sent further copies about 3 s after setup.
- `current_mode_update` and `config_option_update` after a Claude mode or config change. `config_option_update` also came after Grok's set calls. Codex sent neither on its no-op calls.
- `session_info_update`: `title` appears after the first prompt (spike logs). Codex also sends updates with no title, only `_meta`.
- `usage_update` `{used, size}` after turns (spike logs: Claude 22224 of 1000000, Codex 19821 of 475000).
- Vendor notifications (`_x.ai/*`, `_auth/status_update`) are ignored.

Mapping rules:

- The synthetic model option (`__model`, from `models`) is added only when no option has category `model`. None of the three needs it.
- `session/set_model` is used only for `__model`. Real options go through `session/set_config_option`.
- Grouped options (`{group, name, options}`) are flattened. Boolean options are skipped.

## Chat interaction direction

Reviewed [T3 Code's scroll anchoring](https://github.com/pingdotgg/t3code/blob/main/apps/web/src/components/chat/timelineScrollAnchoring.ts),
[MonoCode's queue dispatch](https://github.com/hardbeat920/monocode/blob/main/src/features/sessions/model/messageQueue.ts),
and [its queue controls](https://github.com/hardbeat920/monocode/blob/main/src/features/sessions/ui/MessageQueue.tsx)
on 2026-10-12. Vibra adapts the interaction patterns to its existing pane and composer:

- A reader scrolling through the transcript keeps their position during streaming and automatic queued sends. A small **Jump to latest** control appears above the composer; explicit sends and queue resumption return to the newest message.
- Only successful turns automatically advance the queue. Stop pauses it immediately, including when completion races with cancellation; errors, refusals and token limits hold the remaining prompts. **Resume** starts the next queued prompt, preserving the current draft. Sending a new prompt also releases the queue for subsequent successful turns.
- The queue shows at most four rows before scrolling. File counts reveal the attachment names on hover; editing a queued prompt still returns its files to the composer.
- **Stop** stays available while drafting a follow-up. **Command-period** stops without discarding the draft or attached files; Escape retains its existing behavior.
- Choosing another provider before the first prompt preserves the unsent text and file references.
- Empty chats show only the composer. Subscription usage stays in Vibra's status bar; the chat has no separate usage footer or Continue in Terminal button.
- The editor accepts input while the agent connects. Prompts entered during startup wait in the queue and are dispatched after the session's model and mode metadata arrive. Cached adapters at the exact pinned version launch directly with Node; npx remains the fallback for first installs or an unavailable cache entry.

The transcript remains the primary surface. These controls fit the existing composer and appear when relevant, without adding a new sidebar or permanent header.

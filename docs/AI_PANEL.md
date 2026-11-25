# Assistant panel: build a game by describing it

Status: **A1 built** (2026-11-25); A2 and A3 not started. D20 below is not yet confirmed. Depends on [BEHAVIOURS.md](BEHAVIOURS.md) (D19) for reliable results; uses `box mcp` (D15) and the live control socket. User guide: [guide/editor.md](guide/editor.md#the-assistant).

## Goal

A newcomer types "a coin-collecting game on a floating island, with spikes and a moving platform" into a panel in the editor. A few minutes later the scene holds a playable game built from presets and behaviours. The assistant has rendered it, played it with a playtest to show it can be finished, and explained what it made in plain words. The user then keeps going in either direction: tweak in the inspector, or ask ("make the platform slower", "add a second level"). Every assistant turn is one undo step.

What the panel is not: a code generator that leaves the user with scripts they cannot read, a cloud service the engine depends on, or anything that runs in exported games.

## What exists today

- `box mcp` (`apps/cli/src/mcp.rs`, D15): MCP over stdio with offline tools (`check`, `registry`, `scene_list`, `scene_show`, `scene_apply`, `render`, `walk_route`, `playtest`, `import`) and live tools that drive a running editor or player through the control socket (`status`, `world_query`, `apply_commands`, `screenshot`, `play`, `stop`, `pause`, `step_frames`, `send_input`, `teleport`, `record_start`, `record_stop`, `read_log`, `frame_stats`, audio).
- The editor serves the socket when File ▸ Allow Agent Control is on (`--control`). Agent edits arrive through `box_editor::control::apply_commands`, close any open edit, and become one undo step named "Agent edit"; during Play they are flagged `discarded_on_stop`.
- Every new project gets `AGENTS.md` and `CLAUDE.md` (`crates/box_engine/src/project_agents.md`) that teach an agent the project layout, commands and known limits.
- So an external agent (Claude Code with `claude mcp add box -- box mcp --project …`) can already build in a running editor. A newcomer does not know this is possible, has to set it up in a terminal, and sees the results only as edits appearing with no explanation in the editor.

The panel makes this path visible, one click to start, and readable.

## Proposal

### The panel

**View ▸ Assistant** (also the palette: "ask", "assistant"), docked next to the Inspector by default.

- A transcript of turns. Each assistant turn shows its text, then a compact list of steps in plain words ("Added 12 Pickups around the island", "Rendered the scene", "Played to the goal: won in 41 s"), built from the tool calls. Tool details (commands, JSON) fold away. Captures from `render`/`screenshot` appear as thumbnails; clicking one opens it full size.
- Entity names in the transcript are links: clicking selects and frames the entity.
- An input box with the current context shown as chips: the open scene, the selection ("Platform 2"), whether Play is running. The selection and the viewport camera go with every message, so "make this bigger" and "put a tree here" work.
- **Stop** ends the turn at once (the backend is cancelled; finished edits stay, as one undo step). **Undo turn** on each turn reverts it (the scene's undo history, not a separate one).
- Empty-state prompts suited to the project ("Make a coin-collecting game", "Add a moving platform", "Why can't I jump onto the ledge?"), and on the Welcome card a **Describe your game** field that creates a project from the third-person template and starts the first turn.

### One undo step per turn

`EditStack` gains a turn group: `begin_group(label)` / `end_group()`. While a turn is open, every `apply_commands` (and file write, below) lands in the group, labelled "Assistant: <first words of the prompt>". Cmd+Z after a turn undoes the whole turn. The user editing the scene during a turn closes the group first, so their edit is never folded into the assistant's.

### Backends

The panel talks to an **agent backend** through a small trait (`AgentBackend`: start a turn with text + context, stream events back: text, tool step, capture, question, done, error; cancel). Two backends; the user picks one in the editor's preferences (user config folder, never the project):

1. **External agent** (first, A1). The editor starts an installed agent CLI in headless streaming mode, with `box mcp --project <dir>` as its only MCP server and the editor's control socket switched on for the session. First target: Claude Code (`claude -p` with stream-JSON input and output, an MCP config naming `box`, the allowed tools limited to `mcp__box__*`; exact flags checked against its current docs when built). The editor stores no key: the agent uses its own login. Other agents can be added as further adapters of the same trait.
2. **Built-in** (later, A3). The editor runs the loop itself: it starts `box mcp` as a child process and is its MCP client (so the tools stay in one place), and calls the Claude Messages API over HTTPS (raw HTTP: there is no official Rust SDK). Defaults: model `claude-opus-5-5`, adaptive thinking, effort `high` for building turns and `low` for questions, streaming with `eager_input_streaming` on tools (inputs validated against the tool schema before running), `tool_choice: auto` (forced tool choice is rejected by current models), prompt caching on the stable prefix (tools, system prompt, the project's `AGENTS.md`), server-side refusal fallbacks, compaction for long sessions. The key comes from `ANTHROPIC_API_KEY` or the OS keychain, never from project files. Token use is shown per turn.

Anthropic's hosted Managed Agents were considered and left out: the tools act on the editor running on the user's machine (live world, selection, Play), so the loop has to reach localhost, and a hosted sandbox adds nothing the panel needs.

**Off by default.** Nothing connects to a network until the user picks a backend. The vision's "no proprietary services" holds: the engine, the player and exported games never depend on the panel, and `box mcp` stays the open interface for any agent.

### Tools the panel needs (added to `box mcp`, so every agent gets them)

| Tool | Why |
|---|---|
| `editor_context` (live) | Selection (uids, names), viewport camera pose, open scene, Play state. The panel also sends it with each message; the tool lets an agent refresh it mid-turn. |
| `add_preset` (through `scene_apply` / `apply_commands`) | From D19: the agent builds with presets, not raw component lists. |
| `list_presets`, `describe_behaviour` | What can be placed, with plain-language docs, so the agent does not guess field names. |
| `read_asset`, `write_asset` | Text assets under `assets/` only (`.luau`, `.json`, `.strings.json`, `.wgsl`); writes are part of the turn's undo group (`asset_undo` already tracks material files). No other file access: the panel's agent gets no shell. |
| `undo_turn` (live) | The agent can back out of its own failed attempt cleanly. |
| `docs_search` | Searches the bundled guides and the script reference, so answers quote the engine's own docs instead of memory. |
| `ask_user` (panel only) | A question with choices rendered as buttons ("Third-person or top-down?"). |

### How the assistant works (system prompt and project `AGENTS.md`)

- **Prefer presets and behaviours**, then `Interactable` actions, and only write Luau when nothing fits (and say why). This keeps results editable in the inspector, which is the point for a newcomer.
- **Check before claiming.** After building: `check` (no problems), `render` (look at it, show the user), and a playtest or `walk_route` for anything the player must reach. The turn summary states what was verified and what was not.
- **Explain in the user's words.** Short summary first, then what to try ("Press Play and collect the coins; the gate opens at 12").
- **Ask when it matters.** Genre, camera and win condition are worth one question; colours and counts are not.
- **Stay in the project.** No files outside `assets/`, no deleting scenes or assets without an `ask_user` confirmation.

These rules live in `project_agents.md`, so an external agent outside the editor follows them too.

### Safety and trust

- The control socket stays token-protected and local (D15); the panel only switches it on for its own session and off when the panel closes, unless the user had it on.
- Destructive steps (delete an asset or scene, overwrite a script the user wrote, replace the scene) need a confirmation card in the panel.
- Edits during Play are labelled in the transcript as discarded on Stop, unless the agent used `apply_and_save`.
- Every turn has a hard step limit and the Stop button. With the built-in backend, a per-session token budget.

## Acceptance

- `crates/box_editor/tests/assistant.rs` with a **scripted backend** (replays a recorded event stream; no network): the transcript renders text, steps and thumbnails; entity links select; one Cmd+Z undoes a whole multi-command turn; a user edit during a turn is a separate undo step; Stop leaves the finished edits as one step; a destructive step waits for the confirmation card. Captures in `target/captures/assistant/` (`transcript.png`, `confirm.png`).
- `apps/cli/tests/mcp.rs` covers the new tools (`editor_context`, `read_asset`/`write_asset` path checks, `undo_turn`, `docs_search`, `list_presets`).
- **Evaluation set** (`docs/measurements/assistant_eval.md`, run by hand, never in CI): 10 prompts from "coin game on an island" to "add a locked door that needs two keys", each run in a fresh project with the Claude Code backend. Pass = `box check` clean, the playtest the agent wrote passes, and a person agrees the result matches the prompt. Record pass rate, turn time, and tokens; re-run after changes to presets, tools or `project_agents.md`.
- **User test:** someone new to box-engine makes a game with the panel only, then changes three things in the inspector without help. Recorded with the behaviours user test.

## A1 as built

Code: `crates/box_editor/src/assistant/` (`backend.rs`: `AgentBackend`, `AgentEvent`, `ScriptedBackend`; `claude_code.rs`: the Claude Code process and its stream-JSON parser; `mod.rs`: turns, undo, Undo Turn, the context note; `window.rs`: the window; `prompt.md`: the instructions appended to the agent's system prompt), `EditStack::begin_group`/`end_group` in `crates/box_core/src/edit/mod.rs`, `editor_context` and `undo_turn` on the control socket (`ControlHost`), `read_asset`/`write_asset` in `box mcp` (`crates/box_engine/src/agent_files.rs`). Tests: `crates/box_editor/tests/assistant.rs` (scripted turns, undo rules, Stop, the agent's undo, a stand-in `claude` process), the MCP conversation test, the editor control socket test. Capture: `target/captures/assistant/transcript.png`.

Where it differs from the proposal:

- The panel is a floating window (View ▸ Assistant, palette "assistant"), like the Mixer, not a docked pane: no layout migration and no change to the editor golden. Docking can follow if people keep it open.
- The Claude Code command line: `claude -p --input-format stream-json --output-format stream-json --verbose --strict-mcp-config --tools "" --allowedTools mcp__box --setting-sources project,local --mcp-config <box mcp for the project> --append-system-prompt <prompt.md>`, run in the project folder (so the project's `CLAUDE.md`/`AGENTS.md` apply, the user's own hooks do not). One process per project stays up between turns; Stop kills it and the next message adds `--resume <session>`. `claude` is found on PATH or in the usual install folders (an editor started from Finder has a short PATH); `box` next to the editor executable or on PATH. `BOX_ASSISTANT_COMMAND` and `BOX_ASSISTANT_BOX` override them.
- Allow Agent Control is switched on for a turn and off again after it, if the panel turned it on.
- Undo: a turn opens an `EditStack` group; each run of consecutive "Agent edit" steps becomes one step labelled "Assistant: <first words>". A user edit during the turn stays its own step and splits the run; Undo Turn then refuses ("undo the later changes first"), because undoing around the user's edit could conflict. File writes: `write_asset` copies the replaced file to `.box/assistant/backups/<seq>` and appends to `.box/assistant/journal.jsonl`; Undo Turn restores the entries the turn added.
- Thumbnails, entity links, `ask_user` cards, `docs_search`, `list_presets` and `describe_behaviour` are A2 (`box registry` already lists presets).
- Verified against the real Claude Code 2.1 CLI (`real_claude_code_builds_a_small_game`, ignored, run by hand): the request "a player, five pickups in a row and a goal that wins once every pickup is collected, then check the project" took 133 s and about 0.8 USD (as Claude Code reports it). The agent placed the presets, gated the goal on `GroupDone`, added `GameRules` and `HudSettings`, removed the starter cube in the way, ran `check`, took a screenshot and confirmed with a playtest; the result was one undo step. A sign-in failure arrives as a failed turn with a hint.
- What that run changed: the agent wrote Markdown (the panel now shows text plain, and the prompt asks for plain text); it looked up `Pickup` and `Condition` in `registry` (registry by name now also answers for presets and field types); it read the saved file instead of the live scene (the prompt says which tool reads what); it could not confirm the win in a playtest (the prompt names the `game won` log line).

## Phases

| Phase | Scope |
|---|---|
| A1 (built) | Panel, transcript, context chips, `AgentBackend` with the Claude Code adapter, turn undo groups, Stop, `editor_context`, `read_asset`/`write_asset`, `undo_turn`; `project_agents.md` rules. |
| A2 | Thumbnails, entity links, `ask_user` and confirmation cards, `docs_search`, `list_presets`/`describe_behaviour` (with D19 B1), Describe your game on the Welcome card, the evaluation set. |
| A3 | Built-in backend (MCP client in the editor, Messages API over HTTPS, keychain, token display and budget). |

## Decision

### D20. In-editor assistant panel (proposed)

**Decision.** How a newcomer builds with an AI agent from inside the editor.

**Recommendation.** An Assistant pane that runs an agent backend against `box mcp` and the editor's own control socket, so the editor, CLI and outside agents share one tool layer. First backend: an installed external agent (Claude Code) in headless streaming mode, so the editor holds no keys and adds no network code. Later: a built-in loop on the Messages API that reuses `box mcp` as an MCP child process. One undo group per turn. Agents build with D19 presets and behaviours first, and verify with `check`, `render` and playtests before reporting.

**Alternatives.** (1) Document the external setup only (today): works, but newcomers never find it and see unexplained edits. (2) Built-in API loop first: one install for the user, but adds an HTTP client, key storage and a dependency on one provider to the editor before the UX is proven. (3) Hosted agents (Managed Agents): the tools must reach the local editor; nothing gained.

**Trade-offs.** The first backend requires the user to install and sign in to an agent CLI. Quality depends on the presets, tool docs and `project_agents.md`, so those need the evaluation set kept current. An optional AI feature in the editor has to stay clearly optional.

**Reversibility.** Easy (editor-only; no saved formats change beyond the new MCP tools).

**Confidence.** Medium.

## Open questions

1. Is requiring an installed agent CLI acceptable for A1, or should the built-in backend come first for a one-download experience?
2. Should the panel ship in release builds before the evaluation set reaches an agreed pass rate (for example 8 of 10), or stay behind a preference until then?
3. Asset generation (models, sounds) from the panel: out of scope here; a separate optional plugin if wanted.

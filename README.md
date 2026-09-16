# forgetmenot

Trigger-based, scoped LLM memory.

forgetmenot is a memory server for LLM coding agents, for people who work on many projects across several machines. It works with Claude Code today.

- **Scopes.** Every memory belongs to one or more scopes, such as a project, a topic, a machine or a session. A session receives only the memories of the scopes active in it.
- **Triggers.** Regular expressions matched against messages and tool calls activate scopes automatically, so the agent does not have to remember to. The agent can also turn scopes on and off itself.
- **Critical memories.** Critical memories are delivered in full whenever they apply; other memories are delivered as their one-line description, and the full text only when the agent asks for it.
- **Interception.** A tool call that brings a critical memory into play is held until the agent has been given that memory.
- **Reminders.** Optionally, everything that applies is delivered again after a set number of context tokens: critical memories in full, the rest as their descriptions.
- **Git.** The store is a git repository of markdown files. Every change is a commit; several changes can be made on a branch and landed as one. Several agents and people can write at the same time: their changes are merged, and only edits to the same lines conflict.
- **Backwards compatible.** Memory files use Claude Code's own auto-memory format, so an existing Claude Code memory directory can be imported directly, and migrating back to vanilla memory is easy as Claude Code can read forgetmenot's memory format (losing only the extended functionality).
- **Frontend.** A web page to browse and edit memories and scopes, and to watch live sessions.
- **Statistics.** Every trigger match, every memory shown to the agent and every memory it reads is recorded, so unused memories are visible.

One server serves every machine on a network, and subagents receive the same memories as the session that spawned them.

## Components

| Component | What it is |
|---|---|
| `forgetmenot` | The server: hook endpoint, MCP server, JSON API, static frontend |
| `forgetmenot-hook` | A small client that Claude Code runs on each hook event; it forwards the event to the server and prints the answer |
| `web/` | The frontend: browse and edit memories and scopes, watch live contexts and statistics |
| store | A git repository of scope and memory files, the source of truth |

## Concepts

**Memory.** A markdown note with a one-line description, stored as a file in a git repository. There are two kinds. A *critical* memory holds a rule the agent must follow; whenever it applies, its full text is placed in the agent's context. A *knowledge* memory holds facts the agent may need; the agent sees only its description and reads the full text when it wants it. Memories can link to each other with `[[name]]`.

**Scope.** A label that groups memories: a project, a topic, a machine, a session. A memory belongs to one or more scopes, and a session receives only the memories of the scopes that are active in it. Three scopes exist without being defined anywhere: `global` is active in every session, `machine:<name>` in every session on that machine, and `session:<machine>/<id>` in one session only, for its private notes. Every other scope is defined by a small file naming the scopes it implies and its triggers. A scope with a file exists by its file, and `global` exists always. A `machine:` or `session:` scope exists once the server has seen that machine or that session, or a store file names it; naming a scope in a memory, in another scope's `implies` or in a session's active set does not by itself make that scope exist.

**Trigger.** A condition that automatically activates a scope. Today a trigger is a regular expression, optionally limited to one machine. By default it is matched against everything that flows through a session: the user's messages, the agent's replies, the tool calls it makes and their results, and its directories; a trigger that names one of those with `on` is matched against that text alone. When a trigger matches, its scope becomes active for the rest of the session and the scope's memories are delivered. The inputs and results of the store's own MCP tools, and of any tool named in `trigger_exempt_tools`, are not matched, because they carry the memory system's own scope ids, memory ids and bodies rather than anything about the work. Triggers only turn scopes on; the agent can turn a scope off with a tool call.

**Session and context.** A session is one Claude Code conversation. A context is either the session itself or one subagent inside it; each context keeps its own record of what it has been shown. A session start begins that record afresh and keeps the scopes the session is working in; a session id seen for the first time starts with the three implicit scopes. A subagent starts with the scopes its parent had active.

**Delivery.** At every hook event the server compares what is due, the memories of the active scopes, with what the context has already seen, and sends the difference: memories never shown, memories changed since they were shown, memories shown more than a configurable number of context tokens ago, and notices for memories that were deleted or whose scope was turned off. If a tool call is about to run while a critical memory is due that the context has not seen, the call is held, the memory is delivered, and the agent reissues the call. After a compaction or a resume, everything due arrives once, at that session start.

**Branch.** Several changes to the store can be made on a git branch and landed as one commit. Nothing on a branch is delivered until it lands. See [Branches](#branches).

## Store layout

The store is a git repository:

```
config.yml
scopes/<id>.yaml
memories/<name>.md
memories/sessions/<machine>/<session-id>/<name>.md
```

A scope file names the scopes it implies and its triggers:

```yaml
# scopes/widgets.yaml
id: widgets
implies: [rocketry]
triggers:
  - pattern: '\bwidgets?\b'
  - on: session_directory
    pattern: '/widgets(/|$)'
    machine: alpha
```

A trigger with no `on` is matched against every text of the session; `on` is one of `any`, `user_message`, `assistant_message`, `tool_name`, `tool_input`, `tool_result`, `shell_directory` or `session_directory`. Both directories come from the `cwd` value Claude Code sends with every hook event. `session_directory` is the directory `claude` was started in: the `cwd` of the session's first event, remembered for the whole session and never moved. `shell_directory` is the working directory of the session's shell: the same directory at first, and after the agent runs `cd` in its shell, wherever it went. Both are matched when the session starts, before every tool call, and when the shell directory changes. `machine` restricts a trigger to one machine: the trigger fires only when the session runs on that machine and the pattern matches.

A memory file uses Claude Code's own memory format, with forgetmenot's fields inside `metadata`:

```markdown
---
name: widgets-release-rule
description: Releases of widgets are cut from main only, after the full test suite
metadata:
  kind: critical
  scopes: [widgets]
  source: user
---
Cut releases from `main` only, and only after `cargo test --workspace` is green.
See [[rocketry-notes]].
```

`kind` is `critical` or `knowledge` and defaults to `knowledge`; `scopes` defaults to `[global]`; `source` records who wrote the memory, `user` or `assistant` by convention; any other value is kept. Any other key is preserved untouched, so an existing Claude Code memory directory becomes a valid store the moment it is a git repository.

Every change to the store is a git commit with a title line. A write carries the version of the file it was based on, and is refused if someone else changed the file in between.

### Settings

Everything that changes what the agent experiences lives in `config.yml` at the root of the store, versioned like everything else; where the server listens, what it writes and how long it keeps it stay on the command line. A missing file means all defaults, and a commit changing the file takes effect at the next event in every session.

| Key | Type | Default | Meaning |
|---|---|---|---|
| `reminder_tokens` | integer or null | `null` (off) | Deliver everything that applies again this many context tokens after it was last shown: critical memories in full, knowledge memories as their description |
| `interrupt_on_critical` | bool | `true` | Hold a tool call when a critical memory is due and unseen |
| `interrupt_exempt_tools` | list of strings | `[]` | Tool names never held, by exact match on the tool name |
| `trigger_exempt_tools` | list of strings | `[]` | Tool names whose inputs and results are never matched against triggers, by exact match on the tool name; the store's own MCP tools are never matched whatever this says |
| `subagents_inherit_scopes` | bool | `true` | A subagent starts with its parent's active scopes |
| `deliver_knowledge_index` | bool | `true` | Deliver the description of each knowledge memory that applies, so the agent knows it exists and can ask for the full text; `false` delivers nothing about knowledge memories, the agent has to list them itself |
| `tool_result_match_limit` | integer | `262144` | Bytes of a tool result matched against triggers |
| `answer_file_threshold` | integer or null | `10000` | Characters of a hook answer above which Claude Code saves it to a file and shows the model a preview; an answer past this opens with a notice to read the file; `null` sends no notice |

An unknown key, or a value of the wrong type, is a validation error: `forgetmenot check` reports it and a write is refused. The settings are part of the store, so a branch may change them and land like any other change.

## Interfaces

- `POST /hook`: the hook endpoint, called by `forgetmenot-hook` on every Claude Code hook event.

Claude Code does not show the model a long hook answer. Past 10 000 characters (Claude Code 2.1.270; the length in UTF-16 code units) it writes the answer to a file under the session's `tool-results` directory and shows the model the file's path and the first 2000 characters, cut back to the last newline when that lies past the first 1000. An answer longer than `answer_file_threshold` therefore opens with a short notice telling the model to read the file in full before doing anything else; the notice sits inside the part of the preview that is never cut. The notice is a workaround: the server still records every memory in such an answer as delivered, and the fix, an answer budget with the rest carried to the next event, is [issue 19](https://github.com/ArviFoxy/forgetmenot/issues/19).
- `/mcp`: MCP over streamable HTTP, for the agent.
- `/api/*`: the JSON API used by the frontend. `GET /api/scopes` answers one row per scope that exists, carrying its `id`, its `kind`, the `name` of a session and the `file` of a scope that has one.
- `/`: the frontend, served from `web/dist`.

Claude Code templates for the hooks block and the MCP registration are in `examples/claude-code/`. An example store is in `examples/store/`.

### MCP tools

Memory tools read and change the store; every change is one commit.

| Tool | What it does |
|---|---|
| `memory_index` | List memories with their descriptions, kinds and scopes |
| `memory_get` | Read one memory |
| `memory_put` | Create a memory or replace one whole, including any extra metadata keys |
| `memory_replace_text` | Replace one exact snippet in a body, leaving the rest as it is |
| `memory_set_fields` | Change the description, kind, scopes, source or extra metadata keys without touching the body |
| `memory_rename` | Move a memory to a new id and update every `[[link]]` to it |
| `memory_delete` | Remove a memory; its history stays in git |

A write through these tools records the writer's own session as having seen the new version, the same way `memory_get` does, so the change is delivered to every other session and subagent but not back to its writer; a write on a branch does this when the branch lands.

Settings tools read and change the store's behaviour settings; a change is one commit and is in force for every session.

| Tool | What it does |
|---|---|
| `settings_get` | Report every setting with the value in force, its version and the schema |
| `settings_set` | Change one setting, leaving the rest of the file as it is |

Session tools change what the calling session receives and never touch the store.

| Tool | What it does |
|---|---|
| `session_scopes` | Show the active scopes and the scopes available |
| `session_scope_on`, `session_scope_off` | Turn a scope on or off for this session |
| `session_inherit` | Take over another session's active scopes and its private notes |

Every session tool takes the session key that the first hook event of the session prints.

### Branches

To land several changes as one commit, work on a branch:

```
branch_create                     start a branch from main
memory_put ... branch=<name>      any write, made on the branch instead of main
branch_diff                       see what the branch would change
branch_land message="..."         merge into main as one commit with that title
branch_abandon                    throw the branch away
```

Nothing on a branch reaches any session until it lands. A write on a branch is checked on its own; whether every link resolves and every scope exists is checked when the branch lands, so memories that link to each other can be written in any order. Landing is a three-way merge, so changes made on `main` in the meantime are kept, and two branches that edited different parts of the same memory both land. When the branch and `main` changed the same lines, landing reports the file with its three versions and leaves everything as it was; write the version you want on the branch and land again. Branches are ordinary git refs under `tx/`, so nothing about them is lost on a restart.

## Running

```
cargo build --release
(cd web && npm ci && npm run build)
forgetmenot check --store /path/to/store
forgetmenot serve --store /path/to/store --listen 0.0.0.0:7373 --web-dist web/dist \
    --state-path /var/lib/forgetmenot/contexts.json --stats-path /var/lib/forgetmenot/stats.sqlite \
    --allowed-host memory.example:7373
forgetmenot stats --stats-path /var/lib/forgetmenot/stats.sqlite
```

`--allowed-host` lists every `Host` header value clients use to reach `/mcp`; loopback is always accepted. Session state and open branches are kept indefinitely unless `--context-retention-days` or `--branch-retention-days` is set. How the server behaves towards the agent is the store's own [`config.yml`](#settings), not a flag. `forgetmenot stats` prints the same statistics the frontend shows, or JSON with `--json`.

On each machine that runs Claude Code, put `forgetmenot-hook` on `PATH`, add the hooks block from `examples/claude-code/settings-hooks.json` to Claude Code's settings with the server address and a machine name filled in, and register the MCP server with `claude mcp add --transport http forgetmenot http://SERVER/mcp`.

## State the server holds

*Persistence* says where the state lives and when it is written. *Durability* says what an unexpected stop, a crash or a power loss, can cost; a normal restart loses nothing in any row.

| State | Persistence | Durability |
|---|---|---|
| Scopes, memories and settings | git repository; every change is a commit, written before the request is answered | Durable once the files have been flushed to disk |
| Open branches and their commits | git refs and objects in the same repository | Same as above |
| Parsed catalog and compiled triggers | in memory, derived from the repository's head; rebuilt whenever it moves | Nothing to lose; rebuilt on start |
| Context state (active scopes, what each context has seen) | in memory; snapshot to a JSON file about a second after each change and on shutdown | A crash loses at most the last second of changes |
| Statistics | sqlite in write-ahead-log mode; each record committed as it is written | Durable once written; a crash can lose only records still in the queue |
| MCP transport sessions | in memory | Lost; clients reconnect |

## Runtime limits

These are requirements, checked by tests:

- `cargo test --workspace` under 60 s
- `POST /hook` p99 under 50 ms on loopback with the example store
- `forgetmenot-hook` end to end under 30 ms with a 40 MB transcript, at every event but SessionStart
- `forgetmenot-hook` at SessionStart under 200 ms with a 40 MB transcript
- `npm test` and `npx playwright test` together under 5 minutes

## License

MIT or Apache-2.0, at your option.

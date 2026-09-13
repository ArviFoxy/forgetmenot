# forgetmenot

Trigger-based, scoped LLM memory.

forgetmenot is a memory server for LLM coding agents, for people who work on many projects across several machines. It works with Claude Code today.

- **Scopes.** Every memory belongs to one or more scopes, such as a project, a topic, a machine or a session. A session receives only the memories of the scopes active in it.
- **Triggers.** Regular expressions matched against messages and tool calls activate scopes automatically, so the agent does not have to remember to. The agent can also turn scopes on and off itself.
- **Critical memories.** Critical memories are delivered in full whenever they apply; other memories are delivered as a one-line index and read on demand.
- **Interception.** A tool call that brings a critical memory into play is held until the agent has been given that memory.
- **Reminders.** Optionally, everything that applies is delivered again after a set number of context tokens: critical memories in full, the rest as the index.
- **Git.** The store is a git repository of markdown files. Every change is a commit; several changes can be made on a branch and landed as one. Several agents and people can write at the same time: their changes are merged, and only edits to the same lines conflict.
- **Frontend.** A web page to browse and edit memories and scopes, and to watch live sessions.
- **Statistics.** Every trigger match, delivery and fetch is recorded, so unused memories are visible.

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

**Scope.** A label that groups memories: a project, a topic, a machine, a session. A memory belongs to one or more scopes, and a session receives only the memories of the scopes that are active in it. Three scopes exist without being defined anywhere: `global` is active in every session, `machine:<name>` in every session on that machine, and `session:<machine>/<id>` in one session only, for its private notes. Every other scope is defined by a small file naming the scopes it implies and its triggers.

**Trigger.** A condition that automatically activates a scope. Today a trigger is a regular expression, optionally limited to one machine. By default it is matched against everything that flows through a session: the user's messages, the agent's replies, the tool calls it makes and their results, and its working directory; a trigger that names one of those with `on` is matched against that text alone. When a trigger matches, its scope becomes active for the rest of the session and the scope's memories are delivered. Triggers only turn scopes on; the agent can turn a scope off with a tool call.

**Session and context.** A session is one Claude Code conversation. A context is either the session itself or one subagent inside it; each context keeps its own record of what it has been shown. A subagent starts with the scopes its parent had active.

**Delivery.** At every hook event the server compares what is due, the memories of the active scopes, with what the context has already seen, and sends the difference: memories never shown, memories changed since they were shown, critical memories shown more than a configurable number of context tokens ago, and notices for memories that were deleted or whose scope was turned off. If a tool call is about to run while a critical memory is due that the context has not seen, the call is held, the memory is delivered, and the agent reissues the call.

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
  - on: working_directory
    pattern: '/widgets(/|$)'
    machine: alpha
```

A trigger with no `on` is matched against every text of the session; `on` is one of `any`, `user_message`, `assistant_message`, `tool_name`, `tool_input`, `tool_result` or `working_directory`. `machine` restricts a trigger to one machine, and only a `working_directory` trigger or one matched against every text may carry it, because a path means different things on different machines while a message does not.

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

`kind` is `critical` or `knowledge` and defaults to `knowledge`; `scopes` defaults to `[global]`; `source` records whether the user or the assistant wrote the memory. Any other key is preserved untouched, so an existing Claude Code memory directory becomes a valid store the moment it is a git repository.

Every change to the store is a git commit with a title line. A write carries the version of the file it was based on, and is refused if someone else changed the file in between.

### Settings

Everything that changes what the agent experiences lives in `config.yml` at the root of the store, versioned like everything else; where the server listens, what it writes and how long it keeps it stay on the command line. A missing file means all defaults, and a commit changing the file takes effect at the next event in every session.

| Key | Type | Default | Meaning |
|---|---|---|---|
| `reminder_tokens` | integer or null | `null` (off) | Re-deliver critical memories after this many context tokens |
| `interrupt_on_critical` | bool | `true` | Hold a tool call when a critical memory is due and unseen |
| `interrupt_exempt_tools` | list of strings | `[]` | Tool names never held, by exact match on the tool name |
| `subagents_inherit_scopes` | bool | `true` | A subagent starts with its parent's active scopes |
| `deliver_knowledge_index` | bool | `true` | Deliver knowledge memories as index lines; `false` fetches them on demand only |
| `tool_result_match_limit` | integer | `262144` | Bytes of a tool result matched against triggers |

An unknown key, or a value of the wrong type, is a validation error: `forgetmenot check` reports it and a write is refused. The settings are part of the store, so a branch may change them and land like any other change.

## Interfaces

- `POST /hook`: the hook endpoint, called by `forgetmenot-hook` on every Claude Code hook event.
- `/mcp`: MCP over streamable HTTP, for the agent.
- `/api/*`: the JSON API used by the frontend.
- `/`: the frontend, served from `web/dist`.

Claude Code templates for the hooks block and the MCP registration are in `examples/claude-code/`. An example store is in `examples/store/`.

### MCP tools

Memory tools read and change the store; every change is one commit.

| Tool | What it does |
|---|---|
| `memory_index` | List memories with their descriptions, kinds and scopes |
| `memory_get` | Read one memory |
| `memory_put` | Create a memory or replace one whole |
| `memory_replace_text` | Replace one exact snippet in a body, leaving the rest as it is |
| `memory_set_fields` | Change the description, kind, scopes or source without touching the body |
| `memory_rename` | Move a memory to a new id and update every `[[link]]` to it |
| `memory_delete` | Remove a memory; its history stays in git |

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

Nothing on a branch reaches any session until it lands. Landing is a three-way merge, so changes made on `main` in the meantime are kept, and two branches that edited different parts of the same memory both land. When the branch and `main` changed the same lines, landing reports the file with its three versions and leaves everything as it was; write the version you want on the branch and land again. Branches are ordinary git refs under `tx/`, so nothing about them is lost on a restart.

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
- `forgetmenot-hook` end to end under 30 ms with a 40 MB transcript
- `npm test` and `npx playwright test` together under 5 minutes

## License

MIT or Apache-2.0, at your option.

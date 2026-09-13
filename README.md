# forgetmenot

Trigger-based, scoped LLM memory.

forgetmenot is a memory server for LLM coding agents, for people who work on many projects across several machines. It works with Claude Code today.

- **Scoped memories.** Every memory belongs to scopes: a project, a domain, a machine, a session. A session only sees what applies to it, so the context stays small and relevant.
- **Triggers turn scopes on.** Regular expressions over the user's messages, the agent's commands and tool calls, its output and its working directory activate scopes automatically. Nobody has to remember to load the right memory.
- **Critical memories are always delivered in full.** Rules the agent must not break are never reduced to an index line the agent might skip.
- **A command that surfaces a critical memory is intercepted.** The call is held, the memory is delivered, and the agent reissues the call after reading it. The rule arrives before the action, not after.
- **Periodic reminders.** Critical memories are delivered again after a configurable number of context tokens, because attention to early context fades in long sessions.
- **Stored in git.** Memories are markdown files; every change is a commit with a title line, so history, review and rollback come for free.
- **A web frontend.** Browse and edit memories and scopes in place, test a scope's triggers against sample text, and watch live sessions.
- **Statistics.** Every trigger fire, delivery and fetch is recorded, so you can see which memories are used and which never are.

One server serves every machine on a network, and subagents get the same memories as the session that spawned them.

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

**Trigger.** A regular expression attached to a scope. It is matched against everything that flows through a session: the user's messages, the agent's replies, the tool calls it makes and their results, and its working directory. When a trigger matches, its scope becomes active for the rest of the session and the scope's memories are delivered. Triggers only turn scopes on; the agent can turn a scope off with a tool call.

**Session and context.** A session is one Claude Code conversation. A context is either the session itself or one subagent inside it; each context keeps its own record of what it has been shown. A subagent starts with the scopes its parent had active.

**Delivery.** At every hook event the server compares what is due, the memories of the active scopes, with what the context has already seen, and sends the difference: memories never shown, memories changed since they were shown, critical memories shown more than a configurable number of context tokens ago, and notices for memories that were deleted or whose scope was turned off. If a tool call is about to run while a critical memory is due that the context has not seen, the call is held, the memory is delivered, and the agent reissues the call.

**Branch.** Several changes to the store can be made on a git branch and landed as one commit. Nothing on a branch is delivered until it lands. See [Branches](#branches).

## Store layout

The store is a git repository:

```
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
  - on: tool_input
    pattern: '\bwidgets?\b'
  - on: working_directory
    pattern: '/widgets(/|$)'
    machine: alpha
```

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

`--allowed-host` lists every `Host` header value clients use to reach `/mcp`; loopback is always accepted. Critical memories are delivered again after 200000 context tokens (`--stale-tokens`). Session state and open branches are kept indefinitely unless `--context-retention-days` or `--branch-retention-days` is set. `forgetmenot stats` prints the same statistics the frontend shows, or JSON with `--json`.

On each machine that runs Claude Code, put `forgetmenot-hook` on `PATH`, add the hooks block from `examples/claude-code/settings-hooks.json` to Claude Code's settings with the server address and a machine name filled in, and register the MCP server with `claude mcp add --transport http forgetmenot http://SERVER/mcp`.

## State the server holds

| State | Mechanism | Survives restart |
|---|---|---|
| Scopes and memories | git repository, one commit per write, working tree checked out | yes |
| Open branches and what they hold | git refs under `tx/` and their commits in the store repository, the source of truth | yes |
| Parsed catalog with compiled triggers | in-memory cache of HEAD, rebuilt when HEAD moves | rebuilt |
| Context state (active scopes, shown record) | in-memory map, atomic JSON snapshot on change and on SIGTERM | best effort; loss costs one redundant delivery |
| Statistics (events, trigger fires, deliveries, tool calls) | sqlite, append-only | yes |
| MCP transport sessions | in-memory | no; clients reconnect |

## Runtime limits

These are requirements, checked by tests:

- `cargo test --workspace` under 60 s
- `POST /hook` p99 under 50 ms on loopback with the example store
- `forgetmenot-hook` end to end under 30 ms with a 40 MB transcript
- `npm test` and `npx playwright test` together under 5 minutes

## License

MIT or Apache-2.0, at your option.

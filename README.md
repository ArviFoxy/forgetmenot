# forgetmenot

Trigger-based, scoped LLM memory.

forgetmenot is a memory server for LLM coding agents. It works with Claude Code today. Agents forget what they were told and rarely think to look it up, so forgetmenot does not wait to be asked: it watches what the agent is doing and puts the right memories in front of it at the moment they matter.

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

**Scope.** A flag identified by its id. A scope is either on or off in a context, and the id is all it is: scopes carry no kind or label. The only scopes with special meaning are the implicit ones, which need no file: `global` is always on, `machine:<name>` is on for sessions on that machine, `session:<machine>/<session-id>` is on for one session. A file-backed scope's file carries just its `id`, an `implies` list and its `triggers`.

**Trigger.** A regular expression over one of six strings the harness supplies: `user_message`, `assistant_message`, `tool_name`, `tool_input`, `tool_result`, `working_directory`. A match turns the trigger's scope on in the context where the text appeared. Triggers never turn scopes off. A trigger on `working_directory` may be qualified with a machine name.

**Memory.** A markdown file with YAML frontmatter that belongs to one or more scopes. `kind: critical` memories are delivered in full; `kind: knowledge` memories are delivered as one index line and fetched on demand by id. Bodies may link to other memories with `[[name]]`. Memories in a session scope are a silo: nothing outside the session may link into them.

**Context.** One model conversation: a session, or one subagent inside it. Each context has a set of active scopes and a record of what it has been shown. The server delivers, at every hook event, whatever is due and not yet shown:

```
Due    = memories with any active scope, not archived
Needs  = new        due and never shown here
       ∪ changed    shown, but the file has changed since
       ∪ stale      critical, shown more than K context tokens ago
       ∪ retracted  shown, but archived or its scope turned off
```

At `PreToolUse`, if a critical memory is new or changed, the call is denied with a fixed explanation and the memory is delivered; the agent reissues the call, which then passes. This is a delivery mechanism, not a review of the call. Parallel calls produce exactly one interrupt.

## Store layout

```
scopes/<id>.yaml
memories/<name>.md
memories/sessions/<machine>/<session-id>/<name>.md
```

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

The file format is Claude Code's own auto-memory format: `name` (equal to the file name), `description` (the line the agent sees in an index), and a `metadata` map. forgetmenot keeps its fields inside `metadata` (`kind`, `scopes`, `source`, and server-maintained `archived`, `created`, `author`) and preserves every other key untouched. An existing Claude Code memory directory is therefore a valid store as soon as it is a git repository: files without forgetmenot keys are `knowledge` memories in the `global` scope, and files without frontmatter such as `MEMORY.md` are skipped.

Every write through the API or MCP carries a `message` that becomes the commit title and a `base_version`, the blob hash the editor loaded. A write against a stale version is rejected with the current document.

## Interfaces

- `POST /hook`: the hook endpoint, called by `forgetmenot-hook`.
- `/mcp`: MCP over streamable HTTP. Memory management tools (`memory_index`, `memory_get`, `memory_put`, `memory_archive`) change the store and are git commits. Session management tools (`session_scopes`, `session_scope_on`, `session_scope_off`, `session_inherit`) change only the calling context and never touch the store.
- `/api/*`: the JSON API the frontend uses.
- `/`: the frontend, served from `web/dist`.

Claude Code templates for the hooks block and the MCP registration are in `examples/claude-code/`. An example store is in `examples/store/`.

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

`--allowed-host` lists every `Host` header value clients use to reach `/mcp`; the MCP transport rejects other hosts and always accepts loopback. The stale threshold K defaults to 200000 context tokens (`--stale-tokens`). Contexts are kept indefinitely unless `--context-retention-days` is set. `forgetmenot stats` prints the same aggregates the frontend shows, or one JSON object with `--json`.

On each machine that runs Claude Code, put `forgetmenot-hook` on `PATH`, add the hooks block from `examples/claude-code/settings-hooks.json` to `~/.claude/settings.json` with the server address and a machine name filled in, and register the MCP server with `claude mcp add --transport http forgetmenot http://SERVER/mcp`.

## State the server holds

| State | Mechanism | Survives restart |
|---|---|---|
| Scopes and memories | git repository, one commit per write, working tree checked out | yes |
| Parsed catalog with compiled triggers | in-memory cache of HEAD, rebuilt when HEAD moves | rebuilt |
| Context state (active scopes, shown record) | in-memory map, atomic JSON snapshot on change and on SIGTERM | best effort; loss costs one redundant delivery |
| Statistics (events, trigger fires, deliveries, tool calls) | sqlite, append-only | yes |
| MCP transport sessions | in-memory | no; clients reconnect |

## Runtime limits

These are requirements, checked by tests:

- `cargo test --workspace` under 60 s
- `POST /hook` p99 under 50 ms on loopback with the example store
- `forgetmenot-hook` end to end under 30 ms with a 40 MB transcript
- `npm test` under 30 s

## License

MIT or Apache-2.0, at your option.

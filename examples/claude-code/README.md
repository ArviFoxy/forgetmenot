# Claude Code configuration templates

Replace `SERVER_URL` with the server address (for example `http://memory.example:7373`) and `MACHINE` with a name for the machine the session runs on.

- `settings-hooks.json`: the `hooks` block for `~/.claude/settings.json`. Every listed event runs the `forgetmenot-hook` client, which must be on `PATH`.
- `mcp.json`: an MCP configuration for `claude --mcp-config`, or register with `claude mcp add --transport http forgetmenot SERVER_URL/mcp`.

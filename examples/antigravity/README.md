# Antigravity configuration templates

Replace `SERVER_URL` with the server address (for example `http://memory.example:7373`) and `MACHINE` with a name for the machine the conversation runs on.

- `hooks.json`: the hooks file. Antigravity's command-line agent reads it from `~/.gemini/config/hooks.json`, or from `.agents/hooks.json` in a workspace. Every listed event runs the `forgetmenot-hook` client, which must be on `PATH`, with `--harness antigravity`. The four events are the ones the server acts on; `PostToolUse` and `PostInvocation` are left out because nothing can be done at them.
- `mcp_config.json`: the MCP servers file, read from `~/.gemini/config/mcp_config.json` or from `.agents/mcp_config.json` in a workspace. A file that already lists servers takes the `forgetmenot` entry beside them.

A frontend that starts Antigravity with a configuration directory of its own reads both files from there. T3 Code does: its directory is `~/.t3/userdata/providers/antigravity/<id>/config/`, and Antigravity's `mcp` subcommand does not write to it, so the files are put there directly.

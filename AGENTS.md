# Agent Instructions

These instructions apply to every agent working in this repository.

## Required tools

- Activate and follow **Ponytail** in `full` mode before doing repository work. Prefer the smallest correct change: use existing code and standard-library or native features before adding abstractions, files, or dependencies.
- Use the **AgentMemory MCP** at `http://localhost:3114` at the start of each task to retrieve relevant project context before inspecting or changing code.
- If AgentMemory is unavailable, start `agentmemory --port 3114` as a background process, wait for port 3114 to accept connections, and retry the MCP call. If the global command is unavailable, use `npx -y @agentmemory/agentmemory --port 3114`. If port 3114 is unavailable or occupied by another service, choose a free port, start AgentMemory with `--port <port>`, set `AGENTMEMORY_URL=http://localhost:<port>` for the MCP process, and use that endpoint for the rest of the task.
- Store durable project knowledge in AgentMemory after discovering important architecture, conventions, decisions, or non-obvious fixes. Do not store secrets, credentials, transient command output, or guesses.
- If Ponytail remains unavailable, say so explicitly before continuing and use the closest available fallback.

## Git attribution

Never add Codex, OpenAI, AI-generated, or assistant attribution to commits, authors, co-author trailers, signatures, pull request titles, or pull request descriptions. Use only the user's configured Git identity unless the user explicitly requests otherwise.

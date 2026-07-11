# Agent Instructions

These instructions apply to every agent working in this repository.

## Required tools

- Activate and follow **Ponytail** in `full` mode before doing repository work. Prefer the smallest correct change: use existing code and standard-library or native features before adding abstractions, files, or dependencies.
- Use the **AgentMemory MCP** at the start of each task to retrieve relevant project context before inspecting or changing code.
- Store durable project knowledge in AgentMemory after discovering important architecture, conventions, decisions, or non-obvious fixes. Do not store secrets, credentials, transient command output, or guesses.
- If Ponytail or the AgentMemory MCP is unavailable, say so explicitly before continuing and use the closest available fallback.

## Git attribution

Never add Codex, OpenAI, AI-generated, or assistant attribution to commits, authors, co-author trailers, signatures, pull request titles, or pull request descriptions. Use only the user's configured Git identity unless the user explicitly requests otherwise.

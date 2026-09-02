# Agent OS Index

---
name: "Agent OS Index"
updated: "2026-09-03"
---

Welcome to the OpenEngine Agent OS. This directory contains all infrastructure
for autonomous multi-agent development.

## Quick Start

1. Read `AGENTS.md` (root) for the rules.
2. Read `STATE.md` (root) for the current state.
3. Read `.agents/tasks/INDEX.md` for available work.

## Directory Structure

```
.agents/
├── tasks/          # Atomic, self-contained tasks
├── sessions/       # Active agent sessions (advisory locks)
├── knowledge/      # Stable project knowledge + architecture decisions
├── memory/         # Dynamic learnings (short/long term)
├── decisions/      # Architecture Decision Records (ADRs)
├── events/         # Event log (append-only)
├── INDEX.md        # This file
├── PROTOCOL.md     # How agents communicate
├── ROLES.md        # Available agent roles
└── SECURITY.md     # Security & portability rules
```

## Key Files

- **tasks/INDEX.md**: all tasks with status.
- **knowledge/ARCHITECTURE.md**: why key decisions were made.
- **knowledge/CONSTRAINTS.md**: technical + portability constraints.
- **knowledge/PATTERNS.md**: reusable safe patterns (memory bridge, etc.).
- **memory/LONG_TERM.md**: persistent learnings.
- **decisions/INDEX.md**: all ADRs.

## How to Use This System

1. **Find work**: read `tasks/INDEX.md`, pick a `todo` task.
2. **Execute**: read the task file, follow its `Test Protocol`.
3. **Report**: update task status, log in `events/`, update `STATE.md`.
4. **Learn**: add important findings to `memory/LONG_TERM.md`.

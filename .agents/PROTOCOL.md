# PROTOCOL.md — Agent Communication Protocol

---
name: "Agent Protocol"
version: "1.0.0"
updated: "2026-09-03"
---

This document defines how agents communicate and coordinate.

## Task Lifecycle

```
todo → assigned → in_progress → done
                     ↓
                  blocked → todo (reassign)
```

## Session Lifecycle

```
created → running → completed
              ↓
           blocked → handoff
              ↓
           failed → retry/escalate
```

## How to Pick a Task

1. Read `tasks/INDEX.md`.
2. Filter by: `status="todo"`, priority matches your capability, no blockers.
3. Read the task file completely.
4. Verify you have all `required_context`.
5. Update task status to `assigned` with your agent id.
6. Create a session file in `sessions/`.
7. Begin work.

## How to Report Progress

For long tasks, update:
- Task file: progress notes.
- Session file: actions taken.
- `events/`: progress event.
- `STATE.md`: update only if it blocks others.

## How to Report Completion

1. Task file: `status: done` + completion timestamp + results.
2. Session file: mark completed + summary.
3. `events/`: add completion event.
4. `STATE.md`: mark complete, update phase progress.
5. `memory/LONG_TERM.md`: add learnings.

## How to Handle Blockers

1. Task file: `status: blocked` + reason.
2. Session file: mark blocked.
3. `events/`: blocked event.
4. `STATE.md`: add to Blockers.
5. Create a handoff note in the task file if reassigning.

## Event Types

- `task.created` · `task.assigned` · `task.progress` · `task.blocked` ·
  `task.completed`
- `session.started` · `session.completed` · `session.failed`
- `decision.made` · `memory.updated`

## Message Format

```yaml
timestamp: 2026-09-03T14:30:00Z
agent: <your-agent-id>
event: <event-type>
task_id: <task-id>
session_id: <session-id>
details: <description>
```

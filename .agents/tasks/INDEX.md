# Task Index

---
name: "Task Index"
updated: "2026-09-03"
---

## Active Tasks

| task_id | title | status | priority | phase | depends_on |
|---------|-------|--------|----------|-------|------------|
| TASK-001 | Add Position component to contracts | todo | high | Phase 3 | - |
| TASK-002 | Host writes SoA into guest-allocated buffer | todo | high | Phase 3 | TASK-001 |
| TASK-003 | Guest reads its own buffer (zero unsafe) | todo | high | Phase 3 | TASK-002 |

## Completed Tasks

| task_id | title | completed_at | result |
|---------|-------|--------------|--------|
| TASK-000 | Phase 1 + Phase 2 (scaffold, living window, agent OS) | 2026-09-03 | OK |

## Status Legend

- `todo` available · `assigned` claimed · `in_progress` being worked ·
  `blocked` cannot proceed · `done` complete · `cancelled` obsolete.

## How to Use

1. Filter `status="todo"`.
2. Check `depends_on` is done.
3. Read the task file fully.
4. Update status + create a session file when you start.

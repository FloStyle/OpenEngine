# ROLES.md — Available Agent Roles

---
name: "Agent Roles"
version: "1.0.0"
updated: "2026-09-03"
---

### architect
- Design system architecture; write ADRs; review critical changes.
- Can: modify `contracts/`, `AGENTS.md`; create tasks; make decisions.
- Cannot: skip the test protocol, violate domain boundaries, merge unreviewed.

### coder
- Implement features; write tests; fix bugs; follow task specs exactly.
- Can: implement assigned tasks; write unit/integration tests; update docs;
  report blockers.
- Cannot: change `contracts/` without an ADR; skip the test protocol.

### reviewer
- Review quality; verify test coverage; check portability; validate security.
- Can: review any code; request changes; approve merges; flag violations.
- Cannot: approve without passing tests / a portability check.

### tester
- Write test cases; run suites; verify determinism; test cross-platform.
- Cannot: skip determinism tests; mark tests passing without running them.

### curator
- Maintain documentation; update knowledge base; prune obsolete content.
- Cannot: delete active tasks, remove ADRs, or modify `contracts/`.

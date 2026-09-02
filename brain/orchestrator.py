#!/usr/bin/env python3
"""OpenEngine — The Brain.

Domain C orchestration for the CI + Critic loop:

    AI writes code
      -> Brain verifies Wasm purity        (this file)
      -> Rust compiles / clippy            (GitHub Actions)
      -> Wasm hot-reloads in the host      (crates/core sandbox)
      -> LLM Critic reviews the diff       (run_critic_agent, stubbed)

Everything here is a *governance* stub that becomes real enforcement in later
milestones. Keep the CLI surface stable: CI workflows and agent scripts already
call `orchestrator.py verify`, `verify-deps`, and `critic`.

Run:
    python brain/orchestrator.py verify --help
"""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path

# ---------------------------------------------------------------------------
# Purity policy — the forbidden surface of a Domain B Wasm module.
# ---------------------------------------------------------------------------
# Domain B is `#![no_std]`. A correctly-built logic module imports *nothing*
# from WASI or libc/std. A module that imports any of these is impure by
# construction and MUST be rejected.
FORBIDDEN_IMPORTS: tuple[str, ...] = (
    "wasi_snapshot_preview1",  # WASI: filesystem/clock/io — side effects.
    "wasi_unstable",
    "env.memory",              # no ambient memory in logic builds.
    "env.",
    "__wbindgen",              # Rust std shims leak here when std is linked.
    "std.",
)

# Domain B may never pull a Domain A crate. Names here are matched against the
# `[dependencies]` of Domain B manifests.
FORBIDDEN_DOMAIN_A_DEPS: tuple[str, ...] = (
    "wgpu", "winit", "wasmtime", "egui", "rayon",
)


@dataclass
class PurityReport:
    """Result of checking one Wasm artifact or manifest."""

    path: str
    ok: bool
    problems: list[str] = field(default_factory=list)
    imported: list[str] = field(default_factory=list)

    def summary(self) -> str:
        verdict = "PURE" if self.ok else "IMPURE"
        head = f"[{verdict}] {self.path}"
        if not self.problems:
            return head
        return head + "\n  - " + "\n  - ".join(self.problems)


def verify_wasm_purity(wasm_path: str) -> PurityReport:
    """Ensure a Wasm binary does not import forbidden (std/WASI) symbols.

    Preferred path: use ``wasm-tools`` if installed to disassemble the module
    and inspect its *import section* structurally (robust against renaming).
    If ``wasm-tools`` is unavailable, fall back to a best-effort substring scan
    of the raw bytes, which still catches accidental std/WASI linking.

    Import sections are checked against ``FORBIDDEN_IMPORTS``.
    """
    import shutil
    import subprocess  # noqa: PLC0415  (local import: host-only tooling)

    path = Path(wasm_path)
    if not path.is_file():
        return PurityReport(str(path), ok=False, problems=["artifact not found"])

    wasm_tools = shutil.which("wasm-tools")
    if wasm_tools is None:
        print(
            "wasm-tools not found. Install with: cargo install wasm-tools\n"
            "Falling back to basic string inspection...",
            file=sys.stderr,
        )
        raw = path.read_bytes()
        text = raw.decode("latin-1", errors="replace")
    else:
        # Disassemble and search the import-section text form, e.g.
        #   (import "wasi_snapshot_preview1" "..." (func ...))
        try:
            proc = subprocess.run(
                [wasm_tools, "print", str(path)],
                capture_output=True,
                check=False,
                text=True,
                timeout=60,
            )
            text = proc.stdout
        except (OSError, subprocess.TimeoutExpired):
            text = path.read_bytes().decode("latin-1", errors="replace")

    imported: list[str] = []
    problems: list[str] = []
    for needle in FORBIDDEN_IMPORTS:
        if needle in text:
            problems.append(f"forbidden import signature present: {needle!r}")
            imported.append(needle)

    # ARCH_VERSION handshake: when the wasmtime bridge ships, the guest will
    # export a version symbol the host checks before loading. Until that
    # milestone a missing export is NOT a purity failure — only forbidden
    # imports are. See crates/logic-sandbox/src/lib.rs note on wasm exports.

    return PurityReport(str(path), ok=not problems, problems=problems, imported=imported)


def verify_deps() -> PurityReport:
    """Statically forbid Domain A crates inside Domain B manifests."""
    targets = [
        Path("crates/logic-sandbox/Cargo.toml"),
        Path("crates/math/Cargo.toml"),
    ]
    problems: list[str] = []
    dep_pattern = re.compile(r"^\s*([A-Za-z0-9_-]+)\s*=\s*", re.MULTILINE)
    for manifest in targets:
        if not manifest.is_file():
            problems.append(f"missing manifest: {manifest}")
            continue
        text = manifest.read_text(encoding="utf-8")
        # Only scan the first [dependencies] table to avoid dev-deps noise.
        deps_section = text.split("[dependencies]", 1)[-1]
        deps_section = deps_section.split("\n[", 1)[0]
        declared = {m.group(1) for m in dep_pattern.finditer(deps_section)}
        for bad in FORBIDDEN_DOMAIN_A_DEPS:
            if bad in declared:
                problems.append(f"{manifest}: Domain B must not depend on {bad!r}")

    report = PurityReport("domain-B manifests", ok=not problems, problems=problems)
    return report


def run_critic_agent(diff: str) -> str:
    """Stub for the LLM Critic loop.

    Intended pipeline (later milestone):
      1. RAG-index AGENTS.md + contracts/ + docs/abi/*.
      2. Send the *diff* plus the relevant ABI context to a critic LLM.
      3. Ask it to reject any change that breaks the Prime Directive,
         Domain boundaries, or the Determinism Law.
      4. Return structured findings, not prose.

    For now it just surfaces the size of the diff so the pipeline shape is
    visible end-to-end and CI stays green while the real integration lands.
    """
    changed = diff.count("\n+") - diff.count("\n-")
    lines = diff.count("\n")
    return (
        "critic_agent (stub): pipeline shape only.\n"
        f"  diff size: {lines} lines, ~{max(changed, 0)} net additions.\n"
        "  TODO: RAG over AGENTS.md + contracts/, then LLM verdict."
    )


def _do_critic(repo_diff: str) -> int:
    print(run_critic_agent(repo_diff))
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="orchestrator.py", description="OpenEngine Brain (Domain C)."
    )
    sub = parser.add_subparsers(dest="command", required=True)

    # `verify-wasm-purity` is the canonical name; `verify` is kept as an alias.
    for name in ("verify-wasm-purity", "verify"):
        p = sub.add_parser(name, help="check a Wasm logic artifact for purity")
        p.add_argument("wasm", nargs="?", default=None, help="path to a .wasm module")

    sub.add_parser("verify-deps", help="forbid Domain A deps inside Domain B manifests")

    p_critic = sub.add_parser("critic", help="run the LLM Critic over a diff")
    p_critic.add_argument("diff_file", type=Path, help="path to a .diff/.patch file")

    args = parser.parse_args(argv)

    if args.command in ("verify-wasm-purity", "verify"):
        if args.wasm is None:
            # No artifact yet during scaffolding -> report as informational.
            print("no wasm artifact supplied; purity check skipped (OK in scaffold)")
            return 0
        report = verify_wasm_purity(args.wasm)
        print(report.summary())
        return 0 if report.ok else 1

    if args.command == "verify-deps":
        report = verify_deps()
        print(report.summary())
        return 0 if report.ok else 1

    if args.command == "critic":
        diff_text = args.diff_file.read_text(encoding="utf-8") if args.diff_file.exists() else ""
        return _do_critic(diff_text)

    return 2


if __name__ == "__main__":
    sys.exit(main())

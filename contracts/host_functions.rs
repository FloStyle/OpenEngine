# ABI policy for Wasm host functions (Domain B → Domain A imports)

This file is the **single source of truth** for the host functions a logic
module may import. `brain/orchestrator.py verify` treats the set named here as
the allow-list.

## Current policy

Domain B is pure and `#![no_std]`, so in the ideal steady state the guest
imports **nothing** from the host. The Wasm module simply receives a
`StateView` (as raw memory descriptors) and returns a `WorldDelta`. Keep that
state for as long as possible.

## When you MUST add a host function

Only these cases justify an import, and each requires an ABI note + an
`ARCH_VERSION` bump if it changes the boundary:

1. **RNG seed injection** — a single `seed(epoch: u64) -> ()` so logic can use a
   deterministically-seeded PRNG without ambient entropy. Prefer passing the
   seed through `StateView` instead (pure, zero imports).
2. **Tracing / logs** — host-side buffered logging that never affects the
   result of a pure function.
3. **Large-output spill** — copying a `WorldDelta` bigger than the shared arena
   into a host-allocated buffer.

Rule for agents: **prefer pushing data through `StateView` / `WorldDelta` over
adding imports.** A pure function that needs nothing from the outside is the
strongest guarantee this project has.

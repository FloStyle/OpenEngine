---
spec: "15-networking"
phase: "Phase 7"
status: "design"
---

# Deterministic Rollback Netcode (GGPO-style)

## Overview

Multiplayer is built on **deterministic rollback netcode** of the GGPO style,
and it is only feasible because of the architecture already committed to in this
repo: a **fixed-timestep simulation** (`01-game-loop.md`) of **pure systems**
(`fn(&StateView) -> Result<WorldDelta, RecoverableError>`) over **fixed-point
math**. Because two peers that run the same `StateView` at the same tick always
produce the same `WorldDelta`, a peer can (a) *predict* the future locally using
inputs it has assumed, (b) *roll back* to a saved world snapshot the moment the
authoritative inputs for a past tick arrive, and (c) *re-simulate* forward from
that snapshot to the present — and the result is bit-identical to what the remote
peer computed.

The **only thing transmitted between peers is player input**, encoded as
fixed-size commands. No state is ever synced over the wire. Only Domain A does
transport (`tokio` + QUIC or a UDP socket); Domain B stays pure, receives input
as an ordinary deterministic stream, and never touches a socket.

## Why fixed-timestep + pure systems is a precondition

Rollback netcode re-executes simulation from arbitrary past ticks. That is only
meaningful if:

1. **The world at tick N is fully reconstructible** from a saved snapshot plus
   the ordered list of inputs for ticks `N..present` — no wall-clock, no ambient
   RNG, no `f32` jitter that could differ between the rollback pass and the live
   pass.
2. **Re-simulation is cheap and pure** — the same `StateView` in, the same
   `WorldDelta` out, so replaying is just calling the same registered systems
   again.
3. **Inputs are the sole source of external variance** — every other difference
   (physics from `13-physics-basics`, AI, spawn order) is already deterministic.

The existing loop gives all three. `sim_time` advances in fixed ticks; systems
run against the immutable `StateView` in registration order; the host applies
deltas atomically. That design — built for reproducibility — is exactly what
rollback needs.

```
 local ticks .............│...│...│...│...│...│...│ (predicted, optimistic)
                          │     packet for tick T arrives
                          ▼
 snapshot(T) ──rollback──► re-sim T+1..current with authoritative inputs
                          │ (bit-identical to remote)
                          ▼
                  replay render to present
```

## Design

### The only wire message: input commands

Peers exchange **fixed-size input commands** that carry every bit of player
intent needed to advance exactly one tick deterministically. The payload is
small, fixed-width, and versioned so the host can reject mismatched modules.

```rust
/// One tick of one peer's input. THE only thing sent over the network.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable,
         serde::Serialize, serde::Deserialize)]
pub struct InputCommand {
    pub tick: u32,          // the simulation tick this input targets
    pub seq:  u16,          // local send sequence (for ordering/dedup)
    pub move_x: i8,         // -1..=1, quantized fixed input
    pub move_y: i8,
    pub buttons: u16,       // bitfield: jump / attack / interact / ...
    pub look_x: i16,        // quantized fixed look dir (0x7FFF resolution)
    pub look_y: i16,
}
```

The host converts raw device events (keyboard/controller) into these commands in
`PreUpdate`; Domain B only ever reads the deterministic command stream. Because
commands are fixed-size `Pod`, the host packs several ticks of input into a single
datagram/packet (`InputBatch`) to amortize overhead and reduce rollback distance.

### Transport — Domain A only

A `Netcode` driver lives in Domain A (`crates/net`, std) using `tokio` plus a
QUIC or UDP socket (UDP is the primitive GGPO uses; QUIC gives ordered reliable
channels where wanted). Domain B is not linked against `tokio`; the driver sits
in the host and feeds input into the sim between fixed ticks.

```rust
// Domain A (std): owns the socket, never shares it with Domain B.
pub struct Netcode {
    peer: Addr,               // one remote for 2p, peers for FFA
    rtt: Duration,            // measured, feeds input delay
    send_buf: Vec<InputBatch>,
}

impl Netcode {
    pub async fn recv(&mut self) -> Result<Option<InputBatch>, NetError> { /* UDP/QUIC */ }
    pub fn send(&mut self, batch: &InputBatch) { /* enqueue + flush on tick */ }
}
```

Rendering, audio (`14-audio-system.md`) and netcode all run host-side and never
block the fixed sim thread.

### Input delay + prediction

At session start the host measures RTT and sets a fixed **input delay** of
`D` ticks (e.g. `ceil(rtt / fixed_delta) + 1`). While the remote's input for the
tick about to be simulated has not yet arrived, the local peer **predicts**: it
simulates using its assumed/last-known remote input for that tick and continues
forward optimistically. This keeps gameplay responsive; the assumption is
corrected later by rollback.

```rust
// each fixed tick T:
//   - take authoritative local input for tick T
//   - take remote input for tick (T - D); if missing, predict using last known
//     remote input (or a neutral "no input" command)
//   - push (T, local_input, remote_input) into the replay buffer
//   - run the fixed-timestep tick (see 01-game-loop)
```

### Save-state snapshots (rollback support)

Rollback needs a compact, cheap-to-snapshot world at every tick within the
rollback window. The snapshot is a `WorldSnapshot` (defined in
`16-serialization.md`) capturing all SoA columns that systems mutate. Snapshot
cost is minimized by only snapshoting **changed archetype columns** and only
within a rolling window of `W` ticks (≥ `D + margin`), evicting old states once
confirmed.

```rust
pub struct RollbackBuffer {
    inputs: VecDeque<(u32 /*tick*/, LocalInput, RemoteInput)>,
    states: VecDeque<(u32 /*tick*/, WorldSnapshot)>,
    window: usize,          // ticks of history kept for rollback
    confirmed_tick: u32,    // tick the remote has acked as safe
}
```

### Rollback + re-simulate

When an authoritative remote `InputBatch` arrives containing input for a tick
`T < current`, the host:

1. Loads the snapshot at `T` (discarding any live sim state newer than `T`).
2. Replaces the buffered remote input for `T` with the authoritative value.
3. Re-simulates ticks `T..current` by re-running the registered pure systems
   against each snapshot, applying deltas normally — identical because inputs
   and systems are deterministic.
4. Re-renders the now-correct present.

This is exactly the same apply-delta path the offline loop uses; the only new
ingredient is being able to reload an earlier world state.

```rust
fn rollback_to(&mut self, tick: u32, snap: &WorldSnapshot) {
    self.world = snap.clone();                       // restore
    for (t, li, ri) in self.inputs.iter().filter(|(t,_,_)| *t >= tick) {
        // feed the (possibly corrected) remote input, run systems
        let delta = self.run_tick(*t, li, ri)?;
        self.apply(delta);                           // identical to offline
    }
}
```

### Reconciliation

The remote periodically acknowledges the highest tick whose inputs it has
processed and confirmed (`confirmed_tick`). Any inputs/states at or below
`confirmed_tick` are safe and are evicted from the rollback window. If the remote
never receives a tick (packet loss), UDP retransmission of that `InputBatch` and
the ack protocol reconcile the gap. Because both peers deterministically agree on
the post-`confirmed_tick` state, they never diverge for more than the prediction
window.

### Desync detection

Since determinism is guaranteed, any two peers that disagree on state indicate a
**bug** (different code, non-determinism, or a hash-order violation), not
network noise. The host periodically computes a compact `WorldHash` (a
deterministic digest over sorted SoA columns) at a few ticks and piggybacks it in
the ack so each peer can verify the remote's hash matches its own. A mismatch
triggers a rollback to the last common confirmed tick and a non-determinism
report — surfacing Domain B purity violations loudly rather than letting matches
silently diverge.

## Key Rust / types

- `InputCommand` (Pod), `InputBatch`, `RemoteInput`/`LocalInput` — fixed-size
  wire types in `contracts`.
- Host `Netcode` (tokio/UDP/QUIC) in `crates/net`; never linked into Domain B.
- `RollbackBuffer` of `WorldSnapshot` per tick in a rolling window.
- `WorldHash` deterministic digest used for desync checks.
- Pure-system driver shared verbatim with the offline `01-game-loop` fixed tick.

## Constraints

- **Inputs are the only transmitted state.** No position/world syncing.
- Domain B contains no socket code, no `tokio`, no `std::time` — it receives
  inputs as deterministic commands.
- Re-simulation must be byte-identical to the original pass; any divergence is a
  determinism bug to be surfaced, not masked.
- Fixed-timestep and pure systems are **non-negotiable preconditions**; this spec
  does not add a looser alternative.
- Networking must build and pass on `x86_64-linux` and `aarch64-linux`; headless
  CI can run the loop with a fake in-process peer, no real socket needed.

## Performance

- Snapshot: only mutate-changed columns; target < ~100 µs per snapshot tick.
- Rollback distance bounded by input delay + margin (`W`), typically < 8 ticks at
  60 Hz, so a full rollback re-sim is a few fixed ticks (< ~2 ms) — well under
  the 8 ms fixed-update budget.
- Input batching amortizes per-datagram cost; host transport off-thread.

## Testing strategy

- Unit: input packing/unpacking round-trip, buffer eviction at `confirmed_tick`.
- Determinism/rollback: run two in-process sims with a **fake peer**; drop and
  delay inputs on one, roll back, and assert both converge to bit-identical state
  at the same confirmed tick. Run 3× — identical.
- Desync: deliberately inject a non-deterministic value and assert `WorldHash`
  mismatch is detected and reported.
- Replay: save a log of inputs (per `16-serialization`) and re-simulate offline
  from tick 0; assert the result equals the recorded end state.

## Dependencies

- Domain A: `tokio`, `quinn`/`quic` or a UDP crate, `contracts`,
  `openengine-math`. Host `crates/net`.
- Domain B: unchanged (`contracts`, `openengine-math`). No new Domain B dep.
- `16-serialization.md` provides `WorldSnapshot` and replay-input persistence.

## Next steps

1. Add `InputCommand`/`InputBatch`/`WorldHash` to `contracts` (+`docs/abi`).
2. Implement `WorldSnapshot` save/load (see `16-serialization`).
3. Implement `RollbackBuffer` + snapshot-per-tick capture in `crates/core`.
4. Implement `Netcode` transport and ack/reconciliation loop in `crates/net`.
5. Wire input delay + prediction into the fixed loop; add rollback driver.
6. Add fake-peer determinism + desync tests, then a real 2-player test.

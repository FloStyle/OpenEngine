---
spec: "14-audio-system"
phase: "Phase 6"
status: "design"
---

# Audio System

## Overview

Audio is a **Domain A concern**. The host owns the audio device, the mixing
graph, and every handle to a loaded sound asset; Domain B never touches audio
hardware or drivers. When pure gameplay logic wants to hear something — a
collision from `13-physics-basics`, a gunshot, a one-shot footstep loop, an
ambient bed — it emits a *deferred audio request* into its [`WorldDelta`].
Domain A drains those requests and plays them through `rodio`/`cpal`.

The design reuses the canonical `contracts` asset/handle types (a sound asset is
named by `contracts::AssetRef` with `AssetKind::Audio`; a live voice/stream is
identified by `contracts::AudioHandle(u64)`), distinguishes one-shot
from looping playback, carries volume/pan/3D position (converted **only at the
Domain A boundary** from fixed-point), tracks a listener from the camera, and
degrades gracefully when no audio device exists (audio becomes a silent no-op —
never a crash, never a game-logic side effect).

## Design

### Request channel — audio stays out of the pure world

Gameplay never blocks on, or waits for, sound. Physics/logic emits
`AudioRequest`s as `DeferredCommand`s (see `contracts`). `DeferredCommand::Emit`
with an `Audio` topic is the natural carrier, but a dedicated typed variant
should be preferred once `contracts` allows adding one, because it gives the
host a schema it can dispatch without peeking at a topic table.

```rust
/// Domain B -> Domain A. Played by the host; never touched by Domain B.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AudioRequest {
    pub handle: AssetRef,        // sound asset (contracts::AssetRef, AssetKind::Audio)
    pub kind: PlayKind,          // OneShot | Looping
    pub gain: I16F16,            // 0.0 ..= 1.0, fixed-point on the guest side
    pub pan: I16F16,             // -1.0 (L) ..= +1.0 (R); only for 2D pan
    pub position: Option<(I16F16, I16F16)>, // world-space, 3D-attenuated
    pub voice_flags: VoiceFlags, // duckable, positional, exclusive
}
```

All *decision* fields (`gain`, `pan`, `position`, whether to duck) are produced
in fixed-point so gameplay decisions are deterministic and testable. Only when a
request crosses into Domain A are they converted with
`openengine-math::quantize_to_f32` for the device. Conversion at the boundary is
exactly the sanctioned `f32` usage: audio latency/panning is presentation, never
simulation.

### Host playback — rodio/cpal

Domain A (`crates/core`, or a dedicated `crates/audio` host crate) runs an
`AudioEngine` that owns the device and a `rodio::OutputStream`. `rodio` sits on
`cpal` and can either own the output stream (simple) or be handed a
`cpal::Stream` (for headless/no-device CI this crate must still build). The
engine implements a small pool of mixer `Sink`s plus a set of one-shot decoders.

```rust
pub struct AudioEngine {
    // None when no device was found: every play becomes a silent no-op.
    device: Option<rodio::OutputStream>,
    mixer: Option<rodio::Sink>,          // master group
    assets: AssetRegistry,               // AssetRef (Audio kind) -> decoded Source
    ducked: Vec<DuckingGroup>,           // pool metadata
    listener_pos: Option<(f32, f32)>,    // camera, host side only
}

impl AudioEngine {
    /// Drain one frame's deferred audio requests from the applied WorldDelta.
    pub fn drain(&mut self, delta: &WorldDelta) {
        for cmd in &delta.deferred {
            match cmd { /* Audio variant -> self.play(&req); _ => {} */ }
        }
    }

    fn play(&mut self, req: &AudioRequest) {
        let Some(source) = self.assets.get(req.handle) else { return; };
        // clamp gain/pan; convert fixed -> f32 here and only here
        let gain = quantize_to_f32(req.gain);
        // pan/3D attenuation folded in via a host-side spatial helper
        self.spawn_sink(source.clone(), req.kind, gain, pan_vol, pan);
    }
}
```

### Asset references and voice handles

A sound asset is named by the canonical `contracts::AssetRef { id, kind }`
(`kind = AssetKind::Audio`) — the same single asset-reference model used for
meshes; no raw paths and no ad-hoc `AssetHandle`. The host resolves the id
against its `AssetRegistry` at scene load (see `16-serialization`) and keeps
decoded sources (or streamable handles) keyed by asset id. Domain B *names* a
sound by this `AssetRef`; it never stores a decoder, a `Sink`, or a pointer to
audio memory. A live, playing **voice/stream** is identified by the other
canonical handle `contracts::AudioHandle(u64)`, returned by the host when it
spawns a voice and echoed back by gameplay to stop/retarget it.

```rust
// contracts — canonical types only:
// AssetRef { id: u64, kind: AssetKind::Audio }  -> names the sound asset
// AudioHandle(u64)                              -> names one live voice/stream
```

### One-shot vs looping

- **OneShot**: play a decoded source through a transient `rodio::Sink` and drop
  it when the buffer finishes. Cost-controlled by a small cap on concurrent
  one-shots (oldest stopped first).
- **Looping**: play a source repeatedly until an explicit `StopAudio { handle,
  voice }` request or the entity is despawned. Loops are addressable by the
  canonical `contracts::AudioHandle(u64)` the host returns when it spawns the
  voice; gameplay stops a loop by referencing the same `AudioHandle`.

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum PlayKind { OneShot, Looping }
```

Volume and pan are requested per voice. Loops on a distance-attenuated source
follow the entity's current `Position` column, so the host tracks a
`AudioHandle -> Entity` map and re-applies gain each frame — gameplay just moves
the entity and the host attenuates.

### Listener (camera)

The listener position is the camera's world position, read host-side each frame
and cached in `AudioEngine.listener_pos`. It never comes from Domain B's audio
path. 3D attenuation is computed in Domain A from the fixed→f32 world positions
of the speaker and listener: gain `1/(1 + k·dist)`, with the entity's authored
`gain` as the base volume. Panning for non-positional voices is the authored
`pan`; for positional voices pan derives from the speaker's lateral offset from
the listener direction.

### Ducking and pooling

Domain B can request ducking (lowering a group while e.g. dialogue plays) via a
`DuckGroup { group, target_gain, attack, release }` deferred request. The host
holds groups (master, music, sfx, voice, ambient) and cross-fades their sink gain
over host frames. Pooling: the `AudioEngine` keeps a reusable set of transient
`Sink`s and pre-decoded buffers so hot one-shots (impact sounds from
`13-physics-basics`) don't decode per event.

### Graceful failure (no device)

`AudioEngine::device`/`mixer` are `Option`s. `rodio::OutputStream::try_default()`
returns `Err` on headless CI, a container without an audio card, or a locked
device. On `Err` the engine records "no device" and `play`/`duck`/`stop` become
no-ops that still consume requests so the event queue stays bounded and gameplay
never stalls. **No audio device must never change simulation state** — it only
changes whether bytes reach speakers.

## Key Rust / types

- `AudioRequest`, `PlayKind`, `VoiceFlags`, `DuckGroup` — `serde` value types
  crossing in a `WorldDelta` (all fixed-point decision fields).
- `AssetRef` (`AssetKind::Audio`) — sound-asset reference; `AudioHandle(u64)` —
  live voice/stream handle (both canonical `contracts` types).
- Host `AudioEngine` (std) with `drain`, `play`, `stop`, `duck`, `set_listener`.
- `I16F16 -> f32` conversion confined to `crates/audio` at the device boundary.

## Constraints

- Domain B never opens a device, never references `rodio`/`cpal`, never stores a
  mixer/sink. CI enforces the dependency direction (audio crates are Domain A).
- All gameplay audio decisions are fixed-point; `f32` only at the presentation
  boundary (the exact sanctioned use per AGENTS.md).
- No device ⇒ silent no-op, never a panic, never a logic delta change.
- Handle ids only; no pointers to decoded buffers cross into Domain B.
- Portable: audio must build on `x86_64-linux` and `aarch64-linux`, and in Docker
  / headless CI where `cpal` may find no device.

## Performance

- Audio engine is off the fixed-timestep critical path: requests are drained once
  per frame (or on an audio thread) from the applied delta, never synchronously
  inside a physics write.
- One-shot pool cap bounds transient sinks; ducking updates are per-frame cheap
  gain ramps, not per-sample work.
- No blocking waits in Domain B; worst-case host cost is a `Source` clone + sink
  spawn per request.

## Testing strategy

- Unit: fixed→f32 clamping, duck ramp math, `play` with no device returns
  silently and consumes the request.
- Integration (device present, dev machines only): play a one-shot and a loop,
  assert the loop only stops on its matching stop request.
- No-device determinism: run logic tick N times with and without an audio device
  and assert the resulting world (excluding audio) is bit-identical — proving
  audio never leaks into simulation.
- Editor/headless: `AudioEngine` constructs with `device: None` in CI and all
  tests pass without a sound card.

## Dependencies

- `rodio`, `cpal` (Domain A only, behind an optional feature so headless builds
  skip device init).
- `contracts` (`WorldDelta`, `DeferredCommand`, `AssetRef`/`AssetKind`,
  `AudioHandle`), `openengine-math` for fixed→f32 at the boundary.
- Host: `crates/core`, `crates/audio`. No new Domain B dependency.

## Next steps

1. Add an `Audio`/`DeferredCommand` variant (or a stable audio `topic` for
   `DeferredCommand::Emit`) to `contracts`, with `docs/abi` update.
2. Implement `crates/audio` `AudioEngine` with no-device fallback.
3. Implement one-shot pool, looping voice map, and stop-by-voice.
4. Implement listener + 3D attenuation + ducking groups.
5. Emit `AudioRequest`s from gameplay systems (collision from `13-physics-basics`)
   and verify no-device determinism in CI.

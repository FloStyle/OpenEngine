//! # Domain A — `openengine-ecs`
//!
//! Strict **Structure-of-Arrays** (SoA) ECS: one contiguous `Vec<u8>` column per
//! component type, a `World` owning the columns, and spawn/query/apply.
//!
//! ## Phase A (PoC) scope
//! A single fixed archetype holding `Position` + `Velocity` + `Color`. This
//! proves: SoA storage correctness, a 1000-entity workload, and bit-identical
//! determinism. Multi-archetype migration + `ColumnDescriptor` zero-copy
//! bridging to Wasm arrive in later phases (spec 00 / ADR-0001).
//!
//! ## Invariants
//! * **Single mutation channel**: game logic mutates the world only through a
//!   `WorldDelta` applied by `World::apply_delta` — never by holding `&mut`
//!   into a column from a system. `spawn`/internal setup may write columns
//!   directly (host ECS plumbing, not gameplay).
//! * Components are `#[repr(C)] Pod + Zeroable` fixed-point (no `f32`).
//! * `ComponentId`s follow the spec-21 registry.

#![deny(missing_docs)]

pub mod components;
pub mod scene;
pub mod storage;
pub mod world;

pub use components::{Color, Position, Velocity, COLOR, POSITION, VELOCITY};
pub use world::World;

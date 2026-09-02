//! # Domain A — `openengine-ecs`
//!
//! Strict **Structure-of-Arrays** (SoA) entity store with archetype management
//! and **zero-copy memory bridging** into the Wasm logic guest.
//!
//! ## The memory model that makes zero-copy safe
//!
//! * Each archetype owns a set of typed **columns**, each a contiguous `Vec<T>`.
//! * A `ColumnDescriptor` (from `contracts`) names `(component, element_size,
//!   count, data_offset)`. The host packs a *contiguous arena* per tick and
//!   hands Domain B a [`StateView`] that borrows it — the guest reads SoA memory
//!   with no per-entity marshalling and **no copies**.
//! * Writes come back as [`WorldDelta`] whose [`ColumnWrite`] payloads are
//!   already contiguous and are applied with `bytemuck::cast_slice`.
//!
//! ## Hot-path concurrency rules (from AGENTS.md)
//! * **Never `Mutex` in a hot path.** Prefer per-archetype atomic counters and
//!   lock-free work stealing (rayon). Sparse/archetype locks only.
//! * Prefer `bytemuck::cast_slice` for every SoA view — never `transmute`.
//!
//! ## Scaffold state
//! Layout and bridge are specified here; the concrete column/archetype storage
//! arrives in the ECS milestone. Do not write the full ECS now.

#![deny(missing_docs)]

use openengine_contracts::{ArchetypeId, ColumnDescriptor, Entity};

/// Handle type used to reference a live archetype table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArchetypeHandle(ArchetypeId);

/// Describes one archetype's column set.
#[derive(Clone, Debug)]
pub struct ArchetypeDef {
    /// Stable id handed to Domain B.
    pub id: ArchetypeId,
    /// Column descriptors in canonical order.
    pub columns: Vec<ColumnDescriptor>,
}

impl ArchetypeDef {
    /// Number of live entities this archetype currently holds (0 in scaffold).
    pub fn entity_count(&self) -> u32 {
        0
    }
}

/// Skeleton: real spawn lives on the column storage of the ECS milestone.
#[allow(dead_code)]
pub fn spawn_placeholder(_def: &ArchetypeDef) -> Entity {
    Entity {
        generation: 1,
        index: 0,
    }
}

//! The `World`: a single fixed archetype of `Position + Velocity + Color`.

use crate::components::{Color, Position, Velocity, COLOR, POSITION, VELOCITY};
use crate::storage::ArchetypeStorage;

/// A minimal fixed-archetype world for the Phase A PoC.
pub struct World {
    storage: ArchetypeStorage,
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl World {
    /// A world whose archetype holds `Position` (0), `Velocity` (1), `Color` (72).
    pub fn new() -> Self {
        let mut storage = ArchetypeStorage::new();
        storage.add_column::<Position>(POSITION);
        storage.add_column::<Velocity>(VELOCITY);
        storage.add_column::<Color>(COLOR);
        World { storage }
    }

    /// Spawn a new row into the archetype. Returns its stable row index.
    pub fn spawn(&mut self, pos: Position, vel: Velocity, color: Color) -> usize {
        let index = self.storage.allocate();
        self.storage.get_column_mut::<Position>(POSITION).unwrap()[index] = pos;
        self.storage.get_column_mut::<Velocity>(VELOCITY).unwrap()[index] = vel;
        self.storage.get_column_mut::<Color>(COLOR).unwrap()[index] = color;
        index
    }

    /// Number of live entities.
    pub fn entity_count(&self) -> usize {
        self.storage.entity_count()
    }

    /// Read-only `Position` column (first `entity_count` rows valid).
    pub fn get_positions(&self) -> Option<&[Position]> {
        self.storage.get_column::<Position>(POSITION)
    }

    /// Read-only `Velocity` column.
    pub fn get_velocities(&self) -> Option<&[Velocity]> {
        self.storage.get_column::<Velocity>(VELOCITY)
    }

    /// Read-only `Color` column.
    pub fn get_colors(&self) -> Option<&[Color]> {
        self.storage.get_column::<Color>(COLOR)
    }

    /// A deterministic hash of the live rows, for bit-identical-replay checks.
    pub fn hash(&self) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        let n = self.entity_count();
        if let Some(rows) = self.get_positions() {
            for row in rows.iter().take(n) {
                row.x.to_bits().hash(&mut hasher);
                row.y.to_bits().hash(&mut hasher);
            }
        }
        if let Some(rows) = self.get_velocities() {
            for row in rows.iter().take(n) {
                row.x.to_bits().hash(&mut hasher);
                row.y.to_bits().hash(&mut hasher);
            }
        }
        if let Some(rows) = self.get_colors() {
            for row in rows.iter().take(n) {
                (row.r, row.g, row.b, row.a).hash(&mut hasher);
            }
        }
        hasher.finish()
    }

    /// The `ArchetypeId` of this fixed single archetype (always `0`).
    pub const fn archetype_id(&self) -> u32 {
        0
    }

    /// Apply a [`WorldDelta`] through the single mutation channel.
    ///
    /// Only the fixed archetype (id `0`) and the `Position`/`Velocity` columns
    /// are writable in this PoC. Spawns/despawns/deferred are not supported by
    /// the fixed single archetype yet and are ignored (movement writes only).
    /// Each `ColumnWrite` payload must be `indices.len() * element_size` of a
    /// whole registered component (batched write, spec 00).
    pub fn apply_delta(&mut self, delta: &openengine_contracts::WorldDelta) {
        for write in &delta.writes {
            if write.archetype.0 != self.archetype_id() {
                continue;
            }
            let element = match write.component.0 {
                POSITION => core::mem::size_of::<Position>(),
                VELOCITY => core::mem::size_of::<Velocity>(),
                _ => continue,
            };
            // Bounds-check the batched payload is whole elements.
            if write.payload.len() != write.indices.len() * element {
                continue;
            }
            for (slot, idx) in write.indices.iter().enumerate() {
                let off = slot * element;
                let bytes = &write.payload[off..off + element];
                match write.component.0 {
                    POSITION => {
                        if let Some(col) = self.storage.get_column_mut::<Position>(POSITION) {
                            if (*idx as usize) < col.len() {
                                col[*idx as usize] =
                                    bytemuck::pod_read_unaligned::<Position>(bytes);
                            }
                        }
                    }
                    VELOCITY => {
                        if let Some(col) = self.storage.get_column_mut::<Velocity>(VELOCITY) {
                            if (*idx as usize) < col.len() {
                                col[*idx as usize] =
                                    bytemuck::pod_read_unaligned::<Velocity>(bytes);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

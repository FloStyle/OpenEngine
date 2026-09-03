//! SoA archetype storage: one contiguous `Vec<u8>` column per component type.

use std::collections::HashMap;

use bytemuck::Pod;

/// Raw SoA storage for a single fixed archetype.
///
/// Component ids map to `Vec<u8>` byte columns of `element_size * capacity`.
/// Typed views are produced with safe `bytemuck::cast_slice`, never `transmute`.
#[derive(Clone)]
pub struct ArchetypeStorage {
    /// ComponentId → contiguous raw column bytes (length `element_size * capacity`).
    columns: HashMap<u32, Vec<u8>>,
    /// Number of live rows (≤ `capacity`).
    entity_count: usize,
    /// Allocated row capacity (grows by doubling).
    capacity: usize,
}

impl Default for ArchetypeStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl ArchetypeStorage {
    /// An empty store with the default starting capacity.
    pub fn new() -> Self {
        Self {
            columns: HashMap::new(),
            entity_count: 0,
            capacity: 64,
        }
    }

    /// Add a typed column for `component_id`. Must be called before any row uses it.
    pub fn add_column<T: Pod>(&mut self, component_id: u32) {
        let element_size = core::mem::size_of::<T>();
        let data = vec![0u8; element_size * self.capacity];
        self.columns.insert(component_id, data);
    }

    /// Immutable typed view of a column, if present.
    pub fn get_column<T: Pod>(&self, component_id: u32) -> Option<&[T]> {
        self.columns
            .get(&component_id)
            .map(|data| bytemuck::cast_slice(data))
    }

    /// Mutable typed view of a column. Reserved for host ECS plumbing (spawn,
    /// apply_delta), never for gameplay systems.
    pub fn get_column_mut<T: Pod>(&mut self, component_id: u32) -> Option<&mut [T]> {
        self.columns
            .get_mut(&component_id)
            .map(|data| bytemuck::cast_slice_mut(data))
    }

    /// Reserve a new row slot, growing columns when full. Returns the row index.
    pub fn allocate(&mut self) -> usize {
        if self.entity_count >= self.capacity {
            self.grow();
        }
        let index = self.entity_count;
        self.entity_count += 1;
        index
    }

    /// Number of live rows.
    pub fn entity_count(&self) -> usize {
        self.entity_count
    }

    /// Grow every column to `capacity * 2`, preserving existing data.
    fn grow(&mut self) {
        let new_capacity = self.capacity * 2;
        for column in self.columns.values_mut() {
            let element_size = column.len() / self.capacity;
            column.resize(element_size * new_capacity, 0);
        }
        self.capacity = new_capacity;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::{Position, POSITION};

    #[test]
    fn grows_and_keeps_data() {
        let mut s = ArchetypeStorage::new();
        s.add_column::<Position>(POSITION);
        // Force a grow past the initial capacity of 64.
        let mut i = 0usize;
        while i < 200 {
            let idx = s.allocate();
            s.get_column_mut::<Position>(POSITION).unwrap()[idx] = Position {
                x: openengine_math::I16F16::from_num(i as i32),
                y: openengine_math::I16F16::from_num(0),
            };
            i += 1;
        }
        assert_eq!(s.entity_count(), 200);
        let rows = s.get_column::<Position>(POSITION).unwrap();
        assert_eq!(rows[199].x, openengine_math::I16F16::from_num(199));
    }
}

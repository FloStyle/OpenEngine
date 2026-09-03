//! Undo/Redo via Commands (spec 23), headless. Mutations only through these
//! commands → WorldDelta → `World::apply_delta`.

use openengine_contracts::{comp, ArchetypeId, ColumnWrite, ComponentId, Transform, WorldDelta};
use openengine_ecs::World;

/// A command that can re-apply forward and roll back via inverse deltas.
pub trait Command {
    /// Delta that applies the change.
    fn forward(&self) -> WorldDelta;
    /// Delta that reverses the change.
    fn backward(&self) -> WorldDelta;
    /// Human/agent-readable description.
    fn description(&self) -> String;
}

/// Modify one entity's fixed-point `Transform` (id 2) column element.
pub struct ModifyTransformCommand {
    /// Row index in the (single) archetype.
    pub entity_index: u32,
    /// Value before the edit.
    pub old_value: Transform,
    /// Value after the edit.
    pub new_value: Transform,
}

impl Command for ModifyTransformCommand {
    fn forward(&self) -> WorldDelta {
        transform_write(self.entity_index, &self.new_value)
    }
    fn backward(&self) -> WorldDelta {
        transform_write(self.entity_index, &self.old_value)
    }
    fn description(&self) -> String {
        "modify transform".to_owned()
    }
}

fn transform_write(entity_index: u32, value: &Transform) -> WorldDelta {
    let mut delta = WorldDelta::default();
    delta.writes.push(ColumnWrite {
        archetype: ArchetypeId(0),
        component: ComponentId(comp::TRANSFORM),
        indices: vec![entity_index],
        payload: bytemuck::bytes_of(value).to_vec(),
    });
    delta
}

/// Undo/redo history. Deltas are applied by the caller through `World::apply_delta`.
#[derive(Default)]
pub struct UndoRedoManager {
    undo: Vec<Box<dyn Command>>,
    redo: Vec<Box<dyn Command>>,
}

impl UndoRedoManager {
    /// New, empty manager.
    pub fn new() -> Self {
        Self::default()
    }

    /// Execute a command on `world`: apply its forward delta, push to undo,
    /// clear the redo stack.
    pub fn execute(&mut self, world: &mut World, command: Box<dyn Command>) {
        world.apply_delta(&command.forward());
        self.undo.push(command);
        self.redo.clear();
    }

    /// Undo the last command. Returns true if something was undone.
    pub fn undo(&mut self, world: &mut World) -> bool {
        if let Some(command) = self.undo.pop() {
            world.apply_delta(&command.backward());
            self.redo.push(command);
            true
        } else {
            false
        }
    }

    /// Redo the last undone command. Returns true if something was redone.
    pub fn redo(&mut self, world: &mut World) -> bool {
        if let Some(command) = self.redo.pop() {
            world.apply_delta(&command.forward());
            self.undo.push(command);
            true
        } else {
            false
        }
    }

    /// Can we undo?
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Can we redo?
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
}

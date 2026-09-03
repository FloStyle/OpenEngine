//! Edit/Play world isolation (spec 22), headless.

use openengine_ecs::World;

/// Current editor interaction mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorMode {
    /// Authoring: simulation disabled, selection/edits allowed.
    Edit,
    /// A deterministic clone of the edit world is being simulated.
    Playing,
    /// Play is frozen for inspection.
    Paused,
}

/// Holds the persistent edit world and, while playing, an ephemeral clone.
pub struct EditorState {
    /// Current mode.
    pub mode: EditorMode,
    /// Persistent, authorable world (source of truth).
    pub edit_world: World,
    /// Deterministic clone run during play; `None` in edit mode.
    pub play_world: Option<World>,
}

impl EditorState {
    /// A fresh editor over a new empty world.
    pub fn new(edit_world: World) -> Self {
        EditorState {
            mode: EditorMode::Edit,
            edit_world,
            play_world: None,
        }
    }

    /// Enter Play: deep-clone the edit world into the play world (spec 22).
    pub fn play(&mut self) {
        if self.mode == EditorMode::Playing {
            return;
        }
        self.play_world = Some(self.edit_world.clone_state());
        self.mode = EditorMode::Playing;
    }

    /// Pause the simulation (inspect the play world read-only).
    pub fn pause(&mut self) {
        if self.mode == EditorMode::Playing {
            self.mode = EditorMode::Paused;
        }
    }

    /// Resume from Paused.
    pub fn resume(&mut self) {
        if self.mode == EditorMode::Paused {
            self.mode = EditorMode::Playing;
        }
    }

    /// Stop: drop the play world; the edit world is untouched.
    pub fn stop(&mut self) {
        self.play_world = None;
        self.mode = EditorMode::Edit;
    }

    /// The world currently being simulated/edited (edit world when not playing).
    pub fn active_world(&self) -> &World {
        self.play_world.as_ref().unwrap_or(&self.edit_world)
    }

    /// The world that may be mutated right now. Edits are ONLY allowed in edit
    /// mode (on the edit world); while playing the sim mutates the play world.
    pub fn mutable_world(&mut self) -> Option<&mut World> {
        match self.mode {
            EditorMode::Edit => Some(&mut self.edit_world),
            EditorMode::Playing | EditorMode::Paused => self.play_world.as_mut(),
        }
    }

    /// True when the caller (UI/editor) is allowed to author edits.
    pub fn can_edit(&self) -> bool {
        self.mode == EditorMode::Edit
    }
}

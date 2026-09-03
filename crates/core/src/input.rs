//! Host-side keyboard input (Domain A). Read per frame; gameplay math stays in
//! Domain B fixed-point.

use std::collections::HashSet;
use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{KeyCode, PhysicalKey};

/// Per-frame player intent (arrow keys / WASD).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlayerInput {
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
}

/// Tracks held keys + single-frame press edges.
#[derive(Default)]
pub struct InputState {
    pressed: HashSet<KeyCode>,
    just_pressed: HashSet<KeyCode>,
}

impl InputState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn handle_key_event(&mut self, event: &KeyEvent) {
        if let PhysicalKey::Code(key_code) = event.physical_key {
            match event.state {
                ElementState::Pressed => {
                    if !self.pressed.contains(&key_code) {
                        self.just_pressed.insert(key_code);
                    }
                    self.pressed.insert(key_code);
                }
                ElementState::Released => {
                    self.pressed.remove(&key_code);
                }
            }
        }
    }

    pub fn is_pressed(&self, key: KeyCode) -> bool {
        self.pressed.contains(&key)
    }

    /// Clear the single-frame pressed edge set (called once per frame).
    pub fn clear_frame_state(&mut self) {
        self.just_pressed.clear();
    }

    pub fn get_player_input(&self) -> PlayerInput {
        PlayerInput {
            up: self.is_pressed(KeyCode::ArrowUp) || self.is_pressed(KeyCode::KeyW),
            down: self.is_pressed(KeyCode::ArrowDown) || self.is_pressed(KeyCode::KeyS),
            left: self.is_pressed(KeyCode::ArrowLeft) || self.is_pressed(KeyCode::KeyA),
            right: self.is_pressed(KeyCode::ArrowRight) || self.is_pressed(KeyCode::KeyD),
        }
    }
}

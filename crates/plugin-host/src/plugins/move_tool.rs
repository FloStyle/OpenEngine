//! Real editor **Move tool** as a [`Plugin`] (Phase 3 gizmo-as-plugin proof).
//!
//! This is the second "real tool behind the boundary" demonstration (the first
//! was the Rotate tool in `tests/tool_plugin.rs`). It migrates the editor's
//! ground-plane grid Move drag into a reloadable plugin:
//!
//! * The pure math is encapsulated here — [`compute_move_transform`] reuses the
//!   headless editor grid machinery ([`EditorGrid`]) so the snap/ground logic
//!   lives in the plugin, not scattered across the shell.
//! * [`MoveToolPlugin`] provides the full [`Plugin`] lifecycle
//!   (`init`/`update`/`deinit`) so a host can load, drive and unload it.
//!
//! # Intended separation (see also the inline `TODO` below)
//!
//! ```text
//! Plugin (this file):
//!   - owns the REAL move math (EditorGrid snap + ground point)
//!   - exposes compute_move_transform(...)
//! Shell (editor-shell/src/app.rs handle_edit_drag):
//!   - keeps UI interaction (pointer -> ground point, drag state, edit world)
//!   - CURRENTLY uses inline move_actor_on_ground math (the gap)
//!   - FUTURE: calls MoveToolPlugin::compute_move_transform instead
//! ```
//!
//! The plugin boundary deliberately passes only [`FrameCtx`] (ADR-0005: no
//! editor/egui/world in the boundary), so `update` here only demonstrates the
//! lifecycle by advancing a synthetic drag. The *typed* drag input (camera,
//! pointer, selected entity, edit world) stays host-owned in the shell.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use crate::{FrameCtx, Plugin};
use openengine_editor::grid::EditorGrid;

/// Snap a desired ground target and produce the actor's next position.
///
/// This is the callable heart of the Move tool: given the actor's *current*
/// world position and a *desired* ground-plane point (already grab-offset
/// corrected by the caller), it snaps the target XZ to the grid and returns the
/// new world position, preserving the actor's height (ground drag keeps `y`).
///
/// `current` and the returned value are Domain-A floats exactly like the rest
/// of the headless editor math; callers convert to fixed-point at the world
/// boundary. Returns the snapped position.
pub fn compute_move_transform(
    current: [f32; 3],
    target_ground: [f32; 2],
    grid: &EditorGrid,
) -> [f32; 3] {
    let snapped = grid.snap_xz([target_ground[0], current[1], target_ground[1]]);
    [snapped[0], snapped[1], snapped[2]]
}

/// A reloadable Move-tool plugin.
///
/// Lifecycle: [`Plugin::init`] is a no-op (the tool has no setup), each
/// [`Plugin::update`] advances a synthetic ground drag through
/// [`compute_move_transform`] so the real move math demonstrably runs behind
/// the boundary, and [`Plugin::deinit`] clears state.
///
/// The `applied` counter lets a headless test observe that the tool really
/// executed `N` frames before being unloaded (same pattern as the Rotate tool).
pub struct MoveToolPlugin {
    /// Grid the Move tool snaps to (host-owned, set at construction).
    grid: EditorGrid,
    /// Synthetic drag state: actor's current XZ on the ground.
    actor_xz: [f32; 2],
    /// Host-observed count of move applications (for lifecycle tests).
    applied: Arc<AtomicU32>,
}

impl MoveToolPlugin {
    /// A Move tool with an initial actor at `start_xz` and the given snap step.
    ///
    /// `applied` is shared with the host/test so it can assert how many frames
    /// the tool actually ran.
    pub fn new(grid_step: f32, start_xz: [f32; 2], applied: Arc<AtomicU32>) -> Self {
        MoveToolPlugin {
            grid: EditorGrid { step: grid_step },
            actor_xz: start_xz,
            applied,
        }
    }

    /// Current actor XZ the tool is dragging (for introspection in tests).
    pub fn actor_xz(&self) -> [f32; 2] {
        self.actor_xz
    }
}

impl Plugin for MoveToolPlugin {
    fn id(&self) -> &'static str {
        "move-tool"
    }

    fn init(&mut self, _frame: &FrameCtx) -> Result<(), String> {
        Ok(())
    }

    // TODO: Shell currently uses inline move_actor_on_ground math (app.rs
    // handle_edit_drag). Future work: replace the shell's inline call with:
    //   let new_pos = plugin.compute_move_transform(current, target, grid);
    // This gap is intentional — the plugin system is proven here, but rewiring
    // the live shell drag requires visual validation (headless cannot confirm
    // drag feel). ADR-0005 is intentionally unchanged.
    fn update(&mut self, _frame: &FrameCtx) -> Result<(), String> {
        // Synthetic drag: push the ground target +1 grid step on X each frame,
        // then route it through the real move math so the tool's update is
        // genuinely doing Move work, not a no-op.
        let target_ground = [self.actor_xz[0] + self.grid.step, self.actor_xz[1]];
        let next = compute_move_transform(
            [self.actor_xz[0], 0.0, self.actor_xz[1]],
            target_ground,
            &self.grid,
        );
        self.actor_xz = [next[0], next[2]];
        self.applied.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn deinit(&mut self) {
        self.actor_xz = [0.0, 0.0];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_move_transform_snaps_to_grid_and_keeps_y() {
        let grid = EditorGrid { step: 1.0 };
        let cur = [0.2, 0.0, 0.3];
        let out = compute_move_transform(cur, [0.7, 0.9], &grid);
        // Target XZ snapped to the 1.0 grid: 0.7 -> 1.0, 0.9 -> 1.0; y kept.
        assert_eq!(out, [1.0, 0.0, 1.0]);
    }

    #[test]
    fn move_tool_lifecycle_load_update_unload() {
        let applied = Arc::new(AtomicU32::new(0));
        let mut tool = MoveToolPlugin::new(0.5, [0.0, 0.0], applied.clone());
        let mut f = FrameCtx { frame: 0, dt: 0.0 };
        tool.init(&f).unwrap();
        for _ in 0..4 {
            f.frame += 1;
            tool.update(&f).unwrap();
        }
        assert_eq!(applied.load(Ordering::Relaxed), 4);
        // After 4 drags of 0.5 grid step from the origin: XZ snapped to (2.0,0).
        assert_eq!(tool.actor_xz(), [2.0, 0.0]);
        tool.deinit();
        assert_eq!(tool.actor_xz(), [0.0, 0.0]);
    }
}

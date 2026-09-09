//! # Plugin boundary (Phase 3 — "everything is a plugin except the Anvil").
//!
//! A minimal in-process plugin lifecycle: a [`Plugin`] exposes `id`/`init`/
//! `update`/`deinit` and a [`PluginHost`] loads them, initializes them, drives
//! them, and can unload one (calling `deinit`). Tools/UI (e.g. the gizmo) are
//! meant to live behind this boundary so they are reloadable/extendable; native
//! dylib reload transport is a **design** (ADR-0005), not implemented here.
//!
//! No `Any`/downcast is used: a plugin that needs a host service receives it as
//! plain typed arguments at `init`/`update` (the registry is the host's job, per
//! ADR-0005), keeping plugin code concrete and predictable.

/// In-crate editor tool plugins (real tools behind the boundary, e.g. the Move
/// gizmo). Hosts may construct and register these.
pub mod plugins {
    /// The real editor Move tool as a plugin (grid ground-drag math + lifecycle).
    pub mod move_tool;
}

/// Shared per-frame context a plugin may read (host-owned, typed).
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameCtx {
    /// Deterministic host frame counter.
    pub frame: u64,
    /// Delta seconds since the last update (Domain-A pacing only).
    pub dt: f32,
}

/// A reloadable tool/feature behind the plugin boundary.
pub trait Plugin: Send {
    /// Stable, unique id (used to add/unload by name).
    fn id(&self) -> &'static str;

    /// Called once after load, before any `update`.
    fn init(&mut self, _frame: &FrameCtx) -> Result<(), String> {
        Ok(())
    }

    /// Called each host frame.
    fn update(&mut self, _frame: &FrameCtx) -> Result<(), String> {
        Ok(())
    }

    /// Called when the plugin is unloaded (reverse any effects).
    fn deinit(&mut self) {}
}

/// Owns and drives a set of plugins.
pub struct PluginHost {
    plugins: Vec<Box<dyn Plugin>>,
}

impl Default for PluginHost {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginHost {
    /// An empty host.
    pub fn new() -> Self {
        PluginHost {
            plugins: Vec::new(),
        }
    }

    /// Number of loaded plugins.
    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    /// True when no plugin is loaded.
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Registered plugin ids (stable order).
    pub fn ids(&self) -> Vec<&'static str> {
        self.plugins.iter().map(|p| p.id()).collect()
    }

    /// Load + initialize a plugin. Rejects a duplicate id.
    pub fn add(&mut self, plugin: Box<dyn Plugin>, frame: &FrameCtx) -> Result<(), String> {
        if self.plugins.iter().any(|p| p.id() == plugin.id()) {
            return Err(format!("plugin '{}' already loaded", plugin.id()));
        }
        let mut p = plugin;
        p.init(frame)?;
        self.plugins.push(p);
        Ok(())
    }

    /// Drive every plugin for one frame.
    pub fn update_all(&mut self, frame: &FrameCtx) -> Result<(), String> {
        for p in self.plugins.iter_mut() {
            p.update(frame)?;
        }
        Ok(())
    }

    /// Unload a plugin by id (calls its `deinit`), then remove it.
    pub fn unload(&mut self, id: &str) -> bool {
        if let Some(pos) = self.plugins.iter().position(|p| p.id() == id) {
            let mut p = self.plugins.remove(pos);
            p.deinit();
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct CounterPlugin {
        inits: u32,
        updates: u32,
        deinits: u32,
        err_after: u32,
    }
    impl Plugin for CounterPlugin {
        fn id(&self) -> &'static str {
            "counter"
        }
        fn init(&mut self, _: &FrameCtx) -> Result<(), String> {
            self.inits += 1;
            Ok(())
        }
        fn update(&mut self, f: &FrameCtx) -> Result<(), String> {
            self.updates += 1;
            if self.updates == self.err_after {
                return Err("boom".into());
            }
            let _ = f.frame;
            Ok(())
        }
        fn deinit(&mut self) {
            self.deinits += 1;
        }
    }

    #[test]
    fn lifecycle_load_update_unload() {
        let mut host = PluginHost::new();
        let mut f = FrameCtx { frame: 0, dt: 0.0 };
        host.add(Box::new(CounterPlugin::default()), &f).unwrap();
        assert_eq!(host.ids(), vec!["counter"]);
        f.frame += 1;
        host.update_all(&f).unwrap();
        assert_eq!(host.len(), 1);
        assert!(host.unload("counter"));
        assert!(host.is_empty());
    }

    #[test]
    fn duplicate_id_is_rejected() {
        let mut host = PluginHost::new();
        let f = FrameCtx::default();
        host.add(Box::new(CounterPlugin::default()), &f).unwrap();
        assert!(host.add(Box::new(CounterPlugin::default()), &f).is_err());
    }

    #[test]
    fn update_error_propagates() {
        let mut host = PluginHost::new();
        let f = FrameCtx::default();
        let p = CounterPlugin {
            err_after: 1,
            ..Default::default()
        };
        host.add(Box::new(p), &f).unwrap();
        let f2 = FrameCtx { frame: 1, dt: 0.0 };
        assert!(host.update_all(&f2).is_err());
    }
}

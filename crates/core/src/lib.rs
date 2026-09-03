//! # Domain A — `openengine-core` (host runtime)
//!
//! The only domain that may touch `std`, the GPU, threads, and the outside
//! world. It owns the **job system**, the **wgpu(Vulkan) renderer**, the
//! **winit** event loop, and the **wasmtime** sandbox that drives Domain B.
//!
//! ## Scaffold state
//! This crate is intentionally a skeleton. Its job is to pin the dependency
//! graph (see `Cargo.toml`) and to name the module boundaries so future agents
//! have unambiguous homes for their code. The concrete calls into `wgpu`,
//! `wasmtime` and `winit` arrive with the renderer/sandbox milestone; the
//! stubs below deliberately avoid referencing volatile upstream API signatures
//! so this crate does not rot between dependency bumps.
//!
//! ## Domain discipline
//! * You may rely on `unsafe` *only* inside third-party crates. This crate's
//!   own source inherits `forbid(unsafe_code)`. A zero-copy bridge that needs a
//!   raw pointer moves into one reviewed `unsafe_impl` module (RFC, AGENTS.md).
//! * Gameplay values crossing into Domain B go through
//!   `openengine-math::quantize_to_f32` — logic never receives raw `f32` as
//!   truth.
//! * Every tick: build a `StateView` → drive the Wasm logic → collect its
//!   `WorldDelta` → apply in the ECS worker → schedule render. Pure separation.

#![deny(missing_docs)]

pub mod jobs {
    //! Job graph: builds the tick pipeline (pure-system fan-out + delta merge)
    //! over the Rayon pool.

    /// Runs a closure on the shared rayon pool. Skeleton; the real scheduler
    /// (with determinism-aware priorities) arrives in the Job System milestone.
    pub fn spawn<F>(f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        rayon::spawn(f);
    }
}

pub mod renderer {
    //! GPU output. Holds the `wgpu::Instance`/`Device`, the swapchain, and the
    //! draw dispatcher. No implementation yet — declared here so Domain A has a
    //! canonical home for the milestone.

    /// Marker for a renderer that has not been initialized. Placeholder.
    #[derive(Default)]
    pub struct Renderer;

    impl Renderer {
        /// Returns the current logical pixel scale (placeholder, 1.0).
        pub fn pixel_scale(&self) -> f32 {
            1.0
        }
    }
}

pub mod sandbox {
    //! Instantiates `logic-sandbox` Wasm modules and drives their pure systems.
    //!
    //! The milestone must: (1) pre-open a shared memory region, (2) map the SoA
    //! arena descriptors into the guest, (3) call the guest system, (4) pull the
    //! serialized `WorldDelta` back and apply it zero-copy in the ECS.

    /// Policy knobs for instantiating one logic module. Placeholder.
    #[derive(Default)]
    pub struct SandboxConfig {
        /// Max guest instructions per system call (determinism budget).
        pub fuel: u64,
        /// Max `WorldDelta` bytes accepted back from the guest.
        pub delta_budget_bytes: usize,
    }
}

pub mod platform {
    //! `winit` event-loop integration. No implementation yet.

    /// A handle to the OS window currently being driven. Placeholder.
    pub struct WindowHandle;
}

/// Native (Domain A) movement demo — PoC Phase B (single mutation channel).
pub mod native_movement;

/// Wasm SoA movement host — PoC Phase 3 (ADR-0001 bridge).
pub mod wasm_move_host;

/// Cross-crate ABI handshake used by tests and CI. The host refuses to boot on
/// a mismatch between the linked `ARCH_VERSION` and the current ABI constant.
pub fn abi_is_current() -> bool {
    openengine_contracts::ARCH_VERSION == 2
}

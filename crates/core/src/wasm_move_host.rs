//! Wasm SoA movement host — PoC Phase 3/4 (ADR-0001 bridge).
//!
//! Drives the guest movement export `openengine_move_tick`:
//! 1. serialize `[postcard PlayerInput][postcard columns][column arena]`,
//! 2. write it into a guest-allocated input buffer (`openengine_alloc`),
//! 3. call `openengine_move_tick(input, len, out, cap)`,
//! 4. read + decode the returned `WorldDelta`.
//!
//! Player input is pure DATA (Phase D) — the host never writes velocity.

use anyhow::Context;
use openengine_contracts::{comp, ColumnDescriptor, ComponentId, PlayerInput, WorldDelta};
use openengine_ecs::{Position, Velocity, World};
use wasmtime::{Engine, Instance, Linker, Memory, Module, Store, TypedFunc};

const INPUT_CAP: u32 = 1 << 16;
const OUTPUT_CAP: u32 = 1 << 16;

/// Host driver for the guest movement module.
pub struct WasmMoveHost {
    _engine: wasmtime::Engine,
    store: Store<()>,
    tick: TypedFunc<(u32, u32, u32, u32), u32>,
    memory: Memory,
    /// Guest-allocated input buffer address (leaked once, reused every tick).
    input_ptr: u32,
    /// Guest-allocated output buffer address (leaked once, reused every tick).
    output_ptr: u32,
    /// Current pure player input passed as data to the guest.
    input: PlayerInput,
}

impl WasmMoveHost {
    /// Load a logic module exposing `openengine_alloc` + `openengine_move_tick`.
    pub fn load(wasm_path: &str) -> anyhow::Result<Self> {
        let engine = Engine::default();
        let module = Module::from_file(&engine, wasm_path).context("load wasm module")?;
        let mut store = Store::new(&engine, ());
        let linker = Linker::new(&engine);
        let instance: Instance = linker
            .instantiate(&mut store, &module)
            .context("instantiate logic module")?;

        let alloc = instance
            .get_typed_func::<u32, u32>(&mut store, "openengine_alloc")
            .context("missing export openengine_alloc")?;
        let tick = instance
            .get_typed_func::<(u32, u32, u32, u32), u32>(&mut store, "openengine_move_tick")
            .context("missing export openengine_move_tick")?;
        let memory = instance
            .get_memory(&mut store, "memory")
            .context("guest must export linear memory as 'memory'")?;

        // Allocate the two fixed transport buffers ONCE (leaked in the guest).
        let input_ptr = alloc.call(&mut store, INPUT_CAP)?;
        let output_ptr = alloc.call(&mut store, OUTPUT_CAP)?;

        Ok(WasmMoveHost {
            _engine: engine,
            store,
            tick,
            memory,
            input_ptr,
            output_ptr,
            input: PlayerInput::none(),
        })
    }

    /// Set the pure player input sent to the guest on the next tick.
    pub fn set_input(&mut self, input: PlayerInput) {
        self.input = input;
    }

    /// Run one guest movement tick over `world` and return its `WorldDelta`.
    pub fn tick(&mut self, world: &World) -> anyhow::Result<WorldDelta> {
        let n = world.entity_count();
        let positions = world.get_positions().unwrap_or(&[]);
        let velocities = world.get_velocities().unwrap_or(&[]);

        let pos_bytes: &[u8] = bytemuck::cast_slice(&positions[..n]);
        let vel_bytes: &[u8] = bytemuck::cast_slice(&velocities[..n]);
        let columns = vec![
            ColumnDescriptor {
                component_id: ComponentId(comp::POSITION),
                element_size: core::mem::size_of::<Position>() as u32,
                count: n as u32,
                data_offset: 0,
            },
            ColumnDescriptor {
                component_id: ComponentId(comp::VELOCITY),
                element_size: core::mem::size_of::<Velocity>() as u32,
                count: n as u32,
                data_offset: pos_bytes.len() as u32,
            },
        ];
        let input_bytes = postcard::to_allocvec(&self.input).context("encode player input")?;
        let cols_bytes = postcard::to_allocvec(&columns).context("encode columns")?;

        let mut input = Vec::with_capacity(
            input_bytes.len() + cols_bytes.len() + pos_bytes.len() + vel_bytes.len(),
        );
        input.extend_from_slice(&input_bytes);
        input.extend_from_slice(&cols_bytes);
        input.extend_from_slice(pos_bytes);
        input.extend_from_slice(vel_bytes);
        if input.len() as u32 > INPUT_CAP {
            anyhow::bail!("input exceeds guest buffer");
        }

        self.memory
            .write(&mut self.store, self.input_ptr as usize, &input)
            .context("write input into guest buffer")?;

        let out_len = self
            .tick
            .call(
                &mut self.store,
                (
                    self.input_ptr,
                    input.len() as u32,
                    self.output_ptr,
                    OUTPUT_CAP,
                ),
            )
            .context("guest movement tick")?;
        if out_len == 0 || out_len as usize > OUTPUT_CAP as usize {
            anyhow::bail!("guest movement tick returned invalid length ({out_len})");
        }

        let mut out = vec![0u8; out_len as usize];
        self.memory
            .read(&self.store, self.output_ptr as usize, &mut out)
            .context("read WorldDelta from guest memory")?;
        openengine_contracts::decode_delta(&out).context("decode WorldDelta")
    }
}

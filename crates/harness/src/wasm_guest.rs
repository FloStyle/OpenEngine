//! Minimal wasm guest driver for the harness — a small, headless mirror of
//! `openengine-core::wasm_gameplay_host` (kept here so the harness never pulls
//! wgpu/winit). Drives `openengine_gameplay_tick`:
//! `[frame u64 LE][GameplayInputWire (16)][postcard columns][Transform |
//! Velocity3D | Actor arena]` → returns an encoded `WorldDelta`.

use anyhow::Context;
use openengine_contracts::{
    comp, Actor, ColumnDescriptor, ComponentId, GameplayInputWire, InputState3D, Transform,
    Velocity3D, WorldDelta,
};
use openengine_ecs::World;
use wasmtime::{Engine, Instance, Linker, Memory, Module, Store, TypedFunc};

const INPUT_CAP: u32 = 1 << 16;
const OUTPUT_CAP: u32 = 1 << 16;

/// Host driver for the guest gameplay module.
pub struct WasmGuest {
    _engine: wasmtime::Engine,
    store: Store<()>,
    tick: TypedFunc<(u32, u32, u32, u32), u32>,
    memory: Memory,
    input_ptr: u32,
    output_ptr: u32,
    input: InputState3D,
}

impl WasmGuest {
    /// Load a module exposing `openengine_alloc` + `openengine_gameplay_tick`.
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
            .get_typed_func::<(u32, u32, u32, u32), u32>(&mut store, "openengine_gameplay_tick")
            .context("missing export openengine_gameplay_tick")?;
        let memory = instance
            .get_memory(&mut store, "memory")
            .context("guest must export linear memory as 'memory'")?;
        let input_ptr = alloc.call(&mut store, INPUT_CAP)?;
        let output_ptr = alloc.call(&mut store, OUTPUT_CAP)?;
        Ok(WasmGuest {
            _engine: engine,
            store,
            tick,
            memory,
            input_ptr,
            output_ptr,
            input: InputState3D::none(),
        })
    }

    /// Set the (pure data) input for the next tick.
    pub fn set_input(&mut self, input: InputState3D) {
        self.input = input;
    }

    /// Run one guest gameplay tick at `frame` and return its `WorldDelta`.
    pub fn tick(&mut self, world: &World, frame: u64) -> anyhow::Result<WorldDelta> {
        let n = world.entity_count();
        let transforms = world.get_transforms().unwrap_or(&[]);
        let velocities = world.get_velocity_3d().unwrap_or(&[]);
        let actors = world.get_actors().unwrap_or(&[]);

        let t_bytes: &[u8] = bytemuck::cast_slice(&transforms[..n]);
        let v_bytes: &[u8] = bytemuck::cast_slice(&velocities[..n]);
        let a_bytes: &[u8] = bytemuck::cast_slice(&actors[..n]);
        let t_off = 0;
        let v_off = t_bytes.len();
        let a_off = v_off + v_bytes.len();
        let columns = vec![
            ColumnDescriptor {
                component_id: ComponentId(comp::TRANSFORM),
                element_size: core::mem::size_of::<Transform>() as u32,
                count: n as u32,
                data_offset: t_off as u32,
            },
            ColumnDescriptor {
                component_id: ComponentId(comp::VELOCITY3D),
                element_size: core::mem::size_of::<Velocity3D>() as u32,
                count: n as u32,
                data_offset: v_off as u32,
            },
            ColumnDescriptor {
                component_id: ComponentId(comp::ACTOR),
                element_size: core::mem::size_of::<Actor>() as u32,
                count: n as u32,
                data_offset: a_off as u32,
            },
        ];
        let cols_bytes = postcard::to_allocvec(&columns).context("encode columns")?;
        let mut input = Vec::with_capacity(
            24 + cols_bytes.len() + t_bytes.len() + v_bytes.len() + a_bytes.len(),
        );
        input.extend_from_slice(&frame.to_le_bytes());
        input.extend_from_slice(bytemuck::bytes_of(&GameplayInputWire::from(self.input)));
        input.extend_from_slice(&cols_bytes);
        input.extend_from_slice(t_bytes);
        input.extend_from_slice(v_bytes);
        input.extend_from_slice(a_bytes);
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
            .context("guest gameplay tick")?;
        if out_len == 0 || out_len as usize > OUTPUT_CAP as usize {
            anyhow::bail!("guest gameplay tick returned invalid length ({out_len})");
        }
        let mut out = vec![0u8; out_len as usize];
        self.memory
            .read(&self.store, self.output_ptr as usize, &mut out)
            .context("read WorldDelta from guest memory")?;
        openengine_contracts::decode_delta(&out).context("decode WorldDelta")
    }
}

//! Wasm physics host — drives `openengine_physics_tick` (Domain-B physics in the
//! guest). Mirrors the other host drivers: builds the fixed params header +
//! SoA columns, writes into a guest-allocated buffer, runs one tick, decodes the
//! returned WorldDelta.

use anyhow::Context;
use openengine_contracts::{
    comp, ColumnDescriptor, ComponentId, Transform, Velocity3D, WorldDelta,
};
use openengine_ecs::World;
use openengine_math::I16F16 as F;
use wasmtime::{Engine, Instance, Linker, Memory, Module, Store, TypedFunc};

const INPUT_CAP: u32 = 1 << 16;
const OUTPUT_CAP: u32 = 1 << 16;

/// Physics integrator parameters passed to the guest.
pub struct PhysicsParams {
    /// Gravity per tick (signed).
    pub gravity: f32,
    /// Floor Y (integer world units).
    pub floor: i32,
    /// AABB half-extents [x, y, z].
    pub half: [f32; 3],
}

/// Host driver for the guest physics module (`openengine_physics_tick`).
pub struct WasmPhysicsHost {
    _engine: wasmtime::Engine,
    store: Store<()>,
    tick: TypedFunc<(u32, u32, u32, u32), u32>,
    memory: Memory,
    input_ptr: u32,
    output_ptr: u32,
}

impl WasmPhysicsHost {
    /// Load a logic module exposing `openengine_alloc` + `openengine_physics_tick`.
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
            .context("missing openengine_alloc")?;
        let tick = instance
            .get_typed_func::<(u32, u32, u32, u32), u32>(&mut store, "openengine_physics_tick")
            .context("missing openengine_physics_tick")?;
        let memory = instance
            .get_memory(&mut store, "memory")
            .context("guest must export 'memory'")?;
        let input_ptr = alloc.call(&mut store, INPUT_CAP)?;
        let output_ptr = alloc.call(&mut store, OUTPUT_CAP)?;
        Ok(WasmPhysicsHost {
            _engine: engine,
            store,
            tick,
            memory,
            input_ptr,
            output_ptr,
        })
    }

    /// Run one physics tick over `world`, returning its WorldDelta.
    pub fn tick(&mut self, world: &World, p: &PhysicsParams) -> anyhow::Result<WorldDelta> {
        let n = world.entity_count();
        let transforms = world.get_transforms().unwrap_or(&[]);
        let velocities = world.get_velocity_3d().unwrap_or(&[]);
        let t_bytes: &[u8] = bytemuck::cast_slice(&transforms[..n]);
        let v_bytes: &[u8] = bytemuck::cast_slice(&velocities[..n]);
        let columns = vec![
            ColumnDescriptor {
                component_id: ComponentId(comp::TRANSFORM),
                element_size: core::mem::size_of::<Transform>() as u32,
                count: n as u32,
                data_offset: 0,
            },
            ColumnDescriptor {
                component_id: ComponentId(comp::VELOCITY3D),
                element_size: core::mem::size_of::<Velocity3D>() as u32,
                count: n as u32,
                data_offset: t_bytes.len() as u32,
            },
        ];
        let cols_bytes = postcard::to_allocvec(&columns).context("encode columns")?;
        let mut input = Vec::with_capacity(20 + cols_bytes.len() + t_bytes.len() + v_bytes.len());
        // Fixed header: gravity, floor, halfX/Y/Z as i32 fixed bits.
        input.extend_from_slice(&F::from_num(p.gravity).to_bits().to_le_bytes());
        input.extend_from_slice(&p.floor.to_le_bytes());
        for h in p.half {
            input.extend_from_slice(&F::from_num(h).to_bits().to_le_bytes());
        }
        input.extend_from_slice(&cols_bytes);
        input.extend_from_slice(t_bytes);
        input.extend_from_slice(v_bytes);
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
            .context("guest physics tick")?;
        if out_len == 0 || out_len as usize > OUTPUT_CAP as usize {
            anyhow::bail!("guest physics tick returned invalid length ({out_len})");
        }
        let mut out = vec![0u8; out_len as usize];
        self.memory
            .read(&self.store, self.output_ptr as usize, &mut out)
            .context("read WorldDelta from guest memory")?;
        openengine_contracts::decode_delta(&out).context("decode WorldDelta")
    }
}

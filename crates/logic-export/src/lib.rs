//! # Domain B — wasm ABI export (`openengine-logic-export`)
//!
//! The pure logic in `openengine-logic-sandbox` is compiled to a *host-callable*
//! wasm `cdylib` through this crate. This file contains **no game logic** and
//! **no `unsafe {}` blocks that touch state** — it is a thin serialization
//! shim that:
//!
//! 1. builds a read-only [`StateView`] from a `tick`,
//! 2. calls the pure system [`openengine_logic_sandbox::tick_color`],
//! 3. `postcard`-encodes the returned [`WorldDelta`] into a host-visible buffer
//!    in guest linear memory.
//!
//! ## Why this crate exists (instead of exporting from the logic crate)
//!
//! `#[no_mangle]` is an "unsafe attribute" and cannot coexist with
//! `#![forbid(unsafe_code)]`, which guards all *logic*. Keeping the raw exports
//! in a dedicated, logic-free shim preserves the safety wall: nothing unsafe
//! can ever touch gameplay math.
//!
//! ## Exported C ABI (the only surface wasmtime sees)
//! * `openengine_alloc(len: u32) -> u32` — reserve a fixed guest buffer.
//! * `openengine_tick(tick: u64, out_ptr: u32, out_cap: u32) -> u32` — run the
//!   pure system and write the encoded [`WorldDelta`] into `out_ptr`, returning
//!   the byte length (`0` = failure).

#![cfg_attr(target_arch = "wasm32", no_std)]

extern crate alloc;

use alloc::vec::Vec;
use openengine_contracts::StateView;

// Guest heap allocator for the no_std wasm artifact (wasm-alloc feature only).
#[cfg(all(target_arch = "wasm32", feature = "wasm-alloc"))]
#[global_allocator]
static DLMALLOC: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

/// Zero-cost panic surface for the guest.
#[cfg(all(target_arch = "wasm32", not(test)))]
#[panic_handler]
fn guest_panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

/// Reserve `len` bytes in guest linear memory and return their address.
///
/// The host calls this **once** to obtain a long-lived scratch buffer, then
/// reuses it every frame. The allocation is intentionally leaked (never freed)
/// so the pointer stays valid for the life of the module.
///
/// # Safety
/// Safe: `Vec` handles allocation; `mem::forget` deliberately leaks it so the
/// backing pointer outlives the call. No raw memory is touched here.
#[no_mangle]
pub extern "C" fn openengine_alloc(len: u32) -> u32 {
    let mut buf: Vec<u8> = Vec::with_capacity(len as usize);
    let ptr = buf.as_mut_ptr();
    core::mem::forget(buf); // leak: host owns this buffer for module lifetime
    ptr as u32
}

/// Run the pure `tick_color` system and serialize its [`WorldDelta`] into the
/// guest buffer at `out_ptr` (from [`openengine_alloc`]).
///
/// Returns the number of bytes written, or `0` if the delta could not be
/// produced or does not fit in `out_cap`.
///
/// # Safety
/// `out_ptr`/`out_cap` describe a region of guest linear memory owned by the
/// host (obtained via [`openengine_alloc`]). This crate is the *only* place raw
/// guest-memory writes happen, and they are bounded by `out_cap`. There is no
/// game logic here, only the ABI copy.
#[no_mangle]
pub extern "C" fn openengine_tick(tick: u64, out_ptr: u32, out_cap: u32) -> u32 {
    let view = StateView::tick_only(tick);
    match openengine_logic_sandbox::tick_color(&view) {
        Ok(delta) => {
            match openengine_contracts::encode_delta(&delta) {
                Ok(bytes) => {
                    let n = bytes.len();
                    if n <= out_cap as usize && !bytes.is_empty() {
                        // SAFETY: out_ptr..out_ptr+n is a valid, writable region
                        // of guest memory (host buffer from openengine_alloc) and
                        // n <= out_cap. Copies are length-checked above.
                        unsafe {
                            core::ptr::copy_nonoverlapping(bytes.as_ptr(), out_ptr as *mut u8, n);
                        }
                        n as u32
                    } else {
                        0
                    }
                }
                Err(_) => 0,
            }
        }
        Err(_) => 0,
    }
}

// ────────────────────────────────────────────────────────────────────────────

// ────────────────────────────────────────────────────────────────────────────
// § SoA movement bridge (Phase 3 / ADR-0001)
//
// Transport only. The host writes `[postcard(columns)][column arena]` into a
// guest buffer (via openengine_alloc); this tick reads it back (transport
// `unsafe` is sanctioned in this shim crate — the pure logic in
// logic-sandbox stays `forbid(unsafe_code)`), runs the movement system, and
// writes the encoded WorldDelta into the host output buffer.
// ────────────────────────────────────────────────────────────────────────────

use openengine_contracts::comp;
use openengine_contracts::{
    Actor, ColumnDescriptor, GameplayInputWire, InputState3D, Position, Transform, Velocity,
    Velocity3D,
};

/// Run one movement tick.
///
/// `input_ptr/input_len`: host-written `[postcard PlayerInput][postcard
/// columns][column arena]`. `out_ptr/out_cap`: host output buffer for the
/// encoded WorldDelta. Returns the number of output bytes, or 0 on failure.
///
/// # Safety
/// `input_ptr..+input_len` and `out_ptr..+out_cap` are valid guest linear-memory
/// regions owned by the host (obtained via `openengine_alloc`). Reads and writes
/// are length-checked. This is transport-only; no gameplay logic is here.
#[no_mangle]
pub unsafe extern "C" fn openengine_move_tick(
    input_ptr: u32,
    input_len: u32,
    out_ptr: u32,
    out_cap: u32,
) -> u32 {
    // SAFETY: caller guarantees the input region is readable for input_len.
    let input_bytes: &[u8] =
        unsafe { core::slice::from_raw_parts(input_ptr as *const u8, input_len as usize) };

    // Layout: [postcard PlayerInput][postcard columns][column arena].
    let (player_input, rest) =
        match postcard::take_from_bytes::<openengine_contracts::PlayerInput>(input_bytes) {
            Ok(v) => v,
            Err(_) => return 0,
        };
    let (columns, arena) = match postcard::take_from_bytes::<Vec<ColumnDescriptor>>(rest) {
        Ok(v) => v,
        Err(_) => return 0,
    };

    let positions: Vec<Position> = read_column(&columns, arena, comp::POSITION);
    let velocities: Vec<Velocity> = read_column(&columns, arena, comp::VELOCITY);
    let n = positions.len().min(velocities.len());

    // Run the PURE movement logic (forbid(unsafe_code)) on owned slices, with
    // the player input as data.
    let delta = match openengine_logic_sandbox::movement_system_with_input(
        &positions[..n],
        &velocities[..n],
        &player_input,
    ) {
        Ok(d) => d,
        Err(_) => return 0,
    };
    let bytes = match openengine_contracts::encode_delta(&delta) {
        Ok(b) => b,
        Err(_) => return 0,
    };
    if bytes.len() > out_cap as usize || bytes.is_empty() {
        return 0;
    }
    // SAFETY: out_ptr..out_ptr+n is a writable guest buffer of out_cap bytes.
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), out_ptr as *mut u8, bytes.len());
    }
    bytes.len() as u32
}

/// Run one full 3D gameplay tick (Phase E `gameplay_tick`) in the guest.
///
/// `input_ptr/input_len`: host-written
/// `[frame u64 LE (8)][GameplayInputWire (16)][postcard columns][Transform |
/// Velocity3D | Actor arena]`. `out_ptr/out_cap`: host output buffer for the
/// encoded WorldDelta. The frame counter drives deterministic NPC cadences.
///
/// # Safety
/// `input_ptr..+input_len` and `out_ptr..+out_cap` are valid guest linear-memory
/// regions owned by the host (obtained via `openengine_alloc`). Reads and writes
/// are length-checked. Transport-only shim; no gameplay logic is here.
#[no_mangle]
pub unsafe extern "C" fn openengine_gameplay_tick(
    input_ptr: u32,
    input_len: u32,
    out_ptr: u32,
    out_cap: u32,
) -> u32 {
    // SAFETY: caller guarantees the input region is readable for input_len.
    let input_bytes: &[u8] =
        unsafe { core::slice::from_raw_parts(input_ptr as *const u8, input_len as usize) };

    // Fixed header: frame (u64 LE) + GameplayInputWire, then columns + arena.
    let core = 8 + core::mem::size_of::<GameplayInputWire>();
    if input_bytes.len() < core {
        return 0;
    }
    let frame = u64::from_le_bytes(input_bytes[0..8].try_into().unwrap());
    let wire: GameplayInputWire = bytemuck::pod_read_unaligned(
        &input_bytes[8..8 + core::mem::size_of::<GameplayInputWire>()],
    );
    let game_input = InputState3D::from(wire);
    let (columns, arena) =
        match postcard::take_from_bytes::<Vec<ColumnDescriptor>>(&input_bytes[core..]) {
            Ok(v) => v,
            Err(_) => return 0,
        };

    let transforms: Vec<Transform> = read_column(&columns, arena, comp::TRANSFORM);
    let velocities: Vec<Velocity3D> = read_column(&columns, arena, comp::VELOCITY3D);
    let actors: Vec<Actor> = read_column(&columns, arena, comp::ACTOR);
    let n = transforms.len().min(velocities.len()).min(actors.len());

    let delta = match openengine_logic_sandbox::gameplay_tick(
        frame,
        &transforms[..n],
        &velocities[..n],
        &actors[..n],
        &game_input,
    ) {
        Ok(d) => d,
        Err(_) => return 0,
    };
    let bytes = match openengine_contracts::encode_delta(&delta) {
        Ok(b) => b,
        Err(_) => return 0,
    };
    if bytes.len() > out_cap as usize || bytes.is_empty() {
        return 0;
    }
    // SAFETY: out_ptr..out_ptr+n is a writable guest buffer of out_cap bytes.
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), out_ptr as *mut u8, bytes.len());
    }
    bytes.len() as u32
}

/// Read a column (described by `descriptors`) out of `arena` into an owned
/// `Vec` using unaligned-safe pod reads (alignment of the arena start is not
/// guaranteed after a postcard length prefix).
fn read_column<T: bytemuck::Pod>(
    descriptors: &[ColumnDescriptor],
    arena: &[u8],
    component_id: u32,
) -> Vec<T> {
    let column = match descriptors
        .iter()
        .find(|c| c.component_id.0 == component_id)
    {
        Some(c) => c,
        None => return Vec::new(),
    };
    let start = column.data_offset as usize;
    let end = start + (column.count as usize * column.element_size as usize);
    if end > arena.len() {
        return Vec::new();
    }
    let bytes = &arena[start..end];
    let es = core::mem::size_of::<T>();
    let mut out = Vec::with_capacity(bytes.len() / es);
    let mut off = 0usize;
    while off + es <= bytes.len() {
        out.push(bytemuck::pod_read_unaligned::<T>(&bytes[off..off + es]));
        off += es;
    }
    out
}

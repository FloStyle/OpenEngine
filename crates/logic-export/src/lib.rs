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

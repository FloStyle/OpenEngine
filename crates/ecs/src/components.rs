//! Primitive components used by the PoC, with spec-21 ComponentIds.
//!
//! Fixed-point only (`openengine-math::I16F16`), `#[repr(C)] Pod + Zeroable`.

use bytemuck::{Pod, Zeroable};
use openengine_math::I16F16;

/// Position (2D) — spec-21 id 0.
pub const POSITION: u32 = 0;
/// Velocity (2D) — spec-21 id 1.
pub const VELOCITY: u32 = 1;
/// Color — new engine component (spec-21 engine band, appended at 72).
pub const COLOR: u32 = 72;

/// 2D position, fixed-point.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Pod, Zeroable)]
pub struct Position {
    /// X coordinate.
    pub x: I16F16,
    /// Y coordinate.
    pub y: I16F16,
}

/// 2D velocity, fixed-point.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Pod, Zeroable)]
pub struct Velocity {
    /// X velocity.
    pub x: I16F16,
    /// Y velocity.
    pub y: I16F16,
}

/// RGBA color for the PoC squares.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Pod, Zeroable)]
pub struct Color {
    /// Red channel.
    pub r: u8,
    /// Green channel.
    pub g: u8,
    /// Blue channel.
    pub b: u8,
    /// Alpha channel.
    pub a: u8,
}

impl Color {
    /// Opaque white.
    pub const WHITE: Color = Color {
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    };
}

// OpenEngine editor viewport — Blender-style lit scene.
//
// Mode 0: entity rendered as an analytic sphere (unit sphere mesh scaled by
//         the model matrix); lit by a fixed directional light + ambient.
// Mode 1: ground plane; fragment shades a checkered pattern from world XZ,
//         lit by the same light so the squares read as a floor.
//
// All lighting is view-independent (diffuse + ambient only) so no eye pos or
// specular term is needed — keeps the Frame uniform to a single matrix.

struct Frame {
    view_proj: mat4x4<f32>,
};
@group(0) @binding(0)
var<uniform> frame: Frame;

struct Object {
    model: mat4x4<f32>,
    color: vec4<f32>,  // base color (rgb); alpha unused (opaque)
    mode: f32,         // 0 = sphere entity, 1 = checkered ground
};
@group(1) @binding(0)
var<uniform> object: Object;

const GROUND_MODE: f32 = 1.0;
// Direction the light rays travel toward the scene (points down and to one side).
const LIGHT_DIR: vec3<f32> = vec3<f32>(0.45, 1.0, 0.3);
const AMBIENT: f32 = 0.22;
const DIFFUSE: f32 = 0.85;
// Checker cell half-size (world units) + two floor shades.
const CELL: f32 = 1.0;
const SHADE_A: vec3<f32> = vec3<f32>(0.30, 0.32, 0.36);
const SHADE_B: vec3<f32> = vec3<f32>(0.72, 0.74, 0.78);

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
};
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let world = object.model * vec4<f32>(in.position, 1.0);
    out.clip = frame.view_proj * world;
    out.world = world.xyz;
    // Model carries only translation + uniform scale here, so the local normal
    // maps cleanly to world space (translation has no effect on direction).
    out.normal = in.normal;
    return out;
}

// Deterministic checker from world XZ coordinates.
fn checker(v: vec2<f32>) -> f32 {
    let c = floor(v.x / CELL) + floor(v.y / CELL);
    return fract(c * 0.5) * 2.0; // 0 or 1 pattern
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let n = normalize(in.normal);
    let ndl = max(dot(n, normalize(LIGHT_DIR)), 0.0);
    let light = AMBIENT + DIFFUSE * ndl;

    var base: vec3<f32> = object.color.rgb;
    if object.mode == GROUND_MODE {
        let c = checker(in.world.xz);
        base = mix(SHADE_A, SHADE_B, c);
    }
    return vec4<f32>(base * light, 1.0);
}

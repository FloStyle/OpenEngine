// OpenEngine editor viewport — solid unlit cubes.

struct Frame {
    view_proj: mat4x4<f32>,
};
@group(0) @binding(0)
var<uniform> frame: Frame;

struct Object {
    model: mat4x4<f32>,
    color: vec4<f32>,
};
@group(1) @binding(0)
var<uniform> object: Object;

struct VsIn {
    @location(0) position: vec3<f32>,
};
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let world = object.model * vec4<f32>(in.position, 1.0);
    out.clip = frame.view_proj * world;
    out.color = object.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}

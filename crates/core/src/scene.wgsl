// OpenEngine demoscene 3D shaders (Domain A, WGSL).

struct Frame {
    view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    light_dir: vec4<f32>, // direction light travels (world); negate for L
};

@group(0) @binding(0)
var<uniform> frame: Frame;

struct Object {
    model: mat4x4<f32>,
    base_color: vec4<f32>, // rgb, a
    params: vec4<f32>,     // x=metallic, y=roughness, z=emissive, w=unused
};

@group(1) @binding(0)
var<uniform> object: Object;

// ---------------- solid (lit) ----------------
struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
};

@vertex
fn vs_solid(in: VsIn) -> VsOut {
    var out: VsOut;
    let world = object.model * vec4<f32>(in.position, 1.0);
    out.clip = frame.view_proj * world;
    out.world_pos = world.xyz;
    out.normal = (object.model * vec4<f32>(in.normal, 0.0)).xyz;
    return out;
}

fn env_gradient(dir: vec3<f32>) -> vec3<f32> {
    // Cheap "sky" gradient: zenith blue-ish, horizon warm.
    let t = clamp(dir.y * 0.5 + 0.5, 0.0, 1.0);
    return mix(vec3<f32>(0.35, 0.45, 0.7), vec3<f32>(0.9, 0.85, 0.8), t);
}

@fragment
fn fs_solid(in: VsOut) -> @location(0) vec4<f32> {
    let N = normalize(in.normal);
    let V = normalize(frame.camera_pos.xyz - in.world_pos);
    let L = normalize(-frame.light_dir.xyz);
    let H = normalize(L + V);

    let metallic = object.params.x;
    let roughness = object.params.y;
    let emissive = object.params.z;

    let base = object.base_color.rgb;
    let ndl = max(dot(N, L), 0.0);
    let diffuse = base * ndl * (1.0 - metallic);

    let shininess = mix(8.0, 256.0, 1.0 - roughness);
    let spec = pow(max(dot(N, H), 0.0), shininess);

    let f0 = mix(vec3<f32>(0.04), base, metallic);
    let fresnel = f0 + (1.0 - f0) * pow(1.0 - max(dot(V, H), 0.0), 5.0);

    let R = reflect(-V, N);
    let reflection = env_gradient(R) * fresnel * metallic;

    var color = diffuse + spec * mix(vec3<f32>(1.0), base, metallic)
              + reflection + base * emissive;

    // Reinhard tone map + gamma.
    color = color / (color + vec3<f32>(1.0));
    color = pow(color, vec3<f32>(1.0 / 2.2));

    return vec4<f32>(color, object.base_color.a);
}

// ---------------- grid lines (unlit) ----------------
@vertex
fn vs_line(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = frame.view_proj * (object.model * vec4<f32>(in.position, 1.0));
    out.world_pos = in.position;
    out.normal = in.normal;
    return out;
}

@fragment
fn fs_line(in: VsOut) -> @location(0) vec4<f32> {
    return object.base_color;
}

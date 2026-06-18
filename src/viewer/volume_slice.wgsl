// Orthogonal volume slice planes. Each plane is a quad in world space; the
// fragment shader reconstructs the voxel coordinate from the interpolated world
// position and samples the 3D intensity texture, so planes stay correct under
// any voxel<->world affine (no per-plane texture-coordinate baking).
//
// Each quad also carries its parametric `quad_uv` (0..1 across the plane) and a
// per-plane identity `color`, used to paint a colored border plus a thicker
// "tab" strip along one edge that the user grabs to scrub the slice.

struct SliceUniforms {
    // view_projection * scene_model: world coordinates -> clip space.
    clip_from_world: mat4x4<f32>,
    // world coordinates -> continuous voxel index.
    world_to_voxel: mat4x4<f32>,
    // 1.0 / dimension per axis (xyz), w unused.
    inv_dimensions: vec4<f32>,
    // window.x = display low, window.y = display high; zw unused.
    window: vec4<f32>,
}

@group(0) @binding(0)
var<uniform> slice: SliceUniforms;

@group(1) @binding(0)
var volume_texture: texture_3d<f32>;
@group(1) @binding(1)
var volume_sampler: sampler;

const BORDER: f32 = 0.012;
const TAB: f32 = 0.06;

struct SliceVertexInput {
    @location(0) world_position: vec3<f32>,
    @location(1) quad_uv: vec2<f32>,
    @location(2) color: vec3<f32>,
}

struct SliceVertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) quad_uv: vec2<f32>,
    @location(2) color: vec3<f32>,
}

@vertex
fn slice_vs(input: SliceVertexInput) -> SliceVertexOutput {
    var output: SliceVertexOutput;
    output.clip_position = slice.clip_from_world * vec4<f32>(input.world_position, 1.0);
    output.world_position = input.world_position;
    output.quad_uv = input.quad_uv;
    output.color = input.color;
    return output;
}

@fragment
fn slice_fs(input: SliceVertexOutput) -> @location(0) vec4<f32> {
    let uv = input.quad_uv;

    // The grab tab: a solid colored strip along the v=0 edge.
    if (uv.y < TAB) {
        return vec4<f32>(input.color, 1.0);
    }
    // A thin colored border framing the rest of the plane.
    if (uv.x < BORDER || uv.x > 1.0 - BORDER || uv.y > 1.0 - BORDER) {
        return vec4<f32>(input.color, 1.0);
    }

    let voxel = (slice.world_to_voxel * vec4<f32>(input.world_position, 1.0)).xyz;
    let tex_coord = (voxel + vec3<f32>(0.5)) * slice.inv_dimensions.xyz;

    // Drop fragments that fall outside the sampled volume box.
    if (tex_coord.x < 0.0 || tex_coord.x > 1.0
        || tex_coord.y < 0.0 || tex_coord.y > 1.0
        || tex_coord.z < 0.0 || tex_coord.z > 1.0) {
        discard;
    }

    let value = textureSampleLevel(volume_texture, volume_sampler, tex_coord, 0.0).r;
    let low = slice.window.x;
    let high = slice.window.y;
    let gray = clamp((value - low) / max(high - low, 1e-6), 0.0, 1.0);
    return vec4<f32>(gray, gray, gray, 1.0);
}

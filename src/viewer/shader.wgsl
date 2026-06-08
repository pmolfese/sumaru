struct Uniforms {
    view_projection: mat4x4<f32>,
    model: mat4x4<f32>,
    light_direction: vec4<f32>,
    surface_color: vec4<f32>,
}

@group(0) @binding(0)
var<uniform> uniforms: Uniforms;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) color: vec4<f32>,
}

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    let world_position = uniforms.model * vec4<f32>(input.position, 1.0);
    let world_normal = normalize((uniforms.model * vec4<f32>(input.normal, 0.0)).xyz);

    var output: VertexOutput;
    output.clip_position = uniforms.view_projection * world_position;
    output.normal = world_normal;
    output.color = input.color;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let normal = normalize(input.normal);
    let light = normalize(uniforms.light_direction.xyz);
    let diffuse = abs(dot(normal, light));
    let lit = 0.28 + diffuse * 0.72;

    return vec4<f32>(input.color.rgb * lit, input.color.a);
}

struct OverlayInput {
    @location(0) position: vec2<f32>,
}

struct OverlayOutput {
    @builtin(position) clip_position: vec4<f32>,
}

@vertex
fn overlay_vs(input: OverlayInput) -> OverlayOutput {
    var output: OverlayOutput;
    output.clip_position = vec4<f32>(input.position, 0.0, 1.0);
    return output;
}

@fragment
fn overlay_fs(_input: OverlayOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(0.90, 0.96, 1.0, 0.92);
}

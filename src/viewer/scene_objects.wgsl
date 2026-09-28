struct Uniforms {
    view_projection: mat4x4<f32>,
    model: mat4x4<f32>,
    viewport: vec2<f32>,
    widths: vec2<f32>,
    appearance: vec4<f32>,
}
@group(0) @binding(0) var<uniform> uniforms: Uniforms;
struct SegmentInput { @location(0) start: vec3<f32>, @location(1) end: vec3<f32>, @location(2) color: vec4<f32>, }
struct VertexOutput { @builtin(position) clip_position: vec4<f32>, @location(0) @interpolate(linear) offset_px: f32, @location(1) @interpolate(flat) color: vec4<f32>, }

@vertex
fn vs_main(input: SegmentInput, @builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    let corner = array<vec2<f32>, 6>(vec2<f32>(0.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0))[vertex_index];
    let clip_start = uniforms.view_projection * uniforms.model * vec4<f32>(input.start, 1.0);
    let clip_end = uniforms.view_projection * uniforms.model * vec4<f32>(input.end, 1.0);
    var clip = select(clip_start, clip_end, corner.x > 0.5);
    let half_viewport = max(uniforms.viewport, vec2<f32>(1.0)) * 0.5;
    var direction = vec2<f32>(1.0, 0.0);
    if clip_start.w > 0.0001 && clip_end.w > 0.0001 {
        let delta = (clip_end.xy / clip_end.w - clip_start.xy / clip_start.w) * half_viewport;
        if length(delta) > 0.00001 { direction = normalize(delta); }
    }
    let perpendicular = vec2<f32>(-direction.y, direction.x);
    let half_width = (max(uniforms.widths.x, 0.0) + max(uniforms.widths.y, 0.0)) * 0.5;
    let offset_px = perpendicular * corner.y * half_width;
    clip = vec4<f32>(clip.xy + (offset_px / half_viewport) * clip.w, clip.zw);
    var output: VertexOutput; output.clip_position = clip; output.offset_px = corner.y * half_width; output.color = input.color; return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let half_width = (max(uniforms.widths.x, 0.0) + max(uniforms.widths.y, 0.0)) * 0.5;
    let half_solid = max(uniforms.widths.x, 0.0) * 0.5;
    let feather = max(half_width - half_solid, 0.0001);
    let alpha = clamp((half_width - abs(input.offset_px)) / feather, 0.0, 1.0);
    if alpha <= 0.001 { discard; }
    return vec4<f32>(input.color.rgb, input.color.a * uniforms.appearance.x * alpha);
}

struct NodeInput { @location(0) position: vec3<f32>, @location(1) color: vec4<f32>, }
struct NodeOutput { @builtin(position) clip_position: vec4<f32>, @location(0) local: vec2<f32>, @location(1) @interpolate(flat) color: vec4<f32>, }

@vertex
fn node_vs(input: NodeInput, @builtin(vertex_index) vertex_index: u32) -> NodeOutput {
    let corner = array<vec2<f32>, 6>(vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, 1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0))[vertex_index];
    var clip = uniforms.view_projection * uniforms.model * vec4<f32>(input.position, 1.0);
    let half_viewport = max(uniforms.viewport, vec2<f32>(1.0)) * 0.5;
    let radius_px = max(uniforms.appearance.y, 1.0) * 0.5;
    clip = vec4<f32>(clip.xy + (corner * radius_px / half_viewport) * clip.w, clip.zw);
    var output: NodeOutput;
    output.clip_position = clip;
    output.local = corner;
    output.color = input.color;
    return output;
}

@fragment
fn node_fs(input: NodeOutput) -> @location(0) vec4<f32> {
    let radius_squared = dot(input.local, input.local);
    if radius_squared > 1.0 { discard; }
    let normal = normalize(vec3<f32>(input.local, sqrt(max(1.0 - radius_squared, 0.0))));
    let light = normalize(vec3<f32>(-0.35, -0.45, 1.0));
    let shade = 0.3 + 0.7 * max(dot(normal, light), 0.0);
    return vec4<f32>(input.color.rgb * shade, input.color.a * uniforms.appearance.x);
}

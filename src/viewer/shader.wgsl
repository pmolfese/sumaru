struct Uniforms {
    view_projection: mat4x4<f32>,
    model: mat4x4<f32>,
    light_direction_primary: vec4<f32>,
    light_direction_secondary: vec4<f32>,
    light_direction_tertiary: vec4<f32>,
    light_weights: vec4<f32>,
    lighting_params: vec4<f32>,
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

struct FlatVertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
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

@vertex
fn flat_vs_main(input: VertexInput) -> FlatVertexOutput {
    let world_position = uniforms.model * vec4<f32>(input.position, 1.0);
    let world_normal = normalize((uniforms.model * vec4<f32>(input.normal, 0.0)).xyz);

    var output: FlatVertexOutput;
    output.clip_position = uniforms.view_projection * world_position;
    output.normal = world_normal;
    output.color = input.color;
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    if input.color.a <= 0.001 {
        discard;
    }
    let normal = normalize(input.normal);
    let primary = normalize(uniforms.light_direction_primary.xyz);
    let secondary = normalize(uniforms.light_direction_secondary.xyz);
    let tertiary = normalize(uniforms.light_direction_tertiary.xyz);
    let diffuse =
        abs(dot(normal, primary)) * uniforms.light_weights.x
        + abs(dot(normal, secondary)) * uniforms.light_weights.y
        + abs(dot(normal, tertiary)) * uniforms.light_weights.z;
    let lit = clamp(uniforms.lighting_params.x + diffuse * uniforms.lighting_params.y, 0.0, 1.0);

    return vec4<f32>(input.color.rgb * lit, input.color.a * uniforms.surface_color.a);
}

@fragment
fn flat_fs_main(input: FlatVertexOutput) -> @location(0) vec4<f32> {
    if input.color.a <= 0.001 {
        discard;
    }
    let normal = normalize(input.normal);
    let primary = normalize(uniforms.light_direction_primary.xyz);
    let secondary = normalize(uniforms.light_direction_secondary.xyz);
    let tertiary = normalize(uniforms.light_direction_tertiary.xyz);
    let diffuse =
        abs(dot(normal, primary)) * uniforms.light_weights.x
        + abs(dot(normal, secondary)) * uniforms.light_weights.y
        + abs(dot(normal, tertiary)) * uniforms.light_weights.z;
    let lit = clamp(uniforms.lighting_params.x + diffuse * uniforms.lighting_params.y, 0.0, 1.0);

    return vec4<f32>(input.color.rgb * lit, input.color.a * uniforms.surface_color.a);
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

// ---------------------------------------------------------------------------
// Threshold contour ("B")
//
// WebGPU has no line-width state, so a contour of adjustable thickness is drawn
// as quads: each segment becomes four vertices carrying both endpoints, and the
// quad is widened here along the segment's screen-space perpendicular. Widening
// in screen space keeps the line a constant pixel thickness at any zoom, and
// screenshots render at viewport size so saved figures match what is on screen.
// ---------------------------------------------------------------------------

struct ContourUniforms {
    view_projection: mat4x4<f32>,
    model: mat4x4<f32>,
    // Viewport size in physical pixels.
    viewport: vec2<f32>,
    // x: width of this pass in pixels, y: feather width in pixels.
    widths: vec2<f32>,
    color: vec4<f32>,
    // x: 1.0 when auto-contrast, y: 1.0 for the inner line, 0.0 for the casing.
    flags: vec4<f32>,
}

@group(0) @binding(0)
var<uniform> contour: ContourUniforms;

struct ContourVertexInput {
    @location(0) segment_start: vec3<f32>,
    @location(1) segment_end: vec3<f32>,
    // x: side of the centerline (-1 or +1), y: which endpoint (0 or 1),
    // z: luminance of the overlay color underneath.
    @location(2) params: vec3<f32>,
}

struct ContourVertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    // Signed distance from the centerline, in pixels. Interpolated without a
    // perspective divide because it is a screen-space quantity; perspective
    // correction would skew the feather at grazing angles.
    @location(0) @interpolate(linear) offset_px: f32,
    @location(1) @interpolate(flat) luminance: f32,
}

@vertex
fn contour_vs_main(input: ContourVertexInput) -> ContourVertexOutput {
    let side = input.params.x;
    let at_end = input.params.y;

    let clip_start = contour.view_projection * contour.model * vec4<f32>(input.segment_start, 1.0);
    let clip_end = contour.view_projection * contour.model * vec4<f32>(input.segment_end, 1.0);
    var clip_position = select(clip_start, clip_end, at_end > 0.5);

    // Half the viewport maps normalized device coordinates to pixels.
    let half_viewport = max(contour.viewport, vec2<f32>(1.0, 1.0)) * 0.5;

    // Guard against endpoints at or behind the eye, where the perspective
    // divide is meaningless. Such a segment is clipped away anyway; emitting it
    // unwidened avoids producing NaNs that would smear across the screen.
    var direction = vec2<f32>(1.0, 0.0);
    if clip_start.w > 0.0001 && clip_end.w > 0.0001 {
        let pixel_start = (clip_start.xy / clip_start.w) * half_viewport;
        let pixel_end = (clip_end.xy / clip_end.w) * half_viewport;
        let delta = pixel_end - pixel_start;
        let length_px = length(delta);
        if length_px > 0.00001 {
            direction = delta / length_px;
        }
    }
    let perpendicular = vec2<f32>(-direction.y, direction.x);

    // The quad spans the full width plus the feather, so the feather fades
    // outward from the nominal edge instead of eating into the line.
    let half_width = (max(contour.widths.x, 0.0) + max(contour.widths.y, 0.0)) * 0.5;
    let offset_px = perpendicular * side * half_width;
    let offset_ndc = offset_px / half_viewport;
    clip_position.x += offset_ndc.x * clip_position.w;
    clip_position.y += offset_ndc.y * clip_position.w;

    var output: ContourVertexOutput;
    output.clip_position = clip_position;
    output.offset_px = side * half_width;
    output.luminance = input.params.z;
    return output;
}

@fragment
fn contour_fs_main(input: ContourVertexOutput) -> @location(0) vec4<f32> {
    let half_width = (max(contour.widths.x, 0.0) + max(contour.widths.y, 0.0)) * 0.5;
    let half_solid = max(contour.widths.x, 0.0) * 0.5;
    let feather = max(half_width - half_solid, 0.0001);

    // Solid out to the nominal half width, then a linear ramp to zero. The
    // renderer runs without MSAA, so without this a one-pixel diagonal line
    // breaks into a dotted stair.
    let alpha = clamp((half_width - abs(input.offset_px)) / feather, 0.0, 1.0);
    if alpha <= 0.001 {
        discard;
    }

    var rgb = contour.color.rgb;
    if contour.flags.x > 0.5 {
        // Auto-contrast: the inner line takes whichever pole stands out against
        // the overlay color underneath, and the casing takes the other, so the
        // pair stays legible on any colormap or background.
        let contrast = select(0.0, 1.0, input.luminance < 0.5);
        let inner = vec3<f32>(contrast, contrast, contrast);
        rgb = select(vec3<f32>(1.0, 1.0, 1.0) - inner, inner, contour.flags.y > 0.5);
    }

    return vec4<f32>(rgb, contour.color.a * alpha);
}

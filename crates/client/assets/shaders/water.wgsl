// Water fragment shader — Bevy 0.18 ExtendedMaterial extension.
//
// Each fragment receives:
//   in.world_position  — world-space XYZ (XZ used as ripple UV)
//   in.color.rgb       — biome base colour (Water=deep blue, River=lighter blue)
//   in.color.a         — tile-type flag: 1.0=open Water, 0.0=River
//
// Effects:
//   • Two overlapping sine waves perturb the surface normal → lighting ripples.
//   • FBM noise over scrolling UV → organic depth / flow streaks.
//   • River: faster directional scroll + green-brown tint + higher roughness.
//   • Ocean/lake: slow isotropic drift + glassy specular.
//   • Crest brightening at wave peaks (foam suggestion).

#import bevy_pbr::{
    forward_io::VertexOutput,
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{apply_pbr_lighting, main_pass_post_lighting_processing},
}

// ── Uniform ───────────────────────────────────────────────────────────────────

struct WaterExtension {
    time: f32,
}

@group(2) @binding(100)
var<uniform> water: WaterExtension;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn hash2(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

fn smooth_noise(uv: vec2<f32>) -> f32 {
    let i = floor(uv);
    let f = fract(uv);
    let u = f * f * (3.0 - 2.0 * f);
    return mix(
        mix(hash2(i + vec2<f32>(0.0, 0.0)), hash2(i + vec2<f32>(1.0, 0.0)), u.x),
        mix(hash2(i + vec2<f32>(0.0, 1.0)), hash2(i + vec2<f32>(1.0, 1.0)), u.x),
        u.y,
    );
}

fn fbm(uv: vec2<f32>) -> f32 {
    var value = 0.0;
    var amp   = 0.5;
    var freq  = 1.0;
    for (var i = 0; i < 3; i++) {
        value += amp * smooth_noise(uv * freq);
        freq  *= 2.1;
        amp   *= 0.5;
    }
    return value;
}

// ── Fragment entry point ──────────────────────────────────────────────────────

@fragment
fn fragment(
    in:          VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> @location(0) vec4<f32> {
    var pbr_input = pbr_input_from_standard_material(in, is_front);

    let t        = water.time;
    let world_xz = in.world_position.xz;

    // is_river: 1.0=river, 0.0=open water (inverted from vertex alpha flag).
    let is_river = 1.0 - in.color.a;

    // ── Ripple waves ──────────────────────────────────────────────────────────

    let scale1   = 0.35;
    let scale2   = 0.72;
    let ripple1  = sin(world_xz.x * scale1 + t * 0.8) * sin(world_xz.y * scale1 + t * 0.6);
    let ripple2  = sin(world_xz.x * scale2 - t * 1.1) * sin(world_xz.y * scale2 + t * 0.9);
    let ripple   = (ripple1 + ripple2) * 0.5;  // range [−1, 1]

    // Perturb surface normal slightly for lighting interaction.
    let tilt   = 0.10 * ripple;
    pbr_input.N = normalize(vec3<f32>(tilt, 1.0, tilt * 0.8));

    // ── Scrolling FBM ─────────────────────────────────────────────────────────

    let ocean_scroll = vec2<f32>(t * 0.04, t * 0.03);
    let river_scroll = vec2<f32>(t * 0.18, t * 0.05);
    let scroll       = mix(ocean_scroll, river_scroll, is_river);
    let flow_uv      = world_xz * 0.25 + scroll;
    let flow_noise   = fbm(flow_uv);  // ~[0.2, 0.8]

    // ── Colour composition ────────────────────────────────────────────────────

    let base_col = in.color.rgb;

    // Depth modulation: troughs darker, peaks lighter.
    let depth_col = mix(base_col * 0.55, base_col * 1.15, flow_noise);

    // Foam / crest brightening at wave peaks.
    let crest     = smoothstep(0.55, 0.90, (ripple + 1.0) * 0.5);
    let foam_col  = vec3<f32>(0.82, 0.92, 1.00);
    let ocean_col = mix(depth_col, foam_col, crest * 0.35);

    // River tint: slight green-brown cast + directional streak.
    let river_tint = vec3<f32>(0.18, 0.50, 0.30);
    let streak     = smoothstep(0.60, 0.80, flow_noise) * 0.25;
    let river_col  = mix(depth_col, river_tint, 0.15) + vec3<f32>(streak);

    let final_col = mix(ocean_col, river_col, is_river);

    // ── Apply to PBR input ────────────────────────────────────────────────────

    pbr_input.material.base_color = vec4<f32>(final_col, 0.85);

    // Rivers rougher (turbulent surface), ocean glassy.
    pbr_input.material.perceptual_roughness = mix(0.05, 0.22, is_river);

    // ── Lighting ──────────────────────────────────────────────────────────────

    var out_color = apply_pbr_lighting(pbr_input);
    out_color = main_pass_post_lighting_processing(pbr_input, out_color);
    return out_color;
}

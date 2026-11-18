struct Uniforms {
    viewport: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0)
var<uniform> u: Uniforms;

struct QuadInstance {
    @location(0) pos_size: vec4<f32>,
    @location(1) color: vec4<f32>,
    @location(2) border_color: vec4<f32>,
    @location(3) params: vec4<f32>, // x: radius, y: border_width, z: shadow_blur, w: unused
    @location(4) shadow_color: vec4<f32>,
};

struct QuadOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) local_pos: vec2<f32>,
    @location(1) half_size: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) border_color: vec4<f32>,
    @location(4) params: vec4<f32>,
    @location(5) shadow_color: vec4<f32>,
};

fn rounded_box_sdf(p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let r = min(radius, min(half_size.x, half_size.y));
    let q = abs(p) - half_size + vec2<f32>(r, r);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

@vertex
fn vs_quad(@builtin(vertex_index) vid: u32, inst: QuadInstance) -> QuadOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    let c = corners[vid];
    let shadow_pad = inst.params.z * 2.0;
    let origin = inst.pos_size.xy - vec2<f32>(shadow_pad, shadow_pad);
    let size = inst.pos_size.zw + vec2<f32>(shadow_pad * 2.0, shadow_pad * 2.0);
    let px = origin + c * size;

    let ndc = vec2<f32>(
        (px.x / u.viewport.x) * 2.0 - 1.0,
        1.0 - (px.y / u.viewport.y) * 2.0,
    );

    let center = inst.pos_size.xy + inst.pos_size.zw * 0.5;
    var out: QuadOut;
    out.clip_pos = vec4<f32>(ndc, 0.0, 1.0);
    out.local_pos = px - center;
    out.half_size = inst.pos_size.zw * 0.5;
    out.color = inst.color;
    out.border_color = inst.border_color;
    out.params = inst.params;
    out.shadow_color = inst.shadow_color;
    return out;
}

@fragment
fn fs_quad(in: QuadOut) -> @location(0) vec4<f32> {
    let radius = in.params.x;
    let border_w = in.params.y;
    let shadow_blur = in.params.z;

    let d = rounded_box_sdf(in.local_pos, in.half_size, radius);
    let aa = 0.75;

    var out_col = vec4<f32>(0.0);

    // Soft drop shadow outside the box
    if shadow_blur > 0.0 && in.shadow_color.a > 0.0 {
        let shadow_alpha = 1.0 - smoothstep(-1.0, shadow_blur * 1.5, d);
        out_col = vec4<f32>(in.shadow_color.rgb, in.shadow_color.a * shadow_alpha);
    }

    // Fill + border
    let fill_alpha = 1.0 - smoothstep(-aa, aa, d);
    if fill_alpha > 0.0 {
        var body = in.color;
        if border_w > 0.0 && in.border_color.a > 0.0 {
            let inner_d = d + border_w;
            let border_mix = smoothstep(-aa, aa, inner_d);
            body = mix(in.color, in.border_color, border_mix);
        }
        let body_a = body.a * fill_alpha;
        // Alpha-over composite onto shadow
        let out_a = body_a + out_col.a * (1.0 - body_a);
        let out_rgb = select(
            vec3<f32>(0.0),
            (body.rgb * body_a + out_col.rgb * out_col.a * (1.0 - body_a)) / out_a,
            out_a > 0.0001
        );
        out_col = vec4<f32>(out_rgb, out_a);
    }

    if out_col.a <= 0.001 {
        discard;
    }
    return out_col;
}

// --- Text Glyph Pipeline ---

@group(1) @binding(0)
var glyph_tex: texture_2d<f32>;
@group(1) @binding(1)
var glyph_sampler: sampler;

struct TextInstance {
    @location(0) pos_size: vec4<f32>,
    @location(1) uv_rect: vec4<f32>,
    @location(2) color: vec4<f32>,
};

struct TextOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_text(@builtin(vertex_index) vid: u32, inst: TextInstance) -> TextOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    let c = corners[vid];
    let px = inst.pos_size.xy + c * inst.pos_size.zw;
    let ndc = vec2<f32>(
        (px.x / u.viewport.x) * 2.0 - 1.0,
        1.0 - (px.y / u.viewport.y) * 2.0,
    );
    let uv = mix(inst.uv_rect.xy, inst.uv_rect.zw, c);

    var out: TextOut;
    out.clip_pos = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = uv;
    out.color = inst.color;
    return out;
}

@fragment
fn fs_text(in: TextOut) -> @location(0) vec4<f32> {
    let cov = textureSample(glyph_tex, glyph_sampler, in.uv).r;
    let a = in.color.a * cov;
    if a <= 0.001 {
        discard;
    }
    return vec4<f32>(in.color.rgb, a);
}

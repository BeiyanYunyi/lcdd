struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

struct BlurParams {
    direction: vec2<f32>,
    texel_size: vec2<f32>,
    radius: u32,
    _padding: u32,
};

@group(0) @binding(0) var input_texture: texture_2d<f32>;
@group(0) @binding(1) var input_sampler: sampler;
@group(0) @binding(2) var<uniform> params: BlurParams;

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(3.0, 1.0),
    );
    var uvs = array<vec2<f32>, 3>(
        vec2<f32>(0.0, 2.0),
        vec2<f32>(0.0, 0.0),
        vec2<f32>(2.0, 0.0),
    );

    var out: VertexOutput;
    out.position = vec4<f32>(positions[vertex_index], 0.0, 1.0);
    out.uv = uvs[vertex_index];
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    var color = vec4<f32>(0.0);
    var weight_sum = 0.0;
    let radius = i32(params.radius);

    for (var offset = -32; offset <= 32; offset = offset + 1) {
        if (abs(offset) > radius) {
            continue;
        }

        let weight = f32(radius + 1 - abs(offset));
        let sample_uv = in.uv + params.direction * params.texel_size * f32(offset);
        color = color + textureSample(input_texture, input_sampler, sample_uv) * weight;
        weight_sum = weight_sum + weight;
    }

    return color / max(weight_sum, 1.0);
}

@group(0) @binding(0) var<uniform> camera: mat4x4<f32>;
@group(1) @binding(0) var material_texture: texture_2d<f32>;
@group(1) @binding(1) var material_sampler: sampler;
struct Output {
    @builtin(position) position:vec4<f32>,
    @location(0) normal:vec3<f32>,
    @location(1) uv:vec2<f32>,
    @location(2) diffuse:vec4<f32>,
};
@vertex fn vs_main(@location(0) position:vec3<f32>, @location(1) normal:vec3<f32>, @location(2) uv:vec2<f32>, @location(3) diffuse:vec4<f32>)->Output {
    var out:Output;out.position=camera*vec4<f32>(position,1.);out.normal=normal;out.uv=uv;out.diffuse=diffuse;return out;
}
fn shade(input:Output, front:bool, color:vec4<f32>)->vec4<f32> {
    let normal=normalize(input.normal)*select(-1.,1.,front);
    let light=0.62+0.38*max(dot(normal,normalize(vec3<f32>(-0.5,0.7,-1.))),0.);
    return vec4<f32>(color.rgb*light,color.a);
}
@fragment fn fs_solid(input:Output, @builtin(front_facing) front:bool)->@location(0) vec4<f32> {
    let color=textureSample(material_texture,material_sampler,input.uv)*input.diffuse;
    if color.a<1. {discard;}
    return shade(input,front,color);
}
@fragment fn fs_translucent(input:Output, @builtin(front_facing) front:bool)->@location(0) vec4<f32> {
    let color=textureSample(material_texture,material_sampler,input.uv)*input.diffuse;
    if color.a<=0. || color.a>=1. {discard;}
    return shade(input,front,color);
}

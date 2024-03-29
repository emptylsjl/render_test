#version 460

//#extension GL_ARB_separate_shader_objects : enable
//#extension GL_ARB_shading_language_420pack : enable
//
//layout (location = 0) in vec4 o_color;
//layout (location = 0) out vec4 uFragColor;
//
//void main() {
//    uFragColor = o_color;
//}


layout(binding = 1) uniform sampler2D texSampler;
layout(location = 1) in vec2 fragTexCoord;

layout(location = 0) in vec3 fragColor;

layout(location = 0) out vec4 outColor;

void main() {
    outColor = texture(texSampler, fragTexCoord);
}
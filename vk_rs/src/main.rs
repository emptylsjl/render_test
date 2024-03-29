#![feature(cstr_from_bytes_until_nul)]
#![allow(clippy::single_match)]
#![feature(core_panic)]

extern crate core;

use std::error::Error;
use std::ffi::{c_char, CStr};
use std::fmt::{Debug, Formatter, Pointer};
use std::{mem, ptr, slice, time};
use std::borrow::Borrow;
use std::fs::{File, read};
use std::mem::size_of_val;

use ash::{*, };
use itertools::Itertools;
use png::{ColorType, Info, OutputInfo, Reader};
use winit::{
    event::{Event, DeviceEvent as DE, WindowEvent as WE},
    event_loop::EventLoop,
    window::WindowBuilder,
};
use winit::window::Window;
use raw_window_handle::{HasRawDisplayHandle, HasRawWindowHandle, RawWindowHandle};
use smallvec::{smallvec, SmallVec};
use winit::dpi::PhysicalSize;
use winit::event::VirtualKeyCode;
use winit::platform::run_return::EventLoopExtRunReturn;

mod define;
mod vk_proc;
mod present;
mod device;
mod swapchain;
mod vertex;

use define::*;
use vk_proc::dvk::*;
use vk_proc::proc::VKProc;
use crate::device::VKDevice;
use crate::present::{create_image_views, create_vk_surface, VKFramebuffer, VKWindow};
use crate::vertex::{camera, Vertex, Vertices};

fn spirv_from_bytes(bytes: &[u8]) -> Vec<u32> {

    assert_eq!(bytes.len() % 4, 0, "shader len");
    let bytes_ptr = bytes.as_ptr() as *mut u32;
    let spirv = unsafe { slice::from_raw_parts(bytes_ptr, bytes.len()/4) }.to_vec();

    // let mut spirv: Vec<_> = bytes
    //     .chunks(4)
    //     .map(|word| u32::from_le_bytes(word.try_into().unwrap()))
    //     .collect();

    match spirv[0] {
        SPV_MAGIC_NUMBER_LE => { spirv },
        SPV_MAGIC_NUMBER_BE => { spirv.into_iter().map(|x| x.swap_bytes()).collect() },
        _ => {panic!("spir-v invalid")}
    }
}

// fn shader_module(device: &Device, path: &str) -> ShaderModule {
fn create_shader_module<'a>(device: &Device, bytes: &[u8], stage: vk::ShaderStageFlags) -> vk::PipelineShaderStageCreateInfo<'a> {
    optick::event!();

    // let mut file = std::fs::File::open(path).unwrap();
    // let spirv_bytes = util::read_spv(&mut file).unwrap();
    let spirv_bytes = spirv_from_bytes(bytes);
    let shader_create_info = vk::ShaderModuleCreateInfo::default().code(&spirv_bytes);
    let shader_module = unsafe {
        device
            .create_shader_module(&shader_create_info, None)
            .expect("create shader create")
    };
    vk::PipelineShaderStageCreateInfo::default()
        .stage(stage)
        .module(shader_module)
        .name(CStr::from_bytes_with_nul(b"main\0").unwrap())
}


fn create_descriptor_set_layout(device: &VKDevice) -> vk::DescriptorSetLayout {
    optick::event!();

    let descriptor_set_layout_bindings =  [
        vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_count(1)
            .descriptor_type(VK_DESCRIPTOR_TYPE_UNIFORM_BUFFER)
            .stage_flags(VK_SHADER_STAGE_VERTEX_BIT),
        vk::DescriptorSetLayoutBinding::default()
            .binding(1)
            .descriptor_count(1)
            .descriptor_type(VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER)
            .stage_flags(VK_SHADER_STAGE_FRAGMENT_BIT),
    ];

    let descriptor_set_layout_create_info = vk::DescriptorSetLayoutCreateInfo::default()
        .bindings(&descriptor_set_layout_bindings);

    unsafe {
        device.device
            .create_descriptor_set_layout(&descriptor_set_layout_create_info, None)
            .expect("Failed to create Descriptor Set Layout!")
    }
}

fn create_descriptor_pool(device: &VKDevice) -> vk::DescriptorPool {
    optick::event!();

    let descriptor_pool_sizes = [
        vk::DescriptorPoolSize::default()
            .ty(VK_DESCRIPTOR_TYPE_UNIFORM_BUFFER)
            .descriptor_count(FRAMES_IN_FLIGHT as u32),
        vk::DescriptorPoolSize::default()
            .ty(VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER)
            .descriptor_count(FRAMES_IN_FLIGHT as u32)
    ];

    let descriptor_pool_create_info = vk::DescriptorPoolCreateInfo::default()
        .max_sets(FRAMES_IN_FLIGHT as u32)
        .pool_sizes(&descriptor_pool_sizes);

    unsafe {
        device.device
            .create_descriptor_pool(&descriptor_pool_create_info, None)
            .expect("create Descriptor Pool")
    }
}

fn create_descriptor_sets<T>(
    device: &VKDevice,
    descriptor_pool: &vk::DescriptorPool,
    descriptor_set_layout: &vk::DescriptorSetLayout,
    uniform_buffers: &Vec<VKBuffer<T>>,
    texture_image_view: &vk::ImageView,
    texture_sampler: &vk::Sampler,
) -> Vec<vk::DescriptorSet> {
    optick::event!();

    let layouts = [*descriptor_set_layout; FRAMES_IN_FLIGHT];

    let descriptor_set_allocate_info = vk::DescriptorSetAllocateInfo::default()
        .descriptor_pool(*descriptor_pool)
        .set_layouts(&layouts);

    let descriptor_sets = unsafe {
        device.device
            .allocate_descriptor_sets(&descriptor_set_allocate_info)
            .expect("Failed to allocate descriptor sets!")
    };

    descriptor_sets.iter().zip(uniform_buffers).for_each(|(&descriptor_set, &ref buffer)| {
        let descriptor_buffer_infos = [
            vk::DescriptorBufferInfo::default()
                .range(std::mem::size_of::<glam::Mat4>() as u64)
                .buffer(buffer.buffer)
                .offset(0),
        ];
        let descriptor_image_infos = [
            vk::DescriptorImageInfo::default()
                .image_layout(VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL)
                .image_view(*texture_image_view)
                .sampler(*texture_sampler),
        ];

        let descriptor_write_sets = [
            vk::WriteDescriptorSet::default()
                .descriptor_type(VK_DESCRIPTOR_TYPE_UNIFORM_BUFFER)
                .buffer_info(&descriptor_buffer_infos)
                .dst_set(descriptor_set)
                .dst_array_element(0)
                .dst_binding(0),
            vk::WriteDescriptorSet::default()
                .descriptor_type(VK_DESCRIPTOR_TYPE_COMBINED_IMAGE_SAMPLER)
                .image_info(&descriptor_image_infos)
                .dst_set(descriptor_set)
                .dst_array_element(0)
                .dst_binding(1),

        ];

        unsafe {
            device.device.update_descriptor_sets(&descriptor_write_sets, &[]);
        }
    });
    descriptor_sets
}

fn create_pipeline_layout(device: &VKDevice, descriptor_set_layout: &[vk::DescriptorSetLayout]) -> vk::PipelineLayout {
    optick::event!();

    let layout_create_info = vk::PipelineLayoutCreateInfo::default()
        .set_layouts(descriptor_set_layout);

    unsafe {
        device.device
            .create_pipeline_layout(&layout_create_info, None)
            .expect("create pipeline layout")
    }
}

fn create_render_pass(device: &VKDevice, format: vk::Format) -> vk::RenderPass {
    optick::event!();

    let attachment_description = [
        vk::AttachmentDescription::default()
            .format(format)
            .samples(VK_SAMPLE_COUNT_1_BIT)
            .load_op(VK_ATTACHMENT_LOAD_OP_CLEAR)
            .store_op(VK_ATTACHMENT_STORE_OP_STORE)
            .stencil_load_op(VK_ATTACHMENT_LOAD_OP_DONT_CARE)
            .stencil_store_op(VK_ATTACHMENT_STORE_OP_DONT_CARE)
            .initial_layout(VK_IMAGE_LAYOUT_UNDEFINED)
            .final_layout(VK_IMAGE_LAYOUT_PRESENT_SRC_KHR)
    ];

    let attachment_references = [
        vk::AttachmentReference::default()
            .layout(VK_IMAGE_LAYOUT_COLOR_ATTACHMENT_OPTIMAL)
            .attachment(0)
    ];

    let subpass_descriptions = [
        vk::SubpassDescription::default()
            .pipeline_bind_point(VK_PIPELINE_BIND_POINT_GRAPHICS)
            .color_attachments(&attachment_references)
    ];

    let subpass_dependencies = [
        vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .src_stage_mask(VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT)
            .dst_stage_mask(VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT)
            .dst_access_mask(VK_ACCESS_COLOR_ATTACHMENT_WRITE_BIT)
    ];

    let render_pass_create_info = vk::RenderPassCreateInfo::default()
        .attachments(&attachment_description)
        .subpasses(&subpass_descriptions)
        .dependencies(&subpass_dependencies);

    unsafe {
        device.device
            .create_render_pass(&render_pass_create_info, None)
            .expect("create render pass")
    }
}

fn create_graphic_pipeline(
    device: &VKDevice,
    render_pass: &vk::RenderPass,
    pipeline_layout: &vk::PipelineLayout,
    shader_create_infos: &[vk::PipelineShaderStageCreateInfo],
) -> Vec<vk::Pipeline> {
    optick::event!();

    let vertex_binding_descriptions = Vertex::binding_description();
    let vertex_attribute_descriptions = Vertices::attribute_descriptions();

    let vertex_input_create_info = vk::PipelineVertexInputStateCreateInfo::default()
        .vertex_binding_descriptions(&vertex_binding_descriptions)
        .vertex_attribute_descriptions(&vertex_attribute_descriptions);

    let input_assembly_create_info = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(VK_PRIMITIVE_TOPOLOGY_TRIANGLE_LIST)
        .primitive_restart_enable(false);

    let dynamic_states = [
        VK_DYNAMIC_STATE_VIEWPORT,
        VK_DYNAMIC_STATE_SCISSOR
    ];
    let dynamic_state_create_info = vk::PipelineDynamicStateCreateInfo::default()
        .dynamic_states(&dynamic_states);

    let viewport_create_info = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);

    let rasterization_create_info = vk::PipelineRasterizationStateCreateInfo::default()
        .depth_clamp_enable(false)
        .rasterizer_discard_enable(false)
        .polygon_mode(VK_POLYGON_MODE_FILL)
        .line_width(1.0)
        .cull_mode(VK_CULL_MODE_NONE)
        .front_face(VK_FRONT_FACE_COUNTER_CLOCKWISE)
        .depth_clamp_enable(false);

    let multisample_create_info = vk::PipelineMultisampleStateCreateInfo::default()
        .rasterization_samples(VK_SAMPLE_COUNT_1_BIT);

    let color_blend_attachment = [
        vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(false)
            .color_blend_op(VK_BLEND_OP_ADD)
            .alpha_blend_op(VK_BLEND_OP_ADD)
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .src_color_blend_factor(VK_BLEND_FACTOR_ONE)
            .dst_color_blend_factor(VK_BLEND_FACTOR_ZERO)
            .src_alpha_blend_factor(VK_BLEND_FACTOR_ONE)
            .dst_alpha_blend_factor(VK_BLEND_FACTOR_ZERO)
        // .src_color_blend_factor(VK_BLEND_FACTOR_SRC_ALPHA)
        // .dst_color_blend_factor(VK_BLEND_FACTOR_ONE_MINUS_SRC_ALPHA)
        // .src_alpha_blend_factor(VK_BLEND_FACTOR_ONE)
        // .dst_alpha_blend_factor(VK_BLEND_FACTOR_ONE)
    ];

    let color_blend_create_info = vk::PipelineColorBlendStateCreateInfo::default()
        .logic_op_enable(false)
        .logic_op(VK_LOGIC_OP_COPY)
        .attachments(&color_blend_attachment);

    let pipeline_create_info = [
        vk::GraphicsPipelineCreateInfo::default()
            .stages(&shader_create_infos)
            .vertex_input_state(&vertex_input_create_info)
            .input_assembly_state(&input_assembly_create_info)
            .viewport_state(&viewport_create_info)
            .rasterization_state(&rasterization_create_info)
            .multisample_state(&multisample_create_info)
            // .depth_stencil_state(&depth_state_info)
            .color_blend_state(&color_blend_create_info)
            .dynamic_state(&dynamic_state_create_info)
            .layout(*pipeline_layout)
            .render_pass(*render_pass)
            .base_pipeline_index(-1)
    ];

    unsafe {
        device.device
            .create_graphics_pipelines(vk::PipelineCache::null(), &pipeline_create_info, None)
            .expect("create graphics pipeline")
    }
}

fn record_command_buffer(
    device: &VKDevice,
    window: &VKWindow,
    image_index: &u32,
    render_pass: &vk::RenderPass,
    framebuffer: &VKFramebuffer,
    index_buffer: &vk::Buffer,
    vertex_buffer: &vk::Buffer,
    command_buffer: &vk::CommandBuffer,
    descriptor_set: &vk::DescriptorSet,
    pipeline_layout: &vk::PipelineLayout,
    graphic_pipeline: &vk::Pipeline,
) {
    optick::event!();
    let descriptor_sets = [*descriptor_set];
    let command_buffer_begin_info = vk::CommandBufferBeginInfo::default()
        .flags(vk::CommandBufferUsageFlags::default());

    let clear_values = [
        vk::ClearValue {color: vk::ClearColorValue {float32: [0.,0.,0.,0.]}}
    ];
    let swapchain_extent = window.get_extent();

    let render_pass_begin_info = vk::RenderPassBeginInfo::default()
        .render_pass(*render_pass)
        .framebuffer(framebuffer.index(*image_index))
        .render_area(swapchain_extent.into())
        .clear_values(&clear_values);

    let scissors = [
        vk::Rect2D::from(swapchain_extent)
    ];
    let viewports = [
        vk::Viewport::default()
            .x(0.)
            .y(0.)
            .height(swapchain_extent.height as f32)
            .width(swapchain_extent.width as f32)
            .min_depth(0.)
            .max_depth(1.)
    ];

    unsafe {

        device.device.begin_command_buffer(*command_buffer, &command_buffer_begin_info).expect("begin command buffer");

        device.device.cmd_begin_render_pass(
            *command_buffer,
            &render_pass_begin_info,
            vk::SubpassContents::INLINE,
        );
        device.device.cmd_bind_pipeline(
            *command_buffer,
            vk::PipelineBindPoint::GRAPHICS,
            *graphic_pipeline
        );
        device.device.cmd_set_viewport(*command_buffer, 0, &viewports);
        device.device.cmd_set_scissor(*command_buffer, 0, &scissors);
        device.device.cmd_bind_vertex_buffers(
            *command_buffer,
            0,
            &[*vertex_buffer],
            &[0],
        );
        device.device.cmd_bind_index_buffer(
            *command_buffer,
            *index_buffer,
            0,
            vk::IndexType::UINT32,
        );
        device.device.cmd_bind_descriptor_sets(
            *command_buffer,
            vk::PipelineBindPoint::GRAPHICS,
            *pipeline_layout,
            0,
            &descriptor_sets,
            &[],
        );

        device.device.cmd_draw_indexed(
            *command_buffer,
            6,
            1,
            0,
            0,
            0
        );
        device.device.cmd_end_render_pass(*command_buffer);
        device.device.end_command_buffer(*command_buffer).expect("end command buffer");
    }
}

fn slice_size<T: Copy + Sized>(slice: &[T]) -> usize {
    std::mem::size_of::<T>() * slice.len()
}

#[derive(Debug)]
struct VKBuffer<'a, T> {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    mapped: Option<&'a mut [T]>
}

fn read_img(path: &str) -> (Vec<u8>, OutputInfo) {
    optick::event!();
    let decoder = png::Decoder::new(File::open(path).unwrap());
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size()];
    let wh = reader.next_frame(&mut buf).unwrap();
    (buf, wh)
}

fn rgb2rgba(rgb_vec: Vec<u8>) -> Vec<u8>{
    optick::event!();

    assert_eq!(rgb_vec.len() % 3, 0, "uha?");
    let mut rgba_vec = vec![255u8; rgb_vec.len()/3*4];
    for (rgb, rgba) in rgb_vec.chunks(3).zip(rgba_vec.chunks_mut(4)) {
        unsafe { rgb.as_ptr().copy_to_nonoverlapping(rgba.as_mut_ptr(), 3) }
    }
    rgba_vec
}

fn main() {

    optick::start_capture();
    optick::event!();

    let imgs = [
        read_img("E:/storage/graphic/ai_gen/a/00014-1244403046.png"),
        read_img("E:/storage/graphic/ai_gen/a/394562.png"),
        read_img("E:/storage/graphic/ai_gen/a/00549-1880067250.png"),
    ].map(|(rgb_vec, info)| {
        (rgb2rgba(rgb_vec), OutputInfo {color_type: ColorType::Rgba, ..info})
    });
    let img = &imgs[0];

    let img_mem_size = imgs.iter().map(|(x, _)| x.len() as u64).sum::<u64>();
    // println!("{}", mem::size_of_val(&imgs));
    // println!("{}", mem::size_of_val(&imgs));

    let vkproc = VKProc::new(true);

    let mut event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title("window!")
        .with_inner_size(winit::dpi::LogicalSize::new(H, W))
        .build(&event_loop)
        .unwrap();

    let surface = create_vk_surface(&vkproc, &window).unwrap();

    let vkdevice = VKDevice::new(&vkproc, &surface);

    let mut vkwindow = VKWindow::new(&vkproc, &vkdevice, window, surface);

    let render_pass = create_render_pass(&vkdevice, vkwindow.get_format().format);

    let mut framebuffer = VKFramebuffer::new(&vkdevice, &vkwindow, &render_pass);

    let device = &vkdevice.device;
    let queue = vkdevice.graphic_queue.queue;
    let queue_index = vkdevice.graphic_queue.index;

    let command_pool = vkdevice.create_command_pool(queue_index);
    let command_buffers = vkdevice.command_buffer_allocate(&command_pool, 2);


    let vertices = Vertices::new(vec![
        Vertex::from_arr(
            [-0.4, -0.4, 0.0, 2.0],
            [1.0, 0.0, 0.0, 1.0],
            [1.0, 0.0]
        ),
        Vertex::from_arr(
            [0.4, -0.4, 0.0, 2.0],
            [0.0, 1.0, 0.0, 1.0],
            [0.0, 0.0]
        ),
        Vertex::from_arr(
            [-0.4, 0.4, 0.0, 2.0],
            [1.0, 0.0, 0.0, 1.0],
            [1.0, 1.0]
        ),
        Vertex::from_arr(
            [0.4, 0.4, 0.0, 2.0],
            [0.0, 0.0, 1.0, 1.0],
            [0.0, 1.0]
        ),
    ]);
    let indices = [0u32, 1, 2, 3, 2, 0];

    let (temp_buffer, temp_memory) = vkdevice.create_buffer(
        img.0.len() as u64,
        VK_BUFFER_USAGE_TRANSFER_SRC_BIT,
        VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT,
    );
    vkdevice.map_memory(&vertices.pts, &temp_memory, true);
    let (vertex_buffer, vertex_memory) = vkdevice.create_buffer(
        vertices.mem_size() as u64,
        VK_BUFFER_USAGE_TRANSFER_DST_BIT | VK_BUFFER_USAGE_VERTEX_BUFFER_BIT,
        VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT,
    );
    vkdevice.copy_memory(
        &queue,
        &temp_buffer,
        &vertex_buffer,
        vertices.mem_size() as _,
        &command_pool,
    );
    vkdevice.map_memory(&indices, &temp_memory, true);
    let (index_buffer, index_memory) = vkdevice.create_buffer(
        vertices.mem_size() as u64,
        VK_BUFFER_USAGE_TRANSFER_DST_BIT | VK_BUFFER_USAGE_INDEX_BUFFER_BIT,
        VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT,
    );
    vkdevice.copy_memory(
        &queue,
        &temp_buffer,
        &index_buffer,
        slice_size(&indices) as _,
        &command_pool,
    );

    let mut uniform_buffers = (0..FRAMES_IN_FLIGHT).map(|_| {
        optick::event!();
        let (buffer, memory) = vkdevice.create_buffer(
            std::mem::size_of::<glam::Mat4>() as u64,
            VK_BUFFER_USAGE_UNIFORM_BUFFER_BIT,
            VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT,
        );
        let mapped_memory = vkdevice.map_memory::<glam::Mat4>(
            &[camera([0.0, 0.0, W as f32, H as f32])],
            &memory,
            false
        ).unwrap();
        VKBuffer {buffer, memory, mapped: Some(mapped_memory)}
    }).collect::<Vec<_>>();

    vkdevice.map_memory(&img.0, &temp_memory, true);

    let img_extent = vk::Extent3D { width: img.1.width, height: img.1.height, depth: 1 };
    let (texture_image, texture_memory) = vkdevice.create_image(
        VK_FORMAT_R8G8B8A8_SRGB,
        img_extent,
        VK_IMAGE_TILING_OPTIMAL,
        VK_IMAGE_USAGE_TRANSFER_DST_BIT | VK_IMAGE_USAGE_SAMPLED_BIT,
        VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT
    );

    vkdevice.set_image_layout(texture_image, queue, command_pool, VK_IMAGE_LAYOUT_UNDEFINED, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL);

    vkdevice.once_command_buffer(command_pool, queue, |command_buffer| {

        let image_offset = vk::Offset3D {x: 0, y: 0, z: 0};
        let image_subresource = vk::ImageSubresourceLayers::default()
            .aspect_mask(VK_IMAGE_ASPECT_COLOR_BIT)
            .mip_level(0)
            .base_array_layer(0)
            .layer_count(1);

        let buffer_image_region = vk::BufferImageCopy::default()
            .buffer_offset(0)
            .buffer_row_length(0)
            .buffer_image_height(0)
            .image_offset(image_offset)
            .image_extent(img_extent)
            .image_subresource(image_subresource);

        unsafe {
            vkdevice.device.cmd_copy_buffer_to_image(
                command_buffer,
                temp_buffer,
                texture_image,
                VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
                &[buffer_image_region],
            );
        }
    });

    vkdevice.set_image_layout(texture_image, queue, command_pool, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, VK_IMAGE_LAYOUT_SHADER_READ_ONLY_OPTIMAL);

    let texture_image_view = create_image_views(&vkdevice, VK_FORMAT_R8G8B8A8_SRGB, texture_image);

    let sampler_create_info = vk::SamplerCreateInfo::default()
        .mag_filter(VK_FILTER_LINEAR)
        .min_filter(VK_FILTER_LINEAR)
        .address_mode_u(VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_BORDER)
        .address_mode_v(VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_BORDER)
        .address_mode_w(VK_SAMPLER_ADDRESS_MODE_CLAMP_TO_BORDER)
        .anisotropy_enable(true)
        .max_anisotropy(vkdevice.limits().max_sampler_anisotropy)
        .border_color(VK_BORDER_COLOR_INT_OPAQUE_BLACK)
        .unnormalized_coordinates(false)
        .compare_enable(false)
        .compare_op(VK_COMPARE_OP_ALWAYS)
        .mipmap_mode(VK_SAMPLER_MIPMAP_MODE_LINEAR)
        .mip_lod_bias(0f32)
        .min_lod(0f32)
        .max_lod(0f32);

    let texture_sampler = unsafe {
        vkdevice.device
            .create_sampler(&sampler_create_info, None)
            .expect("create sampler")
    };




    let frag_create_info = create_shader_module(&device, include_bytes!("shader/frag.spv"), VK_SHADER_STAGE_FRAGMENT_BIT);
    let vert_create_info = create_shader_module(&device, include_bytes!("shader/vert.spv"), VK_SHADER_STAGE_VERTEX_BIT);
    let shader_create_infos = [
        frag_create_info,
        vert_create_info
    ];
    let descriptor_set_layout = create_descriptor_set_layout(&vkdevice);
    let pipeline_layout = create_pipeline_layout(&vkdevice, &[descriptor_set_layout]);

    let descriptor_pool = create_descriptor_pool(&vkdevice);
    let descriptor_sets = create_descriptor_sets(
        &vkdevice,
        &descriptor_pool,
        &descriptor_set_layout,
        &uniform_buffers,
        &texture_image_view,
        &texture_sampler
    );

    let graphic_pipelines = create_graphic_pipeline(&vkdevice, &render_pass, &pipeline_layout, &shader_create_infos);

    let semaphore_create_info = vk::SemaphoreCreateInfo::default();
    let fence_create_info = vk::FenceCreateInfo::default()
        .flags(VK_FENCE_CREATE_SIGNALED_BIT);

    // let setup_commands_reuse_fence = device.create_fence(&fence_create_info, None).expect("create fence");

    let (
        draw_end_fences, image_available_semaphores, draw_end_semaphores
    ) = unsafe {(
        // device.create_fence(&fence_create_info, None).expect("create fence"),
        [
            device.create_fence(&fence_create_info, None).expect("create fence"),
            device.create_fence(&fence_create_info, None).expect("create fence")
        ],
        [
            device.create_semaphore(&semaphore_create_info, None).expect("create semaphore"),
            device.create_semaphore(&semaphore_create_info, None).expect("create semaphore")
        ],
        [
            device.create_semaphore(&semaphore_create_info, None).expect("create semaphore"),
            device.create_semaphore(&semaphore_create_info, None).expect("create semaphore")
        ]
    ) };

    let mut index: usize = 0;
    let mut recreate = false;
    let mut exit = false;
    let mut min = false;
    let mut fps = 0.;
    let pstart = time::Instant::now();

    event_loop.run_return(|event, _, control_flow| {
        // println!("{event:?}");

        control_flow.set_wait();
        // print!("\r{:5.04}", 1000. / (fps / index as f64));


        match event {
            Event::MainEventsCleared => {
                vkwindow.window.request_redraw();
            },
            Event::RedrawEventsCleared => {

                optick::next_frame();

                let fstart = time::Instant::now();
                unsafe {
                    optick::event!();
                    optick::tag!(&(index.to_string()+"_frame"), index as u32);
                    if !exit && !min {
                        let frame_index = index % FRAMES_IN_FLIGHT;
                        device.wait_for_fences(&[draw_end_fences[frame_index]], true, u64::MAX).expect("Wait for fence failed.");
                        let image_index = match vkwindow.swapchain_proc.acquire_next_image(
                            vkwindow.swapchain,
                            u64::MAX,
                            image_available_semaphores[frame_index],
                            vk::Fence::null()
                        ) {
                            Ok((index, suboptimal)) => {
                                recreate = suboptimal;
                                index
                            }
                            Err(VK_ERROR_OUT_OF_DATE_KHR) => {
                                vkwindow.recreate_swapchain(&mut framebuffer, &render_pass);
                                return;
                            }
                            Err(a) => { panic!("miao: {a:?}") }
                        };

                        device.reset_fences(&[draw_end_fences[frame_index]]).expect("Reset fences failed.");
                        let command_buffer = command_buffers[frame_index];
                        let graphic_pipeline = graphic_pipelines[0];
                        device.reset_command_buffer(
                            command_buffer,
                            vk::CommandBufferResetFlags::default(),
                        ).expect("Reset command buffer");
                        let descriptor_set = descriptor_sets[frame_index];

                        record_command_buffer(
                            &vkdevice,
                            &vkwindow,
                            &image_index,
                            &render_pass,
                            &framebuffer,
                            &index_buffer,
                            &vertex_buffer,
                            &command_buffer,
                            &descriptor_set,
                            &pipeline_layout,
                            &graphic_pipeline,
                        );

                        let wait_semaphores = [image_available_semaphores[frame_index]];
                        let signal_semaphores = [draw_end_semaphores[frame_index]];
                        let command_bufferss = [command_buffer];
                        let wait_stages = [VK_PIPELINE_STAGE_COLOR_ATTACHMENT_OUTPUT_BIT];
                        let submit_info = vk::SubmitInfo::default()
                            .wait_semaphores(&wait_semaphores)
                            .wait_dst_stage_mask(&wait_stages)
                            .command_buffers(&command_bufferss)
                            .signal_semaphores(&signal_semaphores);
                        device.queue_submit(queue, &[submit_info], draw_end_fences[frame_index]).expect("queue submit");

                        let swapchains = [vkwindow.swapchain];
                        let image_indexes = [image_index];
                        let present_info = vk::PresentInfoKHR::default()
                            .wait_semaphores(&signal_semaphores) // &base.rendering_complete_semaphore)
                            .swapchains(&swapchains)
                            .image_indices(&image_indexes);

                        match vkwindow.swapchain_proc.queue_present(queue, &present_info) {
                            Ok(suboptimal) => { recreate = suboptimal }
                            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => { recreate = true }
                            Err(a) => { print!("miao miao miao\n{a:?}"); }
                        };
                        if recreate {
                            vkwindow.recreate_swapchain(&mut framebuffer, &render_pass);
                            recreate = false;
                        }
                        index += 1;
                    }
                }
                fps += fstart.elapsed().subsec_nanos() as f64 / 1_000_000.;
            }

            // Event::DeviceEvent {event, device_id} => {
            //     match event {
            //         DE::MouseMotion { delta } => {
            //             print!("\r1 - {delta:?}");
            //         }
            // //         // DE::Button { button, state } => {
            // //         //     println!("{button:?}, {state:?}")
            // //         // }
            //         _ => {}
            //     }
            // }

            Event::WindowEvent { event, window_id } => {
                match event {
                    WE::Resized( PhysicalSize {width: w, height: h} ) => {
                        min = w*h == 0;
                        // println!("\n{w} - {h}");
                        // vkpresent.recreate_swapchain(&render_pass)
                    }
                    WE::KeyboardInput { input, is_synthetic, .. } => {
                        // println!("{input:?}, {is_synthetic:?}")
                    }
                    WE::MouseInput { state, button, .. } => {
                        // println!("3 - {button:?}, {state:?}")
                    }
                    WE::CursorMoved { position: winit::dpi::PhysicalPosition { x, y }, .. } => {
                        optick::event!();
                        let winit::dpi::PhysicalSize {width, height } = vkwindow.get_dim_window();
                        let [x, y, w, h] = [x as f32, y as _, width as _, height as _];

                        print!("\r2 - {:4}:{:4}   ", x, y);
                        uniform_buffers[index % FRAMES_IN_FLIGHT].mapped.as_mut().unwrap()[0] = camera([x, y, w, h]);}

                    WE::CloseRequested => {
                        optick::event!();
                        unsafe {
                            device.device_wait_idle().expect("wait idle");

                            draw_end_fences.iter().for_each( |&fence| device.destroy_fence(fence, None));
                            graphic_pipelines.iter().for_each(|&pipeline| device.destroy_pipeline(pipeline, None));
                            shader_create_infos.iter().for_each(|&shader| device.destroy_shader_module(shader.module, None));
                            draw_end_semaphores.iter().for_each( |&semaphore| device.destroy_semaphore(semaphore, None));
                            image_available_semaphores.iter().for_each( |&semaphore| device.destroy_semaphore(semaphore, None));

                            device.destroy_pipeline_layout(pipeline_layout, None);
                            device.destroy_descriptor_pool(descriptor_pool, None);
                            device.destroy_descriptor_set_layout(descriptor_set_layout, None);
                            device.destroy_render_pass(render_pass, None);
                            device.destroy_command_pool(command_pool, None);
                            device.destroy_buffer(temp_buffer, None);
                            device.free_memory(temp_memory, None);
                            device.destroy_buffer(index_buffer, None);
                            device.free_memory(index_memory, None);
                            device.destroy_buffer(vertex_buffer, None);
                            device.free_memory(vertex_memory, None);

                            device.destroy_sampler(texture_sampler, None);
                            device.destroy_image_view(texture_image_view, None);
                            device.destroy_image(texture_image, None);
                            device.free_memory(texture_memory, None);

                            uniform_buffers.iter().for_each(|&VKBuffer{ buffer, memory, .. }| {
                                device.unmap_memory(memory);
                                device.destroy_buffer(buffer, None);
                                device.free_memory(memory, None);
                            });
                        }

                        println!("\n{:5.04}\n", 1000. / (fps / index as f64));
                        control_flow.set_exit();
                        exit = true;
                        optick::stop_capture("/cap/captureA");
                    }
                    _ => {}
                }
            },
            _ => (),
        }
    });
}
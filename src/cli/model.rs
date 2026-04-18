use super::progress::{CliProgressGuard, CliProgressScope};
use super::ModelArgs;
use crate::{
    qem_context_create, qem_context_destroy, qem_simplify, QemMeshView, QemSimplifyOptions,
    QemSimplifyResult, QEM_STATUS_SUCCESS,
};
use gltf_json as json;
use json::validation::Checked::Valid;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

const MODEL_ATTR_NORMAL: u32 = 3;
const MODEL_ATTR_UV0: u32 = 2;
const MODEL_ATTR_COLOR0: u32 = 4;
const MODEL_NUM_ATTRIBUTES: u32 = MODEL_ATTR_NORMAL + MODEL_ATTR_UV0 + MODEL_ATTR_COLOR0;

#[derive(Debug, Default)]
struct CliMeshData {
    vertices: Vec<f32>,
    indices: Vec<u32>,
    material_ids: Vec<i32>,
    num_attributes: u32,
    attribute_weights: Vec<f32>,
    material_descriptors: Vec<MaterialDescriptor>,
    image_descriptors: Vec<ImageDescriptor>,
    sampler_descriptors: Vec<SamplerDescriptor>,
    texture_descriptors: Vec<TextureDescriptor>,
}

#[derive(Debug, Clone)]
struct MaterialDescriptor {
    name: String,
    base_color_factor: [f32; 4],
    metallic_factor: f32,
    roughness_factor: f32,
    emissive_factor: [f32; 3],
    alpha_mode: json::material::AlphaMode,
    alpha_cutoff: Option<f32>,
    double_sided: bool,
    base_color_texture: Option<TextureInfoDescriptor>,
    metallic_roughness_texture: Option<TextureInfoDescriptor>,
    normal_texture: Option<NormalTextureDescriptor>,
    occlusion_texture: Option<OcclusionTextureDescriptor>,
    emissive_texture: Option<TextureInfoDescriptor>,
}

#[derive(Debug, Clone, Default)]
struct ImageDescriptor {
    mime_type: Option<String>,
    uri: Option<String>,
    data: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
struct SamplerDescriptor {
    mag_filter: Option<json::texture::MagFilter>,
    min_filter: Option<json::texture::MinFilter>,
    wrap_s: json::texture::WrappingMode,
    wrap_t: json::texture::WrappingMode,
}

#[derive(Debug, Clone)]
struct TextureDescriptor {
    source_image_index: usize,
    sampler_index: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
struct TextureInfoDescriptor {
    texture_index: usize,
    tex_coord: u32,
}

#[derive(Debug, Clone, Copy)]
struct NormalTextureDescriptor {
    texture_index: usize,
    tex_coord: u32,
    scale: f32,
}

#[derive(Debug, Clone, Copy)]
struct OcclusionTextureDescriptor {
    texture_index: usize,
    tex_coord: u32,
    strength: f32,
}

impl Default for MaterialDescriptor {
    fn default() -> Self {
        Self {
            name: "material_0".to_string(),
            base_color_factor: [1.0, 1.0, 1.0, 1.0],
            metallic_factor: 1.0,
            roughness_factor: 1.0,
            emissive_factor: [0.0, 0.0, 0.0],
            alpha_mode: json::material::AlphaMode::Opaque,
            alpha_cutoff: None,
            double_sided: false,
            base_color_texture: None,
            metallic_roughness_texture: None,
            normal_texture: None,
            occlusion_texture: None,
            emissive_texture: None,
        }
    }
}

impl Default for SamplerDescriptor {
    fn default() -> Self {
        Self {
            mag_filter: None,
            min_filter: None,
            wrap_s: json::texture::WrappingMode::Repeat,
            wrap_t: json::texture::WrappingMode::Repeat,
        }
    }
}

fn default_attribute_weights(num_attributes: u32) -> Vec<f32> {
    if num_attributes == 0 {
        Vec::new()
    } else {
        vec![1.0; num_attributes as usize]
    }
}

fn stride(num_attributes: u32) -> usize {
    (3 + num_attributes) as usize
}

fn default_material_descriptor(name: String) -> MaterialDescriptor {
    MaterialDescriptor {
        name,
        ..Default::default()
    }
}

fn ensure_material_descriptor(
    material_descriptors: &mut Vec<MaterialDescriptor>,
    descriptor: MaterialDescriptor,
) -> i32 {
    material_descriptors.push(descriptor);
    (material_descriptors.len() - 1) as i32
}

fn material_descriptor_from_gltf(material: &gltf::Material<'_>) -> MaterialDescriptor {
    let pbr = material.pbr_metallic_roughness();

    let base_color_texture = pbr.base_color_texture().map(|info| TextureInfoDescriptor {
        texture_index: info.texture().index(),
        tex_coord: info.tex_coord(),
    });

    let metallic_roughness_texture =
        pbr.metallic_roughness_texture().map(|info| TextureInfoDescriptor {
            texture_index: info.texture().index(),
            tex_coord: info.tex_coord(),
        });

    let normal_texture = material.normal_texture().map(|normal| NormalTextureDescriptor {
        texture_index: normal.texture().index(),
        tex_coord: normal.tex_coord(),
        scale: normal.scale(),
    });

    let occlusion_texture = material
        .occlusion_texture()
        .map(|occlusion| OcclusionTextureDescriptor {
            texture_index: occlusion.texture().index(),
            tex_coord: occlusion.tex_coord(),
            strength: occlusion.strength(),
        });

    let emissive_texture = material.emissive_texture().map(|info| TextureInfoDescriptor {
        texture_index: info.texture().index(),
        tex_coord: info.tex_coord(),
    });

    MaterialDescriptor {
        name: material
            .name()
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("material_{}", material.index().unwrap_or(0))),
        base_color_factor: pbr.base_color_factor(),
        metallic_factor: pbr.metallic_factor().clamp(0.0, 1.0),
        roughness_factor: pbr.roughness_factor().clamp(0.0, 1.0),
        emissive_factor: material.emissive_factor(),
        alpha_mode: material.alpha_mode(),
        alpha_cutoff: material.alpha_cutoff(),
        double_sided: material.double_sided(),
        base_color_texture,
        metallic_roughness_texture,
        normal_texture,
        occlusion_texture,
        emissive_texture,
    }
}

fn map_texture_info(
    info: TextureInfoDescriptor,
    texture_index_map: &[Option<json::Index<json::Texture>>],
) -> Option<json::texture::Info> {
    let texture = texture_index_map
        .get(info.texture_index)
        .and_then(|index| *index)?;

    Some(json::texture::Info {
        index: texture,
        tex_coord: info.tex_coord,
        extensions: Default::default(),
        extras: Default::default(),
    })
}

fn map_normal_texture(
    info: NormalTextureDescriptor,
    texture_index_map: &[Option<json::Index<json::Texture>>],
) -> Option<json::material::NormalTexture> {
    let texture = texture_index_map
        .get(info.texture_index)
        .and_then(|index| *index)?;

    Some(json::material::NormalTexture {
        index: texture,
        scale: info.scale,
        tex_coord: info.tex_coord,
        extensions: Default::default(),
        extras: Default::default(),
    })
}

fn map_occlusion_texture(
    info: OcclusionTextureDescriptor,
    texture_index_map: &[Option<json::Index<json::Texture>>],
) -> Option<json::material::OcclusionTexture> {
    let texture = texture_index_map
        .get(info.texture_index)
        .and_then(|index| *index)?;

    Some(json::material::OcclusionTexture {
        index: texture,
        strength: json::material::StrengthFactor(info.strength),
        tex_coord: info.tex_coord,
        extensions: Default::default(),
        extras: Default::default(),
    })
}

fn build_json_material(
    descriptor: &MaterialDescriptor,
    texture_index_map: &[Option<json::Index<json::Texture>>],
) -> json::Material {
    json::Material {
        name: Some(descriptor.name.clone()),
        alpha_cutoff: descriptor.alpha_cutoff.map(json::material::AlphaCutoff),
        alpha_mode: Valid(descriptor.alpha_mode),
        double_sided: descriptor.double_sided,
        pbr_metallic_roughness: json::material::PbrMetallicRoughness {
            base_color_factor: json::material::PbrBaseColorFactor(descriptor.base_color_factor),
            metallic_factor: json::material::StrengthFactor(descriptor.metallic_factor),
            roughness_factor: json::material::StrengthFactor(descriptor.roughness_factor),
            base_color_texture: descriptor
                .base_color_texture
                .and_then(|info| map_texture_info(info, texture_index_map)),
            metallic_roughness_texture: descriptor
                .metallic_roughness_texture
                .and_then(|info| map_texture_info(info, texture_index_map)),
            ..Default::default()
        },
        normal_texture: descriptor
            .normal_texture
            .and_then(|info| map_normal_texture(info, texture_index_map)),
        occlusion_texture: descriptor
            .occlusion_texture
            .and_then(|info| map_occlusion_texture(info, texture_index_map)),
        emissive_texture: descriptor
            .emissive_texture
            .and_then(|info| map_texture_info(info, texture_index_map)),
        emissive_factor: json::material::EmissiveFactor(descriptor.emissive_factor),
        ..Default::default()
    }
}

pub fn handle_model(args: &ModelArgs, _verbose: bool) -> Result<(), Box<dyn std::error::Error>> {
    let input_path = Path::new(&args.input);
    let ext = input_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    let mut mesh_data = match ext.as_str() {
        "obj" => load_obj(input_path)?,
        "glb" | "gltf" => load_glb(input_path)?,
        _ => return Err(format!("Unsupported input format: {}", ext).into()),
    };

    let settings = args.to_simplify_options(mesh_data.indices.len() as u32 / 3);
    println!(
        "Simplifying mesh: {} -> {} triangles",
        mesh_data.indices.len() / 3,
        settings.target_triangles
    );

    simplify_mesh(&mut mesh_data, settings)?;

    let output_path = Path::new(&args.output);
    let out_ext = output_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    match out_ext.as_str() {
        "obj" => save_obj(
            output_path,
            &mesh_data.vertices,
            mesh_data.num_attributes,
            &mesh_data.indices,
        )?,
        "glb" => save_glb(output_path, &mesh_data)?,
        _ => {
            println!(
                "Defaulting to OBJ output for unknown extension: {}",
                out_ext
            );
            save_obj(
                output_path,
                &mesh_data.vertices,
                mesh_data.num_attributes,
                &mesh_data.indices,
            )?;
        }
    }

    println!("Saved simplified mesh to {}", args.output);
    Ok(())
}

fn simplify_mesh(mesh_data: &mut CliMeshData, settings: QemSimplifyOptions) -> Result<(), Box<dyn std::error::Error>> {
    let context = qem_context_create();
    if context.is_null() {
        return Err("Failed to create qem context".into());
    }

    let progress = CliProgressGuard::attach(context, CliProgressScope::Mesh, "网格简化")?;

    let mesh_stride = stride(mesh_data.num_attributes);
    if mesh_data.vertices.len() % mesh_stride != 0 {
        unsafe { qem_context_destroy(context) };
        return Err(format!(
            "Invalid vertex buffer length {} for stride {}",
            mesh_data.vertices.len(),
            mesh_stride
        )
        .into());
    }

    if mesh_data.num_attributes > 0 && mesh_data.attribute_weights.len() != mesh_data.num_attributes as usize {
        unsafe { qem_context_destroy(context) };
        return Err(format!(
            "Invalid attribute weights length {} for num_attributes {}",
            mesh_data.attribute_weights.len(),
            mesh_data.num_attributes
        )
        .into());
    }

    let mut mesh = QemMeshView {
        vertices: mesh_data.vertices.as_mut_ptr(),
        num_vertices: (mesh_data.vertices.len() / mesh_stride) as u32,
        indices: mesh_data.indices.as_mut_ptr(),
        num_indices: mesh_data.indices.len() as u32,
        material_ids: mesh_data.material_ids.as_mut_ptr(),
        num_attributes: mesh_data.num_attributes,
        attribute_weights: if mesh_data.num_attributes == 0 {
            std::ptr::null()
        } else {
            mesh_data.attribute_weights.as_ptr()
        },
    };

    let mut result = QemSimplifyResult::default();
    let status = unsafe { qem_simplify(context, &mut mesh, &settings, &mut result) };

    let final_status = if status == QEM_STATUS_SUCCESS {
        result.status
    } else {
        status
    };

    progress.finish_if_needed(final_status, "网格简化完成", "网格简化失败");
    drop(progress);

    unsafe { qem_context_destroy(context) };

    if status != QEM_STATUS_SUCCESS || result.status != QEM_STATUS_SUCCESS {
        return Err(format!(
            "qem_simplify failed. status={}, result_status={}",
            status, result.status
        )
        .into());
    }

    mesh_data
        .vertices
        .truncate(result.num_vertices as usize * mesh_stride);
    mesh_data.indices.truncate(result.num_indices as usize);
    mesh_data.material_ids.truncate(result.num_triangles as usize);
    Ok(())
}

fn load_obj(path: &Path) -> Result<CliMeshData, Box<dyn std::error::Error>> {
    let (models, _) = tobj::load_obj(path, &tobj::GPU_LOAD_OPTIONS)?;
    let mut mesh_data = CliMeshData::default();

    for model in models {
        let mesh = model.mesh;
        let offset = (mesh_data.vertices.len() / 3) as u32;
        mesh_data.vertices.extend(mesh.positions);

        let num_tris = mesh.indices.len() / 3;
        for idx in mesh.indices {
            mesh_data.indices.push(idx + offset);
        }
        let material_id = mesh.material_id.unwrap_or(0) as i32;
        let material_slot = material_id.max(0) as usize;
        while mesh_data.material_descriptors.len() <= material_slot {
            let next_index = mesh_data.material_descriptors.len();
            mesh_data
                .material_descriptors
                .push(default_material_descriptor(format!("material_{}", next_index)));
        }
        for _ in 0..num_tris {
            mesh_data.material_ids.push(material_id);
        }
    }

    mesh_data.num_attributes = 0;
    mesh_data.attribute_weights = Vec::new();
    Ok(mesh_data)
}

fn save_obj(
    path: &Path,
    vertices: &[f32],
    num_attributes: u32,
    indices: &[u32],
) -> Result<(), Box<dyn std::error::Error>> {
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);

    let mesh_stride = stride(num_attributes);
    for i in 0..(vertices.len() / mesh_stride) {
        let base = i * mesh_stride;
        writeln!(writer, "v {} {} {}", vertices[base], vertices[base + 1], vertices[base + 2])?;
    }
    for i in indices.chunks(3) {
        writeln!(writer, "f {} {} {}", i[0] + 1, i[1] + 1, i[2] + 1)?;
    }
    Ok(())
}

fn load_glb(path: &Path) -> Result<CliMeshData, Box<dyn std::error::Error>> {
    let (document, buffers, _) = gltf::import(path)?;
    let mut mesh_data = CliMeshData {
        num_attributes: MODEL_NUM_ATTRIBUTES,
        attribute_weights: default_attribute_weights(MODEL_NUM_ATTRIBUTES),
        ..Default::default()
    };

    for material in document.materials() {
        mesh_data
            .material_descriptors
            .push(material_descriptor_from_gltf(&material));
    }

    for image in document.images() {
        let descriptor = match image.source() {
            gltf::image::Source::View { view, mime_type } => {
                let buffer = &buffers[view.buffer().index()];
                let start = view.offset();
                let end = start.saturating_add(view.length()).min(buffer.len());
                let data = if end > start {
                    Some(buffer[start..end].to_vec())
                } else {
                    None
                };

                ImageDescriptor {
                    mime_type: Some(mime_type.to_string()),
                    uri: None,
                    data,
                }
            }
            gltf::image::Source::Uri { uri, mime_type } => ImageDescriptor {
                mime_type: mime_type.map(|m| m.to_string()),
                uri: Some(uri.to_string()),
                data: None,
            },
        };

        mesh_data.image_descriptors.push(descriptor);
    }

    for sampler in document.samplers() {
        mesh_data.sampler_descriptors.push(SamplerDescriptor {
            mag_filter: sampler.mag_filter(),
            min_filter: sampler.min_filter(),
            wrap_s: sampler.wrap_s(),
            wrap_t: sampler.wrap_t(),
        });
    }

    for texture in document.textures() {
        mesh_data.texture_descriptors.push(TextureDescriptor {
            source_image_index: texture.source().index(),
            sampler_index: texture.sampler().index(),
        });
    }

    let mut default_material_id: Option<i32> = None;

    for mesh in document.meshes() {
        for primitive in mesh.primitives() {
            let reader = primitive.reader(|buffer| Some(&buffers[buffer.index()]));

            let positions: Vec<[f32; 3]> = match reader.read_positions() {
                Some(iter) => iter.collect(),
                None => continue,
            };

            let indices: Vec<u32> = match reader.read_indices() {
                Some(read) => read.into_u32().collect(),
                None => continue,
            };

            let normals = reader.read_normals().map(|iter| iter.collect::<Vec<[f32; 3]>>());
            let texcoords = reader
                .read_tex_coords(0)
                .map(|iter| iter.into_f32().collect::<Vec<[f32; 2]>>());
            let colors = reader
                .read_colors(0)
                .map(|iter| iter.into_rgba_f32().collect::<Vec<[f32; 4]>>());

            let offset = (mesh_data.vertices.len() / stride(mesh_data.num_attributes)) as u32;
            for (vertex_index, position) in positions.iter().enumerate() {
                let normal = normals
                    .as_ref()
                    .and_then(|n| n.get(vertex_index).copied())
                    .unwrap_or([0.0, 0.0, 1.0]);
                let texcoord = texcoords
                    .as_ref()
                    .and_then(|uv| uv.get(vertex_index).copied())
                    .unwrap_or([0.0, 0.0]);
                let color = colors
                    .as_ref()
                    .and_then(|c| c.get(vertex_index).copied())
                    .unwrap_or([1.0, 1.0, 1.0, 1.0]);

                mesh_data.vertices.extend_from_slice(position);
                mesh_data.vertices.extend_from_slice(&normal);
                mesh_data.vertices.extend_from_slice(&texcoord);
                mesh_data.vertices.extend_from_slice(&color);
            }

            let material_id = if let Some(index) = primitive.material().index() {
                index as i32
            } else {
                *default_material_id.get_or_insert_with(|| {
                    ensure_material_descriptor(
                        &mut mesh_data.material_descriptors,
                        default_material_descriptor("default_material".to_string()),
                    )
                })
            };

            let num_tris = indices.len() / 3;

            for idx in indices {
                mesh_data.indices.push(idx + offset);
            }
            for _ in 0..num_tris {
                mesh_data.material_ids.push(material_id);
            }
        }
    }

    Ok(mesh_data)
}

fn save_glb(path: &Path, mesh_data: &CliMeshData) -> Result<(), Box<dyn std::error::Error>> {
    let mut root = json::Root::default();

    let mesh_stride = stride(mesh_data.num_attributes);
    let vertex_count = mesh_data.vertices.len() / mesh_stride;

    let mut positions = Vec::with_capacity(vertex_count * 3);
    let mut normals = if mesh_data.num_attributes >= 3 {
        Some(Vec::with_capacity(vertex_count * 3))
    } else {
        None
    };
    let mut texcoords = if mesh_data.num_attributes >= 5 {
        Some(Vec::with_capacity(vertex_count * 2))
    } else {
        None
    };
    let mut colors = if mesh_data.num_attributes >= 9 {
        Some(Vec::with_capacity(vertex_count * 4))
    } else {
        None
    };

    for vertex_index in 0..vertex_count {
        let base = vertex_index * mesh_stride;
        positions.extend_from_slice(&mesh_data.vertices[base..base + 3]);

        if let Some(normals) = &mut normals {
            normals.extend_from_slice(&mesh_data.vertices[base + 3..base + 6]);
        }
        if let Some(texcoords) = &mut texcoords {
            texcoords.extend_from_slice(&mesh_data.vertices[base + 6..base + 8]);
        }
        if let Some(colors) = &mut colors {
            colors.extend_from_slice(&mesh_data.vertices[base + 8..base + 12]);
        }
    }

    let mut positions_bytes = Vec::with_capacity(positions.len() * std::mem::size_of::<f32>());
    for &v in &positions {
        positions_bytes.extend_from_slice(&v.to_le_bytes());
    }

    let mut normals_bytes = Vec::new();
    if let Some(normals) = &normals {
        normals_bytes.reserve(normals.len() * std::mem::size_of::<f32>());
        for &v in normals {
            normals_bytes.extend_from_slice(&v.to_le_bytes());
        }
    }

    let mut texcoords_bytes = Vec::new();
    if let Some(texcoords) = &texcoords {
        texcoords_bytes.reserve(texcoords.len() * std::mem::size_of::<f32>());
        for &v in texcoords {
            texcoords_bytes.extend_from_slice(&v.to_le_bytes());
        }
    }

    let mut colors_bytes = Vec::new();
    if let Some(colors) = &colors {
        colors_bytes.reserve(colors.len() * std::mem::size_of::<f32>());
        for &v in colors {
            colors_bytes.extend_from_slice(&v.to_le_bytes());
        }
    }

    let mut per_material_indices: BTreeMap<i32, Vec<u32>> = BTreeMap::new();
    let triangle_count = mesh_data.indices.len() / 3;
    for tri in 0..triangle_count {
        let material_id = mesh_data.material_ids.get(tri).copied().unwrap_or(0).max(0);
        let tri_start = tri * 3;
        per_material_indices
            .entry(material_id)
            .or_default()
            .extend_from_slice(&mesh_data.indices[tri_start..tri_start + 3]);
    }

    let mut buffer_data = Vec::new();
    let positions_offset = buffer_data.len();
    buffer_data.extend_from_slice(&positions_bytes);

    let normals_offset = if !normals_bytes.is_empty() {
        let offset = buffer_data.len();
        buffer_data.extend_from_slice(&normals_bytes);
        Some(offset)
    } else {
        None
    };

    let texcoords_offset = if !texcoords_bytes.is_empty() {
        let offset = buffer_data.len();
        buffer_data.extend_from_slice(&texcoords_bytes);
        Some(offset)
    } else {
        None
    };

    let colors_offset = if !colors_bytes.is_empty() {
        let offset = buffer_data.len();
        buffer_data.extend_from_slice(&colors_bytes);
        Some(offset)
    } else {
        None
    };

    let mut index_blocks: Vec<(i32, usize, usize)> = Vec::new();
    for (material_id, mat_indices) in &per_material_indices {
        let offset = buffer_data.len();
        for &idx in mat_indices {
            buffer_data.extend_from_slice(&idx.to_le_bytes());
        }
        index_blocks.push((*material_id, offset, mat_indices.len()));
    }

    let mut image_index_map: Vec<Option<json::Index<json::Image>>> =
        vec![None; mesh_data.image_descriptors.len()];
    for (image_slot, image_descriptor) in mesh_data.image_descriptors.iter().enumerate() {
        let image_index = if let Some(data) = &image_descriptor.data {
            if data.is_empty() {
                None
            } else {
                let image_offset = buffer_data.len();
                buffer_data.extend_from_slice(data);
                let image_offset_aligned = image_offset as u64;

                let image_buffer_view = root.push(json::buffer::View {
                    buffer: json::Index::new(0),
                    byte_length: json::validation::USize64(data.len() as u64),
                    byte_offset: Some(json::validation::USize64(image_offset_aligned)),
                    byte_stride: None,
                    extensions: Default::default(),
                    extras: Default::default(),
                    name: None,
                    target: None,
                });

                Some(root.push(json::Image {
                    buffer_view: Some(image_buffer_view),
                    mime_type: image_descriptor
                        .mime_type
                        .as_ref()
                        .map(|mime| json::image::MimeType(mime.clone())),
                    name: None,
                    uri: None,
                    extensions: Default::default(),
                    extras: Default::default(),
                }))
            }
        } else {
            image_descriptor.uri.as_ref().map(|uri| {
                root.push(json::Image {
                    buffer_view: None,
                    mime_type: image_descriptor
                        .mime_type
                        .as_ref()
                        .map(|mime| json::image::MimeType(mime.clone())),
                    name: None,
                    uri: Some(uri.clone()),
                    extensions: Default::default(),
                    extras: Default::default(),
                })
            })
        };

        image_index_map[image_slot] = image_index;
    }

    let mut sampler_index_map: Vec<Option<json::Index<json::texture::Sampler>>> =
        vec![None; mesh_data.sampler_descriptors.len()];
    for (sampler_slot, sampler_descriptor) in mesh_data.sampler_descriptors.iter().enumerate() {
        let sampler_index = root.push(json::texture::Sampler {
            mag_filter: sampler_descriptor.mag_filter.map(Valid),
            min_filter: sampler_descriptor.min_filter.map(Valid),
            name: None,
            wrap_s: Valid(sampler_descriptor.wrap_s),
            wrap_t: Valid(sampler_descriptor.wrap_t),
            extensions: Default::default(),
            extras: Default::default(),
        });
        sampler_index_map[sampler_slot] = Some(sampler_index);
    }

    let mut texture_index_map: Vec<Option<json::Index<json::Texture>>> =
        vec![None; mesh_data.texture_descriptors.len()];
    for (texture_slot, texture_descriptor) in mesh_data.texture_descriptors.iter().enumerate() {
        let source_image = image_index_map
            .get(texture_descriptor.source_image_index)
            .and_then(|image| *image);

        let Some(source_image) = source_image else {
            continue;
        };

        let sampler_index = texture_descriptor
            .sampler_index
            .and_then(|sampler_slot| sampler_index_map.get(sampler_slot).and_then(|s| *s));

        let texture_index = root.push(json::Texture {
            name: None,
            sampler: sampler_index,
            source: source_image,
            extensions: Default::default(),
            extras: Default::default(),
        });

        texture_index_map[texture_slot] = Some(texture_index);
    }

    let buffer = root.push(json::Buffer {
        byte_length: json::validation::USize64(buffer_data.len() as u64),
        extensions: Default::default(),
        extras: Default::default(),
        name: None,
        uri: None,
    });

    let vertex_bv = root.push(json::buffer::View {
        buffer,
        byte_length: json::validation::USize64(positions_bytes.len() as u64),
        byte_offset: Some(json::validation::USize64(positions_offset as u64)),
        byte_stride: Some(json::buffer::Stride(12)),
        extensions: Default::default(),
        extras: Default::default(),
        name: None,
        target: Some(Valid(json::buffer::Target::ArrayBuffer)),
    });

    let positions = root.push(json::Accessor {
        buffer_view: Some(vertex_bv),
        byte_offset: Some(json::validation::USize64(0)),
        count: json::validation::USize64(vertex_count as u64),
        component_type: Valid(json::accessor::GenericComponentType(
            json::accessor::ComponentType::F32,
        )),
        extensions: Default::default(),
        extras: Default::default(),
        type_: Valid(json::accessor::Type::Vec3),
        min: None,
        max: None,
        name: None,
        normalized: false,
        sparse: None,
    });

    let normals_accessor = if let Some(normals_offset) = normals_offset {
        let normals_bv = root.push(json::buffer::View {
            buffer,
            byte_length: json::validation::USize64(normals_bytes.len() as u64),
            byte_offset: Some(json::validation::USize64(normals_offset as u64)),
            byte_stride: Some(json::buffer::Stride(12)),
            extensions: Default::default(),
            extras: Default::default(),
            name: None,
            target: Some(Valid(json::buffer::Target::ArrayBuffer)),
        });

        Some(root.push(json::Accessor {
            buffer_view: Some(normals_bv),
            byte_offset: Some(json::validation::USize64(0)),
            count: json::validation::USize64(vertex_count as u64),
            component_type: Valid(json::accessor::GenericComponentType(
                json::accessor::ComponentType::F32,
            )),
            extensions: Default::default(),
            extras: Default::default(),
            type_: Valid(json::accessor::Type::Vec3),
            min: None,
            max: None,
            name: None,
            normalized: false,
            sparse: None,
        }))
    } else {
        None
    };

    let texcoords_accessor = if let Some(texcoords_offset) = texcoords_offset {
        let texcoords_bv = root.push(json::buffer::View {
            buffer,
            byte_length: json::validation::USize64(texcoords_bytes.len() as u64),
            byte_offset: Some(json::validation::USize64(texcoords_offset as u64)),
            byte_stride: Some(json::buffer::Stride(8)),
            extensions: Default::default(),
            extras: Default::default(),
            name: None,
            target: Some(Valid(json::buffer::Target::ArrayBuffer)),
        });

        Some(root.push(json::Accessor {
            buffer_view: Some(texcoords_bv),
            byte_offset: Some(json::validation::USize64(0)),
            count: json::validation::USize64(vertex_count as u64),
            component_type: Valid(json::accessor::GenericComponentType(
                json::accessor::ComponentType::F32,
            )),
            extensions: Default::default(),
            extras: Default::default(),
            type_: Valid(json::accessor::Type::Vec2),
            min: None,
            max: None,
            name: None,
            normalized: false,
            sparse: None,
        }))
    } else {
        None
    };

    let colors_accessor = if let Some(colors_offset) = colors_offset {
        let colors_bv = root.push(json::buffer::View {
            buffer,
            byte_length: json::validation::USize64(colors_bytes.len() as u64),
            byte_offset: Some(json::validation::USize64(colors_offset as u64)),
            byte_stride: Some(json::buffer::Stride(16)),
            extensions: Default::default(),
            extras: Default::default(),
            name: None,
            target: Some(Valid(json::buffer::Target::ArrayBuffer)),
        });

        Some(root.push(json::Accessor {
            buffer_view: Some(colors_bv),
            byte_offset: Some(json::validation::USize64(0)),
            count: json::validation::USize64(vertex_count as u64),
            component_type: Valid(json::accessor::GenericComponentType(
                json::accessor::ComponentType::F32,
            )),
            extensions: Default::default(),
            extras: Default::default(),
            type_: Valid(json::accessor::Type::Vec4),
            min: None,
            max: None,
            name: None,
            normalized: false,
            sparse: None,
        }))
    } else {
        None
    };

    let mut material_index_map: BTreeMap<i32, json::Index<json::Material>> = BTreeMap::new();
    let mut primitives = Vec::new();
    for (material_id, offset, count) in index_blocks {
        let material_index = *material_index_map.entry(material_id).or_insert_with(|| {
            let descriptor = mesh_data
                .material_descriptors
                .get(material_id as usize)
                .cloned()
                .unwrap_or_else(|| default_material_descriptor(format!("material_{}", material_id)));
            root.push(build_json_material(&descriptor, &texture_index_map))
        });

        let index_bv = root.push(json::buffer::View {
            buffer,
            byte_length: json::validation::USize64((count * std::mem::size_of::<u32>()) as u64),
            byte_offset: Some(json::validation::USize64(offset as u64)),
            byte_stride: None,
            extensions: Default::default(),
            extras: Default::default(),
            name: None,
            target: Some(Valid(json::buffer::Target::ElementArrayBuffer)),
        });

        let indices_acc = root.push(json::Accessor {
            buffer_view: Some(index_bv),
            byte_offset: Some(json::validation::USize64(0)),
            count: json::validation::USize64(count as u64),
            component_type: Valid(json::accessor::GenericComponentType(
                json::accessor::ComponentType::U32,
            )),
            extensions: Default::default(),
            extras: Default::default(),
            type_: Valid(json::accessor::Type::Scalar),
            min: None,
            max: None,
            name: None,
            normalized: false,
            sparse: None,
        });

        let mut attributes = std::collections::BTreeMap::new();
        attributes.insert(Valid(json::mesh::Semantic::Positions), positions);
        if let Some(normals_accessor) = normals_accessor {
            attributes.insert(Valid(json::mesh::Semantic::Normals), normals_accessor);
        }
        if let Some(texcoords_accessor) = texcoords_accessor {
            attributes.insert(
                Valid(json::mesh::Semantic::TexCoords(0)),
                texcoords_accessor,
            );
        }
        if let Some(colors_accessor) = colors_accessor {
            attributes.insert(Valid(json::mesh::Semantic::Colors(0)), colors_accessor);
        }

        primitives.push(json::mesh::Primitive {
            attributes,
            extensions: Default::default(),
            extras: Default::default(),
            indices: Some(indices_acc),
            material: Some(material_index),
            mode: Valid(json::mesh::Mode::Triangles),
            targets: None,
        });
    }

    let mesh = root.push(json::Mesh {
        extensions: Default::default(),
        extras: Default::default(),
        name: None,
        primitives,
        weights: None,
    });

    let node = root.push(json::Node {
        mesh: Some(mesh),
        ..Default::default()
    });

    root.push(json::Scene {
        extensions: Default::default(),
        extras: Default::default(),
        name: None,
        nodes: vec![node],
    });

    let json_string = json::serialize::to_string(&root)?;
    let mut json_offset = json_string.len();
    while json_offset % 4 != 0 {
        json_offset += 1;
    }

    let mut glb = Vec::new();
    glb.extend_from_slice(b"glTF");
    glb.extend_from_slice(&2u32.to_le_bytes());

    let total_length = 12 + 8 + json_offset + 8 + buffer_data.len();
    glb.extend_from_slice(&(total_length as u32).to_le_bytes());

    // JSON Chunk
    glb.extend_from_slice(&(json_offset as u32).to_le_bytes());
    glb.extend_from_slice(b"JSON");
    glb.extend_from_slice(json_string.as_bytes());
    while glb.len() % (12 + 8 + json_offset) < 12 + 8 + json_offset && glb.len() % 4 != 0 {
        glb.push(0x20);
    }

    // Binary Chunk
    glb.extend_from_slice(&(buffer_data.len() as u32).to_le_bytes());
    glb.extend_from_slice(b"BIN\0");
    glb.extend_from_slice(&buffer_data);

    let mut file = File::create(path)?;
    file.write_all(&glb)?;

    Ok(())
}

use super::progress::{CliProgressGuard, CliProgressScope};
use super::SceneArgs;
use crate::scene::{
    qem_scene_graph_compute_decisions, qem_scene_graph_simplify_ex,
    QemSceneExecutionOptions, QemSceneGraphMeshBindingView, QemSceneGraphNodeView,
    QemSceneGraphView, QemSceneMeshDecision, QemSceneMeshResult, QemSceneMeshView,
    QemScenePolicy, QemSceneSimplifyResult,
};
use crate::{
    qem_context_create, qem_context_destroy, QemMeshView, QemSimplifyOptions, QEM_STATUS_SUCCESS,
};
use console::Term;
use gltf_json as json;
use json::validation::Checked::Valid;
use supports_unicode::Stream;
use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::Path;

const PREVIEW_MAX_MESH_DECISIONS: usize = 24;
const PREVIEW_MAX_SCENE_NODES: usize = 200;
const SCENE_ATTR_NORMAL: u32 = 3;
const SCENE_ATTR_UV0: u32 = 2;
const SCENE_ATTR_COLOR0: u32 = 4;
const SCENE_NUM_ATTRIBUTES: u32 = SCENE_ATTR_NORMAL + SCENE_ATTR_UV0 + SCENE_ATTR_COLOR0;

#[derive(Debug, Default)]
struct SceneMeshData {
    name: Option<String>,
    vertices: Vec<f32>,
    indices: Vec<u32>,
    material_ids: Vec<i32>,
    num_attributes: u32,
    attribute_weights: Vec<f32>,
}

#[derive(Debug, Default)]
struct SceneExportMetadata {
    material_descriptors: Vec<MaterialDescriptor>,
    node_names: Vec<Option<String>>,
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

fn mesh_stride(num_attributes: u32) -> usize {
    (3 + num_attributes) as usize
}

fn default_attribute_weights(num_attributes: u32) -> Vec<f32> {
    if num_attributes == 0 {
        Vec::new()
    } else {
        vec![1.0; num_attributes as usize]
    }
}

fn default_material_descriptor(name: String) -> MaterialDescriptor {
    MaterialDescriptor {
        name,
        ..Default::default()
    }
}

fn clamp_unit_or(value: f32, default: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        default
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
        metallic_factor: clamp_unit_or(pbr.metallic_factor(), 1.0),
        roughness_factor: clamp_unit_or(pbr.roughness_factor(), 1.0),
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

fn print_scene_input_diagnostics(
    mesh_data: &[SceneMeshData],
    scene_nodes: &[QemSceneGraphNodeView],
    mesh_bindings: &[QemSceneGraphMeshBindingView],
    root_node: i32,
) {
    let mut invalid_mesh_count = 0usize;
    let mut invalid_mesh_samples = Vec::new();
    let mut total_source_triangles = 0u64;

    for (mesh_index, mesh) in mesh_data.iter().enumerate() {
        let tri_count = mesh.indices.len() / 3;
        total_source_triangles += tri_count as u64;

        let stride = mesh_stride(mesh.num_attributes);

        let valid = !mesh.vertices.is_empty()
            && mesh.vertices.len() % stride == 0
            && !mesh.indices.is_empty()
            && mesh.indices.len() % 3 == 0
            && mesh.material_ids.len() == tri_count;

        if !valid {
            invalid_mesh_count += 1;
            if invalid_mesh_samples.len() < 8 {
                invalid_mesh_samples.push(format!(
                    "mesh[{}]: vertices={}, stride={}, indices={}, material_ids={}",
                    mesh_index,
                    mesh.vertices.len(),
                    stride,
                    mesh.indices.len(),
                    mesh.material_ids.len()
                ));
            }
        }
    }

    let mut invalid_parent_count = 0usize;
    let mut invalid_binding_mesh_ref_count = 0usize;
    let mut invalid_binding_node_ref_count = 0usize;
    let mut invalid_node_samples = Vec::new();

    for (node_index, node) in scene_nodes.iter().enumerate() {
        let invalid_parent =
            node.parent_index >= 0 && (node.parent_index as usize) >= scene_nodes.len();
        if invalid_parent {
            invalid_parent_count += 1;
            if invalid_node_samples.len() < 8 {
                invalid_node_samples.push(format!(
                    "node[{}]: parent_index={}",
                    node_index, node.parent_index
                ));
            }
        }
    }

    for binding in mesh_bindings {
        let invalid_node = (binding.node_index as usize) >= scene_nodes.len();
        let invalid_mesh = (binding.mesh_index as usize) >= mesh_data.len();

        if invalid_node {
            invalid_binding_node_ref_count += 1;
        }
        if invalid_mesh {
            invalid_binding_mesh_ref_count += 1;
        }

        if (invalid_node || invalid_mesh) && invalid_node_samples.len() < 8 {
            invalid_node_samples.push(format!(
                "binding[node={}, mesh={}]",
                binding.node_index, binding.mesh_index
            ));
        }
    }

    println!(
        "Scene input diagnostics: meshes={}, nodes={}, bindings={}, root_node={}, source_triangles={}",
        mesh_data.len(),
        scene_nodes.len(),
        mesh_bindings.len(),
        root_node,
        total_source_triangles
    );
    println!(
        "  Invalid meshes: {}, invalid node parents: {}, invalid binding node refs: {}, invalid binding mesh refs: {}",
        invalid_mesh_count,
        invalid_parent_count,
        invalid_binding_node_ref_count,
        invalid_binding_mesh_ref_count
    );

    if !invalid_mesh_samples.is_empty() {
        println!("  Invalid mesh samples:");
        for sample in invalid_mesh_samples {
            println!("    - {}", sample);
        }
    }

    if !invalid_node_samples.is_empty() {
        println!("  Invalid node/binding samples:");
        for sample in invalid_node_samples {
            println!("    - {}", sample);
        }
    }
}

fn validate_scene_input(
    mesh_data: &[SceneMeshData],
    scene_nodes: &[QemSceneGraphNodeView],
    mesh_bindings: &[QemSceneGraphMeshBindingView],
) -> Result<(), Box<dyn std::error::Error>> {
    for (mesh_index, mesh) in mesh_data.iter().enumerate() {
        let stride = mesh_stride(mesh.num_attributes);

        if mesh.vertices.is_empty() || mesh.vertices.len() % stride != 0 {
            return Err(format!(
                "Invalid mesh[{}]: vertices len={} (expected non-empty and multiple of {})",
                mesh_index,
                mesh.vertices.len(),
                stride
            )
            .into());
        }

        if mesh.indices.is_empty() || mesh.indices.len() % 3 != 0 {
            return Err(format!(
                "Invalid mesh[{}]: indices len={} (expected non-empty triangle list)",
                mesh_index,
                mesh.indices.len()
            )
            .into());
        }

        let triangle_count = mesh.indices.len() / 3;
        if mesh.material_ids.len() != triangle_count {
            return Err(format!(
                "Invalid mesh[{}]: material_ids len={} but triangles={}",
                mesh_index,
                mesh.material_ids.len(),
                triangle_count
            )
            .into());
        }

        if mesh.num_attributes > 0 && mesh.attribute_weights.len() != mesh.num_attributes as usize {
            return Err(format!(
                "Invalid mesh[{}]: attribute_weights len={} but num_attributes={}",
                mesh_index,
                mesh.attribute_weights.len(),
                mesh.num_attributes
            )
            .into());
        }
    }

    for (node_index, node) in scene_nodes.iter().enumerate() {
        if node.parent_index >= 0 && (node.parent_index as usize) >= scene_nodes.len() {
            return Err(format!(
                "Invalid node[{}]: parent_index={} out of range [0, {})",
                node_index,
                node.parent_index,
                scene_nodes.len()
            )
            .into());
        }
    }

    for (binding_index, binding) in mesh_bindings.iter().enumerate() {
        if (binding.node_index as usize) >= scene_nodes.len() {
            return Err(format!(
                "Invalid binding[{}]: node_index={} out of range [0, {})",
                binding_index,
                binding.node_index,
                scene_nodes.len()
            )
            .into());
        }

        if (binding.mesh_index as usize) >= mesh_data.len() {
            return Err(format!(
                "Invalid binding[{}]: mesh_index={} out of range [0, {})",
                binding_index,
                binding.mesh_index,
                mesh_data.len()
            )
            .into());
        }
    }

    Ok(())
}

pub fn handle_scene(args: &SceneArgs, verbose: bool) -> Result<(), Box<dyn std::error::Error>> {
    let input_path = Path::new(&args.input);
    let ext = input_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    let (mut mesh_data, scene_nodes, mesh_bindings, metadata) = match ext.as_str() {
        "glb" | "gltf" => load_glb_scene(input_path)?,
        _ => return Err(format!("Unsupported scene input format: {} (only GLB/GLTF is supported)", ext).into()),
    };

    if verbose {
        println!(
            "Loaded scene: {} meshes, {} nodes, {} bindings",
            mesh_data.len(),
            scene_nodes.len(),
            mesh_bindings.len()
        );
    }

    let mut scene_meshes: Vec<QemSceneMeshView> = Vec::new();
    for (i, mesh_data_item) in mesh_data.iter_mut().enumerate() {
        let stride = mesh_stride(mesh_data_item.num_attributes);
        scene_meshes.push(QemSceneMeshView {
            mesh_id: i as u32,
            mesh: QemMeshView {
                vertices: mesh_data_item.vertices.as_mut_ptr(),
                num_vertices: (mesh_data_item.vertices.len() / stride) as u32,
                indices: mesh_data_item.indices.as_mut_ptr(),
                num_indices: mesh_data_item.indices.len() as u32,
                material_ids: mesh_data_item.material_ids.as_mut_ptr(),
                num_attributes: mesh_data_item.num_attributes,
                attribute_weights: if mesh_data_item.num_attributes == 0 {
                    std::ptr::null()
                } else {
                    mesh_data_item.attribute_weights.as_ptr()
                },
            },
        });
    }

    let root_node = scene_nodes
        .iter()
        .position(|n| n.parent_index < 0)
        .map(|i| i as i32)
        .unwrap_or(-1);

    if verbose {
        print_scene_input_diagnostics(&mesh_data, &scene_nodes, &mesh_bindings, root_node);
    }

    validate_scene_input(&mesh_data, &scene_nodes, &mesh_bindings)?;

    let mut scene_graph = QemSceneGraphView {
        meshes: scene_meshes.as_mut_ptr(),
        num_meshes: scene_meshes.len() as u32,
        nodes: scene_nodes.as_ptr(),
        num_nodes: scene_nodes.len() as u32,
        mesh_bindings: mesh_bindings.as_ptr(),
        num_mesh_bindings: mesh_bindings.len() as u32,
    };

    let policy = QemScenePolicy {
        target_triangle_ratio: args.ratio,
        min_mesh_ratio: args.min_mesh_ratio,
        max_mesh_ratio: args.max_mesh_ratio,
        weight_mode: args.weight_mode,
        use_world_scale: if args.use_world_scale { 1 } else { 0 },
        enable_parallel: if args.enable_parallel { 1 } else { 0 },
        max_parallel_tasks: args.max_parallel_tasks,
        ..Default::default()
    };

    let execution_options = QemSceneExecutionOptions {
        enable_parallel: if args.enable_parallel { 1 } else { 0 },
        max_parallel_tasks: args.max_parallel_tasks,
        retry_count: 1,
        fallback_relaxation_step: 0.15,
    };

    let base_options = QemSimplifyOptions {
        limit_error: f32::INFINITY,
        ..QemSimplifyOptions::default()
    };

    let mut decisions = vec![QemSceneMeshDecision::default(); scene_meshes.len()];
    let mut decision_count = 0u32;
    let mut scene_result = QemSceneSimplifyResult::default();

    if args.dry_run {
        println!("Starting scene decision computation (dry-run)...");
        if verbose {
            println!(
                "  Policy: ratio={:.3}, min_mesh_ratio={:.3}, max_mesh_ratio={:.3}, weight_mode={}, use_world_scale={}",
                args.ratio, args.min_mesh_ratio, args.max_mesh_ratio, args.weight_mode, args.use_world_scale
            );
        }

        let status = unsafe {
            qem_scene_graph_compute_decisions(
                &scene_graph,
                &policy,
                decisions.as_mut_ptr(),
                decisions.len() as u32,
                &mut decision_count,
                &mut scene_result,
            )
        };

        let final_status = if status == QEM_STATUS_SUCCESS {
            scene_result.status
        } else {
            status
        };

        if final_status != QEM_STATUS_SUCCESS {
            return Err(format!(
                "qem_scene_graph_compute_decisions failed. status={}, result_status={}",
                status, scene_result.status
            )
            .into());
        }

        let decision_len = (decision_count as usize).min(decisions.len());
        print_scene_simplification_preview(
            args,
            &scene_nodes,
            &mesh_bindings,
            root_node,
            &decisions[..decision_len],
            &scene_result,
        );

        println!("Dry-run complete: no mesh simplification or output export performed.");
        return Ok(());
    }

    let context = qem_context_create();
    if context.is_null() {
        return Err("Failed to create qem context".into());
    }

    let progress = CliProgressGuard::attach(context, CliProgressScope::Scene, "场景简化")?;

    if verbose {
        println!(
            "Starting scene simplification... parallel={}, max_parallel_tasks={}",
            args.enable_parallel, args.max_parallel_tasks
        );
    }

    let mut mesh_results = vec![QemSceneMeshResult::default(); scene_meshes.len()];
    let status = unsafe {
        qem_scene_graph_simplify_ex(
            context,
            &mut scene_graph,
            &policy,
            &base_options,
            &execution_options,
            decisions.as_mut_ptr(),
            decisions.len() as u32,
            &mut decision_count,
            mesh_results.as_mut_ptr(),
            mesh_results.len() as u32,
            &mut scene_result,
        )
    };

    let final_status = if status == QEM_STATUS_SUCCESS {
        scene_result.status
    } else {
        status
    };

    progress.finish_if_needed(final_status, "场景简化完成", "场景简化失败");
    drop(progress);

    unsafe { qem_context_destroy(context) };

    if final_status != QEM_STATUS_SUCCESS {
        return Err(format!(
            "qem_scene_graph_simplify failed. status={}, result_status={}",
            status, scene_result.status
        )
        .into());
    }

    for (mesh_view, mesh_data_item) in scene_meshes.iter().zip(mesh_data.iter_mut()) {
        let stride = mesh_stride(mesh_data_item.num_attributes);
        let output_vertices = mesh_view.mesh.num_vertices as usize;
        let output_indices = mesh_view.mesh.num_indices as usize;

        mesh_data_item.vertices.truncate(output_vertices * stride);
        mesh_data_item.indices.truncate(output_indices);
        mesh_data_item.material_ids.truncate(output_indices / 3);
    }

    if verbose {
        let decision_len = (decision_count as usize).min(decisions.len());
        print_scene_simplification_preview(
            args,
            &scene_nodes,
            &mesh_bindings,
            root_node,
            &decisions[..decision_len],
            &scene_result,
        );

        println!("Simplification complete:");
        println!("  Source triangles: {}", scene_result.source_triangles);
        println!("  Output triangles: {}", scene_result.output_triangles);
    }

    export_scene_to_glb(
        &args.output,
        &mesh_data,
        &scene_nodes,
        &mesh_bindings,
        &metadata,
    )?;

    Ok(())
}

fn weight_mode_name(weight_mode: u32) -> &'static str {
    match weight_mode {
        0 => "Uniform",
        1 => "Volume",
        2 => "Volume*Instances",
        3 => "External",
        _ => "Unknown",
    }
}

fn format_node_mesh_preview(
    node_index: usize,
    mesh_bindings: &[QemSceneGraphMeshBindingView],
    decisions_by_mesh: &HashMap<i32, QemSceneMeshDecision>,
) -> String {
    let mut bound_mesh_indices = Vec::<i32>::new();
    for binding in mesh_bindings {
        if binding.node_index as usize == node_index {
            bound_mesh_indices.push(binding.mesh_index as i32);
        }
    }

    if bound_mesh_indices.is_empty() {
        return "transform-only".to_string();
    }

    let preview_mesh = bound_mesh_indices[0];
    if let Some(decision) = decisions_by_mesh.get(&preview_mesh) {
        if bound_mesh_indices.len() == 1 {
            return format!(
                "mesh={} src={} target={} keep={:.3}",
                preview_mesh, decision.source_triangles, decision.target_triangles, decision.keep_ratio
            );
        }

        return format!(
            "mesh={} (+{} bindings) src={} target={} keep={:.3}",
            preview_mesh,
            bound_mesh_indices.len() - 1,
            decision.source_triangles,
            decision.target_triangles,
            decision.keep_ratio
        );
    }

    if bound_mesh_indices.len() == 1 {
        format!("mesh={} (no-decision)", preview_mesh)
    } else {
        format!(
            "mesh={} (+{} bindings) (no-decision)",
            preview_mesh,
            bound_mesh_indices.len() - 1
        )
    }
}

fn write_tree_line(line: &str) {
    if Term::stdout().write_line(line).is_err() {
        println!("{}", line);
    }
}

#[derive(Clone, Copy)]
struct TreeGlyphs {
    root: &'static str,
    branch: &'static str,
    last: &'static str,
}

fn tree_glyphs() -> TreeGlyphs {
    if std::io::stdout().is_terminal() && supports_unicode::on(Stream::Stdout) {
        TreeGlyphs {
            root: "● ",
            branch: "├─ ",
            last: "└─ ",
        }
    } else {
        TreeGlyphs {
            root: "* ",
            branch: "|- ",
            last: "`- ",
        }
    }
}

fn print_scene_tree_node(
    node_index: usize,
    nodes: &[QemSceneGraphNodeView],
    mesh_bindings: &[QemSceneGraphMeshBindingView],
    children: &[Vec<usize>],
    decisions_by_mesh: &HashMap<i32, QemSceneMeshDecision>,
    glyphs: TreeGlyphs,
    depth: usize,
    is_last: bool,
    is_root: bool,
    visited: &mut [bool],
    printed: &mut usize,
    max_nodes: usize,
) {
    if *printed >= max_nodes || visited[node_index] {
        return;
    }

    visited[node_index] = true;

    let node = &nodes[node_index];
    let indent = "  ".repeat(depth);
    let branch = if is_root {
        glyphs.root
    } else if is_last {
        glyphs.last
    } else {
        glyphs.branch
    };
    let mesh_preview = format_node_mesh_preview(node_index, mesh_bindings, decisions_by_mesh);

    let line = format!(
        "{}{}[level {} node {} parent={}] {}",
        indent, branch, depth, node_index, node.parent_index, mesh_preview
    );
    write_tree_line(&line);

    *printed += 1;

    for (idx, child_index) in children[node_index].iter().enumerate() {
        let child_is_last = idx + 1 == children[node_index].len();
        print_scene_tree_node(
            *child_index,
            nodes,
            mesh_bindings,
            children,
            decisions_by_mesh,
            glyphs,
            depth + 1,
            child_is_last,
            false,
            visited,
            printed,
            max_nodes,
        );

        if *printed >= max_nodes {
            break;
        }
    }
}

fn print_scene_simplification_preview(
    args: &SceneArgs,
    nodes: &[QemSceneGraphNodeView],
    mesh_bindings: &[QemSceneGraphMeshBindingView],
    root_node: i32,
    decisions: &[QemSceneMeshDecision],
    scene_result: &QemSceneSimplifyResult,
) {
    println!();
    println!("=== Scene Simplification Preview ===");
    println!(
        "Policy: target_ratio={:.3}, min_mesh_ratio={:.3}, max_mesh_ratio={:.3}, weight_mode={}({}), use_world_scale={}",
        args.ratio,
        args.min_mesh_ratio,
        args.max_mesh_ratio,
        args.weight_mode,
        weight_mode_name(args.weight_mode),
        args.use_world_scale
    );
    println!(
        "Result budget: source={} target={} output={} (decisions={})",
        scene_result.source_triangles,
        scene_result.target_triangles,
        scene_result.output_triangles,
        decisions.len()
    );

    println!("Mesh decisions preview:");
    for decision in decisions.iter().take(PREVIEW_MAX_MESH_DECISIONS) {
        println!(
            "  mesh[{:<4}] id={:<6} src={:<8} target={:<8} keep={:.3} weight={:.4}",
            decision.mesh_index,
            decision.mesh_id,
            decision.source_triangles,
            decision.target_triangles,
            decision.keep_ratio,
            decision.importance_weight
        );
    }
    if decisions.len() > PREVIEW_MAX_MESH_DECISIONS {
        println!(
            "  ... {} more mesh decisions omitted",
            decisions.len() - PREVIEW_MAX_MESH_DECISIONS
        );
    }

    println!("Scene tree preview:");
    if nodes.is_empty() {
        println!("  (empty)");
        println!("=== End of Preview ===");
        println!();
        return;
    }

    let mut decisions_by_mesh: HashMap<i32, QemSceneMeshDecision> = HashMap::new();
    for decision in decisions {
        decisions_by_mesh.insert(decision.mesh_index as i32, *decision);
    }

    let mut children = vec![Vec::<usize>::new(); nodes.len()];
    let mut roots = Vec::<usize>::new();
    for (node_index, node) in nodes.iter().enumerate() {
        if node.parent_index >= 0 && (node.parent_index as usize) < nodes.len() {
            children[node.parent_index as usize].push(node_index);
        } else {
            roots.push(node_index);
        }
    }

    let mut ordered_roots = Vec::<usize>::new();
    let mut seen_root = vec![false; nodes.len()];

    if root_node >= 0 {
        let root_index = root_node as usize;
        if root_index < nodes.len() {
            ordered_roots.push(root_index);
            seen_root[root_index] = true;
        }
    }

    for root in roots {
        if !seen_root[root] {
            ordered_roots.push(root);
            seen_root[root] = true;
        }
    }

    if ordered_roots.is_empty() {
        ordered_roots.push(0);
    }

    let mut visited = vec![false; nodes.len()];
    let mut printed = 0usize;
    let glyphs = tree_glyphs();

    for (root_pos, root_index) in ordered_roots.iter().enumerate() {
        if printed >= PREVIEW_MAX_SCENE_NODES {
            break;
        }

        let root_is_last = root_pos + 1 == ordered_roots.len();
        print_scene_tree_node(
            *root_index,
            nodes,
            mesh_bindings,
            &children,
            &decisions_by_mesh,
            glyphs,
            0,
            root_is_last,
            true,
            &mut visited,
            &mut printed,
            PREVIEW_MAX_SCENE_NODES,
        );
    }

    if printed < PREVIEW_MAX_SCENE_NODES {
        for node_index in 0..nodes.len() {
            if visited[node_index] {
                continue;
            }

            print_scene_tree_node(
                node_index,
                nodes,
                mesh_bindings,
                &children,
                &decisions_by_mesh,
                glyphs,
                0,
                true,
                true,
                &mut visited,
                &mut printed,
                PREVIEW_MAX_SCENE_NODES,
            );

            if printed >= PREVIEW_MAX_SCENE_NODES {
                break;
            }
        }
    }

    let omitted = nodes.len().saturating_sub(printed);
    if omitted > 0 {
        println!("  ... {} more scene nodes omitted", omitted);
    }

    println!("=== End of Preview ===");
    println!();
}

fn load_glb_scene(
    path: &Path,
) -> Result<
    (
        Vec<SceneMeshData>,
        Vec<QemSceneGraphNodeView>,
        Vec<QemSceneGraphMeshBindingView>,
        SceneExportMetadata,
    ),
    Box<dyn std::error::Error>,
> {
    let (document, buffers, _) = gltf::import(path)?;

    let mut metadata = SceneExportMetadata::default();
    for material in document.materials() {
        metadata
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

        metadata.image_descriptors.push(descriptor);
    }

    for sampler in document.samplers() {
        metadata.sampler_descriptors.push(SamplerDescriptor {
            mag_filter: sampler.mag_filter(),
            min_filter: sampler.min_filter(),
            wrap_s: sampler.wrap_s(),
            wrap_t: sampler.wrap_t(),
        });
    }

    for texture in document.textures() {
        metadata.texture_descriptors.push(TextureDescriptor {
            source_image_index: texture.source().index(),
            sampler_index: texture.sampler().index(),
        });
    }

    let mut default_material_id: Option<i32> = None;

    let mut mesh_data = Vec::new();
    for mesh in document.meshes() {
        let mut all_v = Vec::new();
        let mut all_i = Vec::new();
        let mut all_material_ids = Vec::new();

        for primitive in mesh.primitives() {
            let reader = primitive.reader(|buffer| Some(&buffers[buffer.index()]));
            let positions: Vec<[f32; 3]> = reader.read_positions().ok_or("No positions")?.collect();
            let indices: Vec<u32> = reader
                .read_indices()
                .ok_or("No indices")?
                .into_u32()
                .collect();

            let normals = reader.read_normals().map(|iter| iter.collect::<Vec<[f32; 3]>>());
            let texcoords = reader
                .read_tex_coords(0)
                .map(|iter| iter.into_f32().collect::<Vec<[f32; 2]>>());
            let colors = reader
                .read_colors(0)
                .map(|iter| iter.into_rgba_f32().collect::<Vec<[f32; 4]>>());

            let offset = (all_v.len() / mesh_stride(SCENE_NUM_ATTRIBUTES)) as u32;
            for (vertex_index, position) in positions.iter().enumerate() {
                let normal = normals
                    .as_ref()
                    .and_then(|n| n.get(vertex_index).copied())
                    .unwrap_or([0.0, 0.0, 1.0]);
                let uv = texcoords
                    .as_ref()
                    .and_then(|uv| uv.get(vertex_index).copied())
                    .unwrap_or([0.0, 0.0]);
                let color = colors
                    .as_ref()
                    .and_then(|c| c.get(vertex_index).copied())
                    .unwrap_or([1.0, 1.0, 1.0, 1.0]);

                all_v.extend_from_slice(position);
                all_v.extend_from_slice(&normal);
                all_v.extend_from_slice(&uv);
                all_v.extend_from_slice(&color);
            }

            let material_id = if let Some(index) = primitive.material().index() {
                index as i32
            } else {
                *default_material_id.get_or_insert_with(|| {
                    ensure_material_descriptor(
                        &mut metadata.material_descriptors,
                        default_material_descriptor("default_material".to_string()),
                    )
                })
            };
            let tri_count = indices.len() / 3;

            for idx in indices {
                all_i.push(idx + offset);
            }

            for _ in 0..tri_count {
                all_material_ids.push(material_id);
            }
        }

        mesh_data.push(SceneMeshData {
            name: mesh.name().map(|s| s.to_string()),
            vertices: all_v,
            indices: all_i,
            material_ids: all_material_ids,
            num_attributes: SCENE_NUM_ATTRIBUTES,
            attribute_weights: default_attribute_weights(SCENE_NUM_ATTRIBUTES),
        });
    }

    let mut scene_nodes = Vec::new();
    let mut mesh_bindings = Vec::new();

    for scene in document.scenes() {
        for node in scene.nodes() {
            process_gltf_node(
                &node,
                -1,
                &mut scene_nodes,
                &mut mesh_bindings,
                &mut metadata.node_names,
            );
        }
    }

    Ok((mesh_data, scene_nodes, mesh_bindings, metadata))
}

fn process_gltf_node(
    node: &gltf::Node,
    parent_index: i32,
    scene_nodes: &mut Vec<QemSceneGraphNodeView>,
    mesh_bindings: &mut Vec<QemSceneGraphMeshBindingView>,
    node_names: &mut Vec<Option<String>>,
) {
    let local_arr = node.transform().matrix();
    let mut local_matrix = [0.0f32; 16];
    for i in 0..4 {
        for j in 0..4 {
            local_matrix[i * 4 + j] = local_arr[i][j];
        }
    }

    let node_index = scene_nodes.len() as u32;

    scene_nodes.push(QemSceneGraphNodeView {
        parent_index,
        local_matrix,
    });

    node_names.push(node.name().map(|s| s.to_string()));

    if let Some(mesh) = node.mesh() {
        mesh_bindings.push(QemSceneGraphMeshBindingView {
            node_index,
            mesh_index: mesh.index() as u32,
            mesh_to_node_matrix: [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ],
            use_mesh_to_node_matrix: 0,
        });
    }

    for child in node.children() {
        process_gltf_node(
            &child,
            node_index as i32,
            scene_nodes,
            mesh_bindings,
            node_names,
        );
    }
}

fn export_scene_to_glb(
    path: &str,
    mesh_data: &[SceneMeshData],
    nodes: &[QemSceneGraphNodeView],
    mesh_bindings: &[QemSceneGraphMeshBindingView],
    metadata: &SceneExportMetadata,
) -> Result<(), Box<dyn std::error::Error>> {
    use gltf_json as json;
    use json::validation::Checked::Valid;
    use std::collections::BTreeMap;
    use std::fs::File;
    use std::io::Write;

    let mut root = json::Root::default();
    let mut buffer_data = Vec::new();

    let mut image_index_map: Vec<Option<json::Index<json::Image>>> =
        vec![None; metadata.image_descriptors.len()];
    for (image_slot, image_descriptor) in metadata.image_descriptors.iter().enumerate() {
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
        vec![None; metadata.sampler_descriptors.len()];
    for (sampler_slot, sampler_descriptor) in metadata.sampler_descriptors.iter().enumerate() {
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
        vec![None; metadata.texture_descriptors.len()];
    for (texture_slot, texture_descriptor) in metadata.texture_descriptors.iter().enumerate() {
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

    let mut mesh_indices_in_gltf = Vec::new();

    let mut material_index_map: BTreeMap<i32, json::Index<json::Material>> = BTreeMap::new();

    for mesh in mesh_data {
        let mesh_stride = mesh_stride(mesh.num_attributes);
        let vertex_count = mesh.vertices.len() / mesh_stride;

        let mut positions = Vec::with_capacity(vertex_count * 3);
        let mut normals = if mesh.num_attributes >= 3 {
            Some(Vec::with_capacity(vertex_count * 3))
        } else {
            None
        };
        let mut texcoords = if mesh.num_attributes >= 5 {
            Some(Vec::with_capacity(vertex_count * 2))
        } else {
            None
        };
        let mut colors = if mesh.num_attributes >= 9 {
            Some(Vec::with_capacity(vertex_count * 4))
        } else {
            None
        };

        for vertex_index in 0..vertex_count {
            let base = vertex_index * mesh_stride;
            positions.extend_from_slice(&mesh.vertices[base..base + 3]);
            if let Some(normals) = &mut normals {
                normals.extend_from_slice(&mesh.vertices[base + 3..base + 6]);
            }
            if let Some(texcoords) = &mut texcoords {
                texcoords.extend_from_slice(&mesh.vertices[base + 6..base + 8]);
            }
            if let Some(colors) = &mut colors {
                colors.extend_from_slice(&mesh.vertices[base + 8..base + 12]);
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

        let mut per_material_indices: BTreeMap<i32, Vec<u32>> = BTreeMap::new();
        let triangle_count = mesh.indices.len() / 3;
        for tri in 0..triangle_count {
            let material_id = mesh.material_ids.get(tri).copied().unwrap_or(0).max(0);
            let tri_start = tri * 3;
            per_material_indices
                .entry(material_id)
                .or_default()
                .extend_from_slice(&mesh.indices[tri_start..tri_start + 3]);
        }

        let buffer_idx = 0;

        let v_bv = root.push(json::buffer::View {
            buffer: json::Index::new(buffer_idx),
            byte_length: json::validation::USize64(positions_bytes.len() as u64),
            byte_offset: Some(json::validation::USize64(positions_offset as u64)),
            byte_stride: Some(json::buffer::Stride(12)),
            extensions: Default::default(),
            extras: Default::default(),
            name: None,
            target: Some(Valid(json::buffer::Target::ArrayBuffer)),
        });

        let pos_acc = root.push(json::Accessor {
            buffer_view: Some(v_bv),
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
                buffer: json::Index::new(buffer_idx),
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
                buffer: json::Index::new(buffer_idx),
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
                buffer: json::Index::new(buffer_idx),
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

        let mut primitives = Vec::new();
        for (material_id, mat_indices) in per_material_indices {
            let index_offset = buffer_data.len();
            for idx in &mat_indices {
                buffer_data.extend_from_slice(&idx.to_le_bytes());
            }

            let material_index = *material_index_map.entry(material_id).or_insert_with(|| {
                let descriptor = metadata
                    .material_descriptors
                    .get(material_id as usize)
                    .cloned()
                    .unwrap_or_else(|| default_material_descriptor(format!("material_{}", material_id)));
                root.push(build_json_material(&descriptor, &texture_index_map))
            });

            let i_bv = root.push(json::buffer::View {
                buffer: json::Index::new(buffer_idx),
                byte_length: json::validation::USize64((mat_indices.len() * std::mem::size_of::<u32>()) as u64),
                byte_offset: Some(json::validation::USize64(index_offset as u64)),
                byte_stride: None,
                extensions: Default::default(),
                extras: Default::default(),
                name: None,
                target: Some(Valid(json::buffer::Target::ElementArrayBuffer)),
            });

            let idx_acc = root.push(json::Accessor {
                buffer_view: Some(i_bv),
                byte_offset: Some(json::validation::USize64(0)),
                count: json::validation::USize64(mat_indices.len() as u64),
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
            attributes.insert(Valid(json::mesh::Semantic::Positions), pos_acc);
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
                indices: Some(idx_acc),
                material: Some(material_index),
                mode: Valid(json::mesh::Mode::Triangles),
                targets: None,
            });
        }

        let mesh = root.push(json::Mesh {
            extensions: Default::default(),
            extras: Default::default(),
            name: mesh.name.clone(),
            primitives,
            weights: None,
        });

        mesh_indices_in_gltf.push(mesh);
    }

    root.buffers.push(json::Buffer {
        byte_length: json::validation::USize64(buffer_data.len() as u64),
        extensions: Default::default(),
        extras: Default::default(),
        name: None,
        uri: None,
    });

    let mut gltf_nodes = Vec::with_capacity(nodes.len());
    for node in nodes {
        gltf_nodes.push(root.push(json::Node {
            mesh: None,
            name: metadata
                .node_names
                .get(gltf_nodes.len())
                .cloned()
                .unwrap_or(None),
            matrix: Some(node.local_matrix),
            ..Default::default()
        }));
    }

    for binding in mesh_bindings {
        let node_idx = binding.node_index as usize;
        let mesh_idx = binding.mesh_index as usize;
        if node_idx < gltf_nodes.len() && mesh_idx < mesh_indices_in_gltf.len() {
            let gltf_node_idx = gltf_nodes[node_idx].value();
            root.nodes[gltf_node_idx].mesh = Some(mesh_indices_in_gltf[mesh_idx]);
        }
    }

    let mut scene_root_nodes = Vec::new();
    for (node_index, node) in nodes.iter().enumerate() {
        let child = gltf_nodes[node_index];
        if node.parent_index >= 0 && (node.parent_index as usize) < nodes.len() {
            let parent = gltf_nodes[node.parent_index as usize];
            let parent_node = &mut root.nodes[parent.value()];
            match &mut parent_node.children {
                Some(children) => children.push(child),
                None => parent_node.children = Some(vec![child]),
            }
        } else {
            scene_root_nodes.push(child);
        }
    }

    root.push(json::Scene {
        extensions: Default::default(),
        extras: Default::default(),
        name: None,
        nodes: scene_root_nodes,
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

    glb.extend_from_slice(&(json_offset as u32).to_le_bytes());
    glb.extend_from_slice(b"JSON");
    glb.extend_from_slice(json_string.as_bytes());
    while glb.len() % (12 + 8 + json_offset) < 12 + 8 + json_offset && glb.len() % 4 != 0 {
        glb.push(0x20);
    }

    glb.extend_from_slice(&(buffer_data.len() as u32).to_le_bytes());
    glb.extend_from_slice(b"BIN\0");
    glb.extend_from_slice(&buffer_data);

    let mut file = File::create(path)?;
    file.write_all(&glb)?;

    println!("Saved simplified scene to {} (GLB format)", path);
    Ok(())
}

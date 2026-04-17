use super::progress::{CliProgressGuard, CliProgressScope};
use super::SceneArgs;
use crate::scene::{
    qem_scene_simplify, QemSceneMeshDecision, QemSceneMeshResult, QemSceneMeshView,
    QemSceneNodeView, QemScenePolicy, QemSceneSimplifyResult, QemSceneView,
};
use crate::{
    qem_context_create, qem_context_destroy, QemMeshView, QemSimplifyOptions, QEM_STATUS_SUCCESS,
};
use glam::Mat4;
use std::collections::HashMap;
use std::path::Path;
use ufbx::{load_file as load_fbx, LoadOpts as FbxOpts};

pub fn handle_scene(args: &SceneArgs) -> Result<(), Box<dyn std::error::Error>> {
    let input_path = Path::new(&args.input);
    let ext = input_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    let (mut mesh_data, scene_nodes) = match ext.as_str() {
        "fbx" => load_fbx_scene(input_path)?,
        "glb" | "gltf" => load_glb_scene(input_path)?,
        _ => return Err(format!("Unsupported scene input format: {}", ext).into()),
    };

    println!(
        "Loaded scene: {} meshes, {} nodes",
        mesh_data.len(),
        scene_nodes.len()
    );

    // Prepare views
    let mut scene_meshes: Vec<QemSceneMeshView> = Vec::new();
    for (i, (v, idx, mat)) in mesh_data.iter_mut().enumerate() {
        scene_meshes.push(QemSceneMeshView {
            mesh_id: i as u32,
            mesh: QemMeshView {
                vertices: v.as_mut_ptr(),
                num_vertices: (v.len() / 3) as u32,
                indices: idx.as_mut_ptr(),
                num_indices: idx.len() as u32,
                material_ids: mat.as_mut_ptr(),
                num_attributes: 0,
                attribute_weights: std::ptr::null(),
            },
        });
    }

    let root_node = scene_nodes
        .iter()
        .position(|n| n.parent_index < 0)
        .map(|i| i as i32)
        .unwrap_or(-1);

    let mut scene_view = QemSceneView {
        meshes: scene_meshes.as_mut_ptr(),
        num_meshes: scene_meshes.len() as u32,
        nodes: scene_nodes.as_ptr(),
        num_nodes: scene_nodes.len() as u32,
        root_node,
    };

    // Simplify Scene
    let context = qem_context_create();
    if context.is_null() {
        return Err("Failed to create qem context".into());
    }

    let progress = CliProgressGuard::attach(context, CliProgressScope::Scene, "场景简化")?;

    let policy = QemScenePolicy {
        target_triangle_ratio: args.ratio,
        min_mesh_ratio: args.min_mesh_ratio,
        max_mesh_ratio: args.max_mesh_ratio,
        weight_mode: args.weight_mode,
        use_world_scale: if args.use_world_scale { 1 } else { 0 },
        ..Default::default()
    };

    let base_options = QemSimplifyOptions::default();

    let mut decisions = vec![QemSceneMeshDecision::default(); scene_meshes.len()];
    let mut decision_count = 0u32;
    let mut mesh_results = vec![QemSceneMeshResult::default(); scene_meshes.len()];
    let mut scene_result = QemSceneSimplifyResult::default();

    println!("Starting scene simplification...");
    let status = unsafe {
        qem_scene_simplify(
            context,
            &mut scene_view,
            &policy,
            &base_options,
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
            "qem_scene_simplify failed. status={}, result_status={}",
            status, scene_result.status
        )
        .into());
    }

    println!("Simplification complete:");
    println!("  Source triangles: {}", scene_result.source_triangles);
    println!("  Output triangles: {}", scene_result.output_triangles);

    // Export to GLB
    export_scene_to_glb(&args.output, &mesh_data, &scene_nodes)?;

    Ok(())
}

fn load_fbx_scene(
    path: &Path,
) -> Result<(Vec<(Vec<f32>, Vec<u32>, Vec<i32>)>, Vec<QemSceneNodeView>), Box<dyn std::error::Error>>
{
    let input_str = path.to_str().ok_or("Invalid input path")?;
    let fbx_scene = load_fbx(input_str, FbxOpts::default())
        .map_err(|e| format!("Failed to load FBX: {:?}", e))?;

    let mut mesh_data = Vec::new();
    let mut mesh_index_by_typed_id: HashMap<u32, i32> = HashMap::new();
    for (mesh_index, fbx_mesh) in fbx_scene.meshes.iter().enumerate() {
        let vertices: Vec<f32> = fbx_mesh
            .vertices
            .iter()
            .flat_map(|v| [v.x as f32, v.y as f32, v.z as f32])
            .collect();
        let mut indices = Vec::new();
        for face in &fbx_mesh.faces {
            if face.num_indices == 3 {
                for j in 0..3 {
                    let idx = fbx_mesh.vertex_indices[(face.index_begin + j) as usize];
                    indices.push(idx as u32);
                }
            }
        }
        let material_ids = vec![0i32; indices.len() / 3];
        mesh_data.push((vertices, indices, material_ids));
        mesh_index_by_typed_id.insert(fbx_mesh.element.typed_id as u32, mesh_index as i32);
    }

    let mut node_index_by_typed_id: HashMap<u32, i32> = HashMap::new();
    for (node_index, fbx_node) in fbx_scene.nodes.iter().enumerate() {
        node_index_by_typed_id.insert(fbx_node.element.typed_id as u32, node_index as i32);
    }

    let mut scene_nodes = Vec::new();
    for fbx_node in &fbx_scene.nodes {
        let m = &fbx_node.node_to_world;
        let world_matrix = [
            m.m00 as f32,
            m.m10 as f32,
            m.m20 as f32,
            0.0,
            m.m01 as f32,
            m.m11 as f32,
            m.m21 as f32,
            0.0,
            m.m02 as f32,
            m.m12 as f32,
            m.m22 as f32,
            0.0,
            m.m03 as f32,
            m.m13 as f32,
            m.m23 as f32,
            1.0,
        ];

        let parent_index = fbx_node
            .parent
            .as_ref()
            .and_then(|parent| {
                node_index_by_typed_id
                    .get(&(parent.element.typed_id as u32))
                    .copied()
            })
            .unwrap_or(-1);

        let mesh_index = fbx_node
            .mesh
            .as_ref()
            .and_then(|fbx_mesh| {
                mesh_index_by_typed_id
                    .get(&(fbx_mesh.element.typed_id as u32))
                    .copied()
            })
            .unwrap_or(-1);

        scene_nodes.push(QemSceneNodeView {
            parent_index,
            mesh_index,
            world_matrix,
        });
    }

    Ok((mesh_data, scene_nodes))
}

fn load_glb_scene(
    path: &Path,
) -> Result<(Vec<(Vec<f32>, Vec<u32>, Vec<i32>)>, Vec<QemSceneNodeView>), Box<dyn std::error::Error>>
{
    let (document, buffers, _) = gltf::import(path)?;

    let mut mesh_data = Vec::new();
    // gltf-rs uses meshes separately. We'll map each mesh in glTF to a mesh in our system.
    for mesh in document.meshes() {
        let mut all_v = Vec::new();
        let mut all_i = Vec::new();

        for primitive in mesh.primitives() {
            let reader = primitive.reader(|buffer| Some(&buffers[buffer.index()]));
            let positions: Vec<f32> = reader
                .read_positions()
                .ok_or("No positions")?
                .flatten()
                .collect();
            let indices: Vec<u32> = reader
                .read_indices()
                .ok_or("No indices")?
                .into_u32()
                .collect();

            let offset = (all_v.len() / 3) as u32;
            all_v.extend(positions);
            for idx in indices {
                all_i.push(idx + offset);
            }
        }
        let material_ids = vec![0i32; all_i.len() / 3];
        mesh_data.push((all_v, all_i, material_ids));
    }

    let mut scene_nodes = Vec::new();
    for scene in document.scenes() {
        for node in scene.nodes() {
            process_gltf_node(&node, -1, Mat4::IDENTITY, &mut scene_nodes);
        }
    }

    Ok((mesh_data, scene_nodes))
}

fn process_gltf_node(
    node: &gltf::Node,
    parent_index: i32,
    parent_world: Mat4,
    scene_nodes: &mut Vec<QemSceneNodeView>,
) {
    let local = Mat4::from_cols_array_2d(&node.transform().matrix());
    let world = parent_world * local;

    let node_index = scene_nodes.len() as i32;
    let mut world_matrix = [0.0f32; 16];
    world_matrix.copy_from_slice(&world.to_cols_array());

    scene_nodes.push(QemSceneNodeView {
        parent_index,
        mesh_index: node.mesh().map(|mesh| mesh.index() as i32).unwrap_or(-1),
        world_matrix,
    });

    for child in node.children() {
        process_gltf_node(&child, node_index, world, scene_nodes);
    }
}

fn export_scene_to_glb(
    path: &str,
    mesh_data: &[(Vec<f32>, Vec<u32>, Vec<i32>)],
    nodes: &[QemSceneNodeView],
) -> Result<(), Box<dyn std::error::Error>> {
    use gltf_json as json;
    use json::validation::Checked::Valid;
    use std::fs::File;
    use std::io::Write;

    let mut root = json::Root::default();
    let mut buffer_data = Vec::new();

    let mut mesh_indices_in_gltf = Vec::new();

    // 1. Create Meshes and Accessors
    for (vertices, indices, _) in mesh_data {
        let v_offset = buffer_data.len();
        for &v in vertices {
            buffer_data.extend_from_slice(&v.to_le_bytes());
        }
        let i_offset = buffer_data.len();
        for &idx in indices {
            buffer_data.extend_from_slice(&idx.to_le_bytes());
        }

        let buffer_idx = 0;

        let v_bv = root.push(json::buffer::View {
            buffer: json::Index::new(buffer_idx),
            byte_length: json::validation::USize64((i_offset - v_offset) as u64),
            byte_offset: Some(json::validation::USize64(v_offset as u64)),
            byte_stride: Some(json::buffer::Stride(12)),
            extensions: Default::default(),
            extras: Default::default(),
            name: None,
            target: Some(Valid(json::buffer::Target::ArrayBuffer)),
        });

        let i_bv = root.push(json::buffer::View {
            buffer: json::Index::new(buffer_idx),
            byte_length: json::validation::USize64((buffer_data.len() - i_offset) as u64),
            byte_offset: Some(json::validation::USize64(i_offset as u64)),
            byte_stride: None,
            extensions: Default::default(),
            extras: Default::default(),
            name: None,
            target: Some(Valid(json::buffer::Target::ElementArrayBuffer)),
        });

        let pos_acc = root.push(json::Accessor {
            buffer_view: Some(v_bv),
            byte_offset: Some(json::validation::USize64(0)),
            count: json::validation::USize64((vertices.len() / 3) as u64),
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

        let idx_acc = root.push(json::Accessor {
            buffer_view: Some(i_bv),
            byte_offset: Some(json::validation::USize64(0)),
            count: json::validation::USize64(indices.len() as u64),
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

        let primitive = json::mesh::Primitive {
            attributes: {
                let mut map = std::collections::BTreeMap::new();
                map.insert(Valid(json::mesh::Semantic::Positions), pos_acc);
                map
            },
            extensions: Default::default(),
            extras: Default::default(),
            indices: Some(idx_acc),
            material: None,
            mode: Valid(json::mesh::Mode::Triangles),
            targets: None,
        };

        let mesh = root.push(json::Mesh {
            extensions: Default::default(),
            extras: Default::default(),
            name: None,
            primitives: vec![primitive],
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

    // 2. Create Nodes (preserve hierarchy)
    let world_mats: Vec<Mat4> = nodes
        .iter()
        .map(|n| Mat4::from_cols_array(&n.world_matrix))
        .collect();

    let mut local_mats = vec![[0.0f32; 16]; nodes.len()];
    for (node_index, node) in nodes.iter().enumerate() {
        let world = world_mats[node_index];
        let local = if node.parent_index >= 0 && (node.parent_index as usize) < nodes.len() {
            let parent_world = world_mats[node.parent_index as usize];
            if parent_world.determinant().abs() > 1.0e-8 {
                parent_world.inverse() * world
            } else {
                world
            }
        } else {
            world
        };
        local_mats[node_index] = local.to_cols_array();
    }

    let mut gltf_nodes = Vec::with_capacity(nodes.len());
    for (node_index, node) in nodes.iter().enumerate() {
        let mesh = if node.mesh_index >= 0 {
            let mesh_index = node.mesh_index as usize;
            if mesh_index < mesh_indices_in_gltf.len() {
                Some(mesh_indices_in_gltf[mesh_index])
            } else {
                None
            }
        } else {
            None
        };

        let gltf_node = root.push(json::Node {
            mesh,
            matrix: Some(local_mats[node_index]),
            ..Default::default()
        });
        gltf_nodes.push(gltf_node);
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

    // 3. Write GLB
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

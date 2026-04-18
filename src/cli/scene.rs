use super::progress::{CliProgressGuard, CliProgressScope};
use super::SceneArgs;
use crate::scene::{
    qem_scene_graph_compute_decisions, qem_scene_graph_simplify, QemSceneGraphMeshBindingView,
    QemSceneGraphNodeView, QemSceneGraphView, QemSceneMeshDecision, QemSceneMeshResult,
    QemSceneMeshView, QemScenePolicy, QemSceneSimplifyResult,
};
use crate::{
    qem_context_create, qem_context_destroy, QemMeshView, QemSimplifyOptions, QEM_STATUS_SUCCESS,
};
use console::Term;
use glam::Mat4;
use supports_unicode::Stream;
use std::collections::HashMap;
use std::io::IsTerminal;
use std::path::Path;
use ufbx::{load_file as load_fbx, LoadOpts as FbxOpts};

const PREVIEW_MAX_MESH_DECISIONS: usize = 24;
const PREVIEW_MAX_SCENE_NODES: usize = 200;

fn print_scene_input_diagnostics(
    mesh_data: &[(Vec<f32>, Vec<u32>, Vec<i32>)],
    scene_nodes: &[QemSceneGraphNodeView],
    mesh_bindings: &[QemSceneGraphMeshBindingView],
    root_node: i32,
) {
    let mut invalid_mesh_count = 0usize;
    let mut invalid_mesh_samples = Vec::new();
    let mut total_source_triangles = 0u64;

    for (mesh_index, (vertices, indices, material_ids)) in mesh_data.iter().enumerate() {
        let tri_count = indices.len() / 3;
        total_source_triangles += tri_count as u64;

        let valid = !vertices.is_empty()
            && vertices.len() % 3 == 0
            && !indices.is_empty()
            && indices.len() % 3 == 0
            && material_ids.len() == tri_count;

        if !valid {
            invalid_mesh_count += 1;
            if invalid_mesh_samples.len() < 8 {
                invalid_mesh_samples.push(format!(
                    "mesh[{}]: vertices={}, indices={}, material_ids={}",
                    mesh_index,
                    vertices.len(),
                    indices.len(),
                    material_ids.len()
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
    mesh_data: &[(Vec<f32>, Vec<u32>, Vec<i32>)],
    scene_nodes: &[QemSceneGraphNodeView],
    mesh_bindings: &[QemSceneGraphMeshBindingView],
) -> Result<(), Box<dyn std::error::Error>> {
    for (mesh_index, (vertices, indices, material_ids)) in mesh_data.iter().enumerate() {
        if vertices.is_empty() || vertices.len() % 3 != 0 {
            return Err(format!(
                "Invalid mesh[{}]: vertices len={} (expected non-empty and multiple of 3)",
                mesh_index,
                vertices.len()
            )
            .into());
        }

        if indices.is_empty() || indices.len() % 3 != 0 {
            return Err(format!(
                "Invalid mesh[{}]: indices len={} (expected non-empty triangle list)",
                mesh_index,
                indices.len()
            )
            .into());
        }

        let triangle_count = indices.len() / 3;
        if material_ids.len() != triangle_count {
            return Err(format!(
                "Invalid mesh[{}]: material_ids len={} but triangles={}",
                mesh_index,
                material_ids.len(),
                triangle_count
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

    let (mut mesh_data, scene_nodes, mesh_bindings) = match ext.as_str() {
        "fbx" => load_fbx_scene(input_path, verbose)?,
        "glb" | "gltf" => load_glb_scene(input_path)?,
        _ => return Err(format!("Unsupported scene input format: {}", ext).into()),
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
        ..Default::default()
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
        println!("Starting scene simplification...");
    }

    let mut mesh_results = vec![QemSceneMeshResult::default(); scene_meshes.len()];
    let status = unsafe {
        qem_scene_graph_simplify(
            context,
            &mut scene_graph,
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
            "qem_scene_graph_simplify failed. status={}, result_status={}",
            status, scene_result.status
        )
        .into());
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

    export_scene_to_glb(&args.output, &mesh_data, &scene_nodes, &mesh_bindings)?;

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

fn load_fbx_scene(
    path: &Path,
    verbose: bool,
) -> Result<
    (
        Vec<(Vec<f32>, Vec<u32>, Vec<i32>)>,
        Vec<QemSceneGraphNodeView>,
        Vec<QemSceneGraphMeshBindingView>,
    ),
    Box<dyn std::error::Error>,
> {
    let input_str = path.to_str().ok_or("Invalid input path")?;
    let fbx_scene = load_fbx(input_str, FbxOpts::default())
        .map_err(|e| format!("Failed to load FBX: {:?}", e))?;

    let mut mesh_data = Vec::new();
    let mut mesh_index_by_typed_id: HashMap<u32, i32> = HashMap::new();
    let mut triangulated_faces = 0usize;
    let mut skipped_faces_invalid_index = 0usize;
    let mut skipped_meshes_empty_triangles = 0usize;

    for fbx_mesh in &fbx_scene.meshes {
        let vertices: Vec<f32> = fbx_mesh
            .vertices
            .iter()
            .flat_map(|v| [v.x as f32, v.y as f32, v.z as f32])
            .collect();

        let mut indices = Vec::new();
        for face in &fbx_mesh.faces {
            if face.num_indices < 3 {
                continue;
            }

            let mut face_indices = Vec::with_capacity(face.num_indices as usize);
            let mut invalid_face = false;

            for j in 0..face.num_indices {
                let raw_idx = fbx_mesh.vertex_indices[(face.index_begin + j) as usize];
                if (raw_idx as usize) >= fbx_mesh.vertices.len() {
                    invalid_face = true;
                    break;
                }
                face_indices.push(raw_idx as u32);
            }

            if invalid_face {
                skipped_faces_invalid_index += 1;
                continue;
            }

            if face_indices.len() == 3 {
                indices.extend_from_slice(&face_indices);
            } else {
                for j in 1..(face_indices.len() - 1) {
                    indices.push(face_indices[0]);
                    indices.push(face_indices[j]);
                    indices.push(face_indices[j + 1]);
                }
                triangulated_faces += 1;
            }
        }

        if indices.is_empty() {
            skipped_meshes_empty_triangles += 1;
            continue;
        }

        let material_ids = vec![0i32; indices.len() / 3];
        let simplified_mesh_index = mesh_data.len() as i32;
        mesh_data.push((vertices, indices, material_ids));
        mesh_index_by_typed_id.insert(fbx_mesh.element.typed_id as u32, simplified_mesh_index);
    }

    if verbose {
        println!(
            "FBX import diagnostics: source_meshes={}, kept_meshes={}, skipped_empty={}, triangulated_faces={}, skipped_invalid_faces={}",
            fbx_scene.meshes.len(),
            mesh_data.len(),
            skipped_meshes_empty_triangles,
            triangulated_faces,
            skipped_faces_invalid_index
        );
    }

    let mut node_index_by_typed_id: HashMap<u32, i32> = HashMap::new();
    for (node_index, fbx_node) in fbx_scene.nodes.iter().enumerate() {
        node_index_by_typed_id.insert(fbx_node.element.typed_id as u32, node_index as i32);
    }

    let mut scene_nodes = Vec::new();
    let mut mesh_bindings = Vec::new();

    let mut world_mats = Vec::new();
    for fbx_node in &fbx_scene.nodes {
        let m = &fbx_node.node_to_world;
        world_mats.push(Mat4::from_cols_array(&[
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
        ]));
    }

    for (node_index, fbx_node) in fbx_scene.nodes.iter().enumerate() {
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

        let world = world_mats[node_index];
        let local = if parent_index >= 0 && (parent_index as usize) < world_mats.len() {
            let parent_world = world_mats[parent_index as usize];
            if parent_world.determinant().abs() > 1.0e-8 {
                parent_world.inverse() * world
            } else {
                world
            }
        } else {
            world
        };

        scene_nodes.push(QemSceneGraphNodeView {
            parent_index,
            local_matrix: local.to_cols_array(),
        });

        if mesh_index >= 0 {
            mesh_bindings.push(QemSceneGraphMeshBindingView {
                node_index: node_index as u32,
                mesh_index: mesh_index as u32,
                mesh_to_node_matrix: [
                    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0,
                    1.0,
                ],
                use_mesh_to_node_matrix: 0,
            });
        }
    }

    Ok((mesh_data, scene_nodes, mesh_bindings))
}

fn load_glb_scene(
    path: &Path,
) -> Result<
    (
        Vec<(Vec<f32>, Vec<u32>, Vec<i32>)>,
        Vec<QemSceneGraphNodeView>,
        Vec<QemSceneGraphMeshBindingView>,
    ),
    Box<dyn std::error::Error>,
> {
    let (document, buffers, _) = gltf::import(path)?;

    let mut mesh_data = Vec::new();
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
    let mut mesh_bindings = Vec::new();

    for scene in document.scenes() {
        for node in scene.nodes() {
            process_gltf_node(&node, -1, &mut scene_nodes, &mut mesh_bindings);
        }
    }

    Ok((mesh_data, scene_nodes, mesh_bindings))
}

fn process_gltf_node(
    node: &gltf::Node,
    parent_index: i32,
    scene_nodes: &mut Vec<QemSceneGraphNodeView>,
    mesh_bindings: &mut Vec<QemSceneGraphMeshBindingView>,
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
        process_gltf_node(&child, node_index as i32, scene_nodes, mesh_bindings);
    }
}

fn export_scene_to_glb(
    path: &str,
    mesh_data: &[(Vec<f32>, Vec<u32>, Vec<i32>)],
    nodes: &[QemSceneGraphNodeView],
    mesh_bindings: &[QemSceneGraphMeshBindingView],
) -> Result<(), Box<dyn std::error::Error>> {
    use gltf_json as json;
    use json::validation::Checked::Valid;
    use std::fs::File;
    use std::io::Write;

    let mut root = json::Root::default();
    let mut buffer_data = Vec::new();

    let mut mesh_indices_in_gltf = Vec::new();

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

    let mut gltf_nodes = Vec::with_capacity(nodes.len());
    for node in nodes {
        gltf_nodes.push(root.push(json::Node {
            mesh: None,
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

use super::progress::{CliProgressGuard, CliProgressScope};
use super::ModelArgs;
use crate::{
    qem_context_create, qem_context_destroy, qem_simplify, QemMeshView, QemSimplifyOptions,
    QemSimplifyResult, QEM_STATUS_SUCCESS,
};
use gltf_json as json;
use json::validation::Checked::Valid;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

pub fn handle_model(args: &ModelArgs, _verbose: bool) -> Result<(), Box<dyn std::error::Error>> {
    let input_path = Path::new(&args.input);
    let ext = input_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    let (mut vertices, mut indices, mut material_ids) = match ext.as_str() {
        "obj" => load_obj(input_path)?,
        "glb" | "gltf" => load_glb(input_path)?,
        _ => return Err(format!("Unsupported input format: {}", ext).into()),
    };

    let settings = args.to_simplify_options(indices.len() as u32 / 3);
    println!(
        "Simplifying mesh: {} -> {} triangles",
        indices.len() / 3,
        settings.target_triangles
    );

    simplify_mesh(&mut vertices, &mut indices, &mut material_ids, settings)?;

    let output_path = Path::new(&args.output);
    let out_ext = output_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    match out_ext.as_str() {
        "obj" => save_obj(output_path, &vertices, &indices)?,
        "glb" => save_glb(output_path, &vertices, &indices)?,
        _ => {
            println!(
                "Defaulting to OBJ output for unknown extension: {}",
                out_ext
            );
            save_obj(output_path, &vertices, &indices)?;
        }
    }

    println!("Saved simplified mesh to {}", args.output);
    Ok(())
}

fn simplify_mesh(
    vertices: &mut Vec<f32>,
    indices: &mut Vec<u32>,
    material_ids: &mut Vec<i32>,
    settings: QemSimplifyOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    let context = qem_context_create();
    if context.is_null() {
        return Err("Failed to create qem context".into());
    }

    let progress = CliProgressGuard::attach(context, CliProgressScope::Mesh, "网格简化")?;

    let mut mesh = QemMeshView {
        vertices: vertices.as_mut_ptr(),
        num_vertices: (vertices.len() / 3) as u32,
        indices: indices.as_mut_ptr(),
        num_indices: indices.len() as u32,
        material_ids: material_ids.as_mut_ptr(),
        num_attributes: 0,
        attribute_weights: std::ptr::null(),
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

    vertices.truncate(result.num_vertices as usize * 3);
    indices.truncate(result.num_indices as usize);
    material_ids.truncate(result.num_triangles as usize);
    Ok(())
}

fn load_obj(path: &Path) -> Result<(Vec<f32>, Vec<u32>, Vec<i32>), Box<dyn std::error::Error>> {
    let (models, _) = tobj::load_obj(path, &tobj::GPU_LOAD_OPTIONS)?;
    let mut all_vertices = Vec::new();
    let mut all_indices = Vec::new();
    let mut all_material_ids = Vec::new();

    for model in models {
        let mesh = model.mesh;
        let offset = (all_vertices.len() / 3) as u32;
        all_vertices.extend(mesh.positions);

        let num_tris = mesh.indices.len() / 3;
        for idx in mesh.indices {
            all_indices.push(idx + offset);
        }
        let material_id = mesh.material_id.unwrap_or(0) as i32;
        for _ in 0..num_tris {
            all_material_ids.push(material_id);
        }
    }
    Ok((all_vertices, all_indices, all_material_ids))
}

fn save_obj(
    path: &Path,
    vertices: &[f32],
    indices: &[u32],
) -> Result<(), Box<dyn std::error::Error>> {
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);

    for v in vertices.chunks(3) {
        writeln!(writer, "v {} {} {}", v[0], v[1], v[2])?;
    }
    for i in indices.chunks(3) {
        writeln!(writer, "f {} {} {}", i[0] + 1, i[1] + 1, i[2] + 1)?;
    }
    Ok(())
}

fn load_glb(path: &Path) -> Result<(Vec<f32>, Vec<u32>, Vec<i32>), Box<dyn std::error::Error>> {
    let (document, buffers, _) = gltf::import(path)?;
    let mut all_vertices = Vec::new();
    let mut all_indices = Vec::new();
    let mut all_material_ids = Vec::new();

    for mesh in document.meshes() {
        for primitive in mesh.primitives() {
            let reader = primitive.reader(|buffer| Some(&buffers[buffer.index()]));

            let vertices: Vec<f32> = match reader.read_positions() {
                Some(iter) => iter.flatten().collect(),
                None => continue,
            };
            let indices: Vec<u32> = match reader.read_indices() {
                Some(read) => read.into_u32().collect(),
                None => continue,
            };

            let offset = (all_vertices.len() / 3) as u32;
            all_vertices.extend(vertices);
            for idx in indices {
                all_indices.push(idx + offset);
            }

            // For now, we don't have a good way to track materials in model mode, just use 0
            let num_tris = all_indices.len() / 3;
            all_material_ids.resize(num_tris, 0);
        }
    }
    Ok((all_vertices, all_indices, all_material_ids))
}

fn save_glb(
    path: &Path,
    vertices: &[f32],
    indices: &[u32],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut root = json::Root::default();

    let mut vertex_data = Vec::new();
    for &v in vertices {
        vertex_data.extend_from_slice(&v.to_le_bytes());
    }

    let mut index_data = Vec::new();
    for &i in indices {
        index_data.extend_from_slice(&i.to_le_bytes());
    }

    let mut buffer_data = vertex_data;
    let index_offset = buffer_data.len();
    buffer_data.extend_from_slice(&index_data);

    let buffer = root.push(json::Buffer {
        byte_length: json::validation::USize64(buffer_data.len() as u64),
        extensions: Default::default(),
        extras: Default::default(),
        name: None,
        uri: None,
    });

    let vertex_bv = root.push(json::buffer::View {
        buffer,
        byte_length: json::validation::USize64(index_offset as u64),
        byte_offset: None,
        byte_stride: Some(json::buffer::Stride(12)),
        extensions: Default::default(),
        extras: Default::default(),
        name: None,
        target: Some(Valid(json::buffer::Target::ArrayBuffer)),
    });

    let index_bv = root.push(json::buffer::View {
        buffer,
        byte_length: json::validation::USize64(index_data.len() as u64),
        byte_offset: Some(json::validation::USize64(index_offset as u64)),
        byte_stride: None,
        extensions: Default::default(),
        extras: Default::default(),
        name: None,
        target: Some(Valid(json::buffer::Target::ElementArrayBuffer)),
    });

    let positions = root.push(json::Accessor {
        buffer_view: Some(vertex_bv),
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

    let indices_acc = root.push(json::Accessor {
        buffer_view: Some(index_bv),
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
            map.insert(Valid(json::mesh::Semantic::Positions), positions);
            map
        },
        extensions: Default::default(),
        extras: Default::default(),
        indices: Some(indices_acc),
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

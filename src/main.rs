use clap::Parser;
use qem_simplifier::{
    qem_context_create, qem_context_destroy, qem_simplify, register_log_callback, QemMeshView,
    QemSimplifyOptions, QemSimplifyResult, QEM_STATUS_SUCCESS,
};
use std::ffi::CStr;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::os::raw::c_char;
use std::path::Path;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Input model file (OBJ or GLB)
    #[arg(short, long)]
    input: String,

    /// Output model file
    #[arg(short, long)]
    output: String,

    /// Reduction ratio (0.0 to 1.0). Used if target-tris is not specified.
    #[arg(short, long, default_value_t = 0.5)]
    ratio: f32,

    /// Target number of triangles.
    #[arg(long)]
    target_triangles: Option<u32>,

    /// Target number of vertices.
    #[arg(long)]
    target_vertices: Option<u32>,

    /// Target error.
    #[arg(long, default_value_t = 0.0)]
    target_error: f32,

    /// Minimum number of vertices.
    #[arg(long, default_value_t = 0)]
    min_vertices: u32,

    /// Minimum number of triangles.
    #[arg(long, default_value_t = 0)]
    min_triangles: u32,

    /// Limit error.
    #[arg(long, default_value_t = 1e10)]
    limit_error: f32,

    /// Edge weight.
    #[arg(long, default_value_t = 8.0)]
    edge_weight: f32,

    /// Max edge length factor (0.0 to disable).
    #[arg(long, default_value_t = 0.0)]
    max_edge_length_factor: f32,

    /// Preserve surface area.
    #[arg(short, long, default_value_t = true)]
    preserve: bool,
}

impl Args {
    fn to_simplify_options(&self, original_tris: u32) -> QemSimplifyOptions {
        let target_triangles = self
            .target_triangles
            .unwrap_or(((original_tris as f32) * self.ratio) as u32);
        QemSimplifyOptions {
            target_vertices: self.target_vertices.unwrap_or(0),
            target_triangles,
            target_error: self.target_error,
            min_vertices: self.min_vertices,
            min_triangles: self.min_triangles,
            limit_error: self.limit_error,
            edge_weight: self.edge_weight,
            max_edge_length_factor: self.max_edge_length_factor,
            preserve_surface_area: if self.preserve { 1 } else { 0 },
        }
    }
}

fn simplify_with_v2(
    vertices: &mut Vec<f32>,
    indices: &mut Vec<u32>,
    material_ids: &mut Vec<i32>,
    settings: QemSimplifyOptions,
) {
    let context = qem_context_create();
    if context.is_null() {
        panic!("Failed to create qem context");
    }

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

    unsafe { qem_context_destroy(context) };

    if status != QEM_STATUS_SUCCESS || result.status != QEM_STATUS_SUCCESS {
        panic!(
            "qem_simplify failed. status={}, result_status={}",
            status, result.status
        );
    }

    vertices.truncate(result.num_vertices as usize * 3);
    indices.truncate(result.num_indices as usize);
    material_ids.truncate(result.num_triangles as usize);
}

unsafe extern "C" fn cli_log_callback(msg: *const c_char) {
    if let Ok(c_str) = unsafe { CStr::from_ptr(msg) }.to_str() {
        println!("[LIB]: {}", c_str);
    }
}

fn load_obj(path: &Path) -> (Vec<f32>, Vec<u32>, Vec<i32>) {
    let (models, _) = tobj::load_obj(path, &tobj::GPU_LOAD_OPTIONS).expect("Failed to load OBJ");
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
    (all_vertices, all_indices, all_material_ids)
}

fn save_obj(path: &Path, vertices: &[f32], indices: &[u32]) {
    let file = File::create(path).expect("Failed to create output file");
    let mut writer = BufWriter::new(file);

    for v in vertices.chunks(3) {
        writeln!(writer, "v {} {} {}", v[0], v[1], v[2]).unwrap();
    }
    for i in indices.chunks(3) {
        writeln!(writer, "f {} {} {}", i[0] + 1, i[1] + 1, i[2] + 1).unwrap();
    }
}

fn handle_glb(args: &Args) {
    let (document, buffers, _) = gltf::import(&args.input).expect("Failed to load GLB");

    for mesh in document.meshes() {
        for primitive in mesh.primitives() {
            let reader = primitive.reader(|buffer| Some(&buffers[buffer.index()]));

            let mut vertices: Vec<f32> = match reader.read_positions() {
                Some(iter) => iter.flatten().collect(),
                None => continue,
            };
            let mut indices: Vec<u32> = match reader.read_indices() {
                Some(read) => read.into_u32().collect(),
                None => continue,
            };
            let mut material_ids = vec![0i32; indices.len() / 3];

            let settings = args.to_simplify_options(indices.len() as u32 / 3);

            println!(
                "Simplifying GLB Primitive: {} -> {} triangles",
                indices.len() / 3,
                settings.target_triangles
            );

            simplify_with_v2(&mut vertices, &mut indices, &mut material_ids, settings);

            let out_path = Path::new(&args.output);
            save_obj(out_path, &vertices, &indices);
            println!("Saved simplified mesh to {} (OBJ format)", args.output);
            return;
        }
    }
}

fn main() {
    let args = Args::parse();

    // Register log callback
    unsafe {
        register_log_callback(cli_log_callback);
    }

    let input_path = Path::new(&args.input);
    let ext = input_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    if ext.to_lowercase() == "obj" {
        let (mut vertices, mut indices, mut material_ids) = load_obj(input_path);
        let settings = args.to_simplify_options(indices.len() as u32 / 3);

        println!(
            "Simplifying OBJ: {} -> {} triangles",
            indices.len() / 3,
            settings.target_triangles
        );

        simplify_with_v2(&mut vertices, &mut indices, &mut material_ids, settings);

        save_obj(Path::new(&args.output), &vertices, &indices);
        println!("Saved simplified OBJ to {}", args.output);
    } else if ext.to_lowercase() == "glb" || ext.to_lowercase() == "gltf" {
        handle_glb(&args);
    } else {
        println!("Unsupported format: {}", ext);
    }
}

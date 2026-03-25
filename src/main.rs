use clap::Parser;
use qem_simplifier::{simplify_mesh, SimplifySettings, register_log_callback};
use std::path::Path;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::ffi::CStr;
use std::os::raw::c_char;

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
    target_tris: Option<u32>,

    /// Target number of vertices.
    #[arg(long)]
    target_verts: Option<u32>,

    /// Target error.
    #[arg(long, default_value_t = 0.0)]
    target_error: f32,

    /// Limit number of vertices.
    #[arg(long, default_value_t = 0)]
    limit_verts: u32,

    /// Limit number of triangles.
    #[arg(long, default_value_t = 0)]
    limit_tris: u32,

    /// Limit error.
    #[arg(long, default_value_t = 1e10)]
    limit_error: f32,

    /// Edge weight (UE5 default 8.0).
    #[arg(long, default_value_t = 8.0)]
    edge_weight: f32,

    /// Max edge length factor (0.0 to disable).
    #[arg(long, default_value_t = 0.0)]
    max_edge_length_factor: f32,

    /// Use UE5 surface area preservation.
    #[arg(short, long, default_value_t = true)]
    preserve: bool,
}

impl Args {
    fn to_simplify_settings(&self, original_tris: u32) -> SimplifySettings {
        let target_tris = self.target_tris.unwrap_or(((original_tris as f32) * self.ratio) as u32);
        SimplifySettings {
            target_num_verts: self.target_verts.unwrap_or(0),
            target_num_tris: target_tris,
            target_error: self.target_error,
            limit_num_verts: self.limit_verts,
            limit_num_tris: self.limit_tris,
            limit_error: self.limit_error,
            edge_weight: self.edge_weight,
            max_edge_length_factor: self.max_edge_length_factor,
            preserve_surface_area: self.preserve,
        }
    }
}

unsafe extern "C" fn cli_log_callback(msg: *const c_char) {
    if let Ok(c_str) = unsafe { CStr::from_ptr(msg) }.to_str() {
        println!("[LIB]: {}", c_str);
    }
}

fn load_obj(path: &Path) -> (Vec<f32>, Vec<u32>, Vec<i32>) {
    let (models, _) = tobj::load_obj(path, &tobj::GPU_LOAD_OPTIONS).expect("Failed to load OBJ");
    let mut all_verts = Vec::new();
    let mut all_indexes = Vec::new();
    let mut all_materials = Vec::new();

    for model in models {
        let mesh = model.mesh;
        let offset = (all_verts.len() / 3) as u32;
        all_verts.extend(mesh.positions);
        
        let num_tris = mesh.indices.len() / 3;
        for idx in mesh.indices {
            all_indexes.push(idx + offset);
        }
        let material_id = mesh.material_id.unwrap_or(0) as i32;
        for _ in 0..num_tris {
            all_materials.push(material_id);
        }
    }
    (all_verts, all_indexes, all_materials)
}

fn save_obj(path: &Path, verts: &[f32], indexes: &[u32]) {
    let file = File::create(path).expect("Failed to create output file");
    let mut writer = BufWriter::new(file);

    for v in verts.chunks(3) {
        writeln!(writer, "v {} {} {}", v[0], v[1], v[2]).unwrap();
    }
    for i in indexes.chunks(3) {
        writeln!(writer, "f {} {} {}", i[0] + 1, i[1] + 1, i[2] + 1).unwrap();
    }
}

fn handle_glb(args: &Args) {
    let (document, buffers, _) = gltf::import(&args.input).expect("Failed to load GLB");
    
    for mesh in document.meshes() {
        for primitive in mesh.primitives() {
            let reader = primitive.reader(|buffer| Some(&buffers[buffer.index()]));
            
            let mut verts: Vec<f32> = match reader.read_positions() {
                Some(iter) => iter.flatten().collect(),
                None => continue,
            };
            let mut indexes: Vec<u32> = match reader.read_indices() {
                Some(read) => read.into_u32().collect(),
                None => continue,
            };
            let mut materials = vec![0i32; indexes.len() / 3];

            let settings = args.to_simplify_settings(indexes.len() as u32 / 3);

            println!("Simplifying GLB Primitive: {} -> {} triangles", indexes.len() / 3, settings.target_num_tris);

            let attribute_weights = vec![];
            unsafe {
                simplify_mesh(
                    verts.as_mut_ptr(),
                    (verts.len() / 3) as u32,
                    indexes.as_mut_ptr(),
                    indexes.len() as u32,
                    materials.as_mut_ptr(),
                    0,
                    attribute_weights.as_ptr(),
                    settings,
                );
            }

            let out_path = Path::new(&args.output);
            save_obj(out_path, &verts, &indexes);
            println!("Saved simplified mesh to {} (OBJ format)", args.output);
            return;
        }
    }
}

fn main() {
    let args = Args::parse();
    
    // Register log callback
    unsafe { register_log_callback(cli_log_callback); }

    let input_path = Path::new(&args.input);
    let ext = input_path.extension().and_then(|s| s.to_str()).unwrap_or("");

    if ext.to_lowercase() == "obj" {
        let (mut verts, mut indexes, mut materials) = load_obj(input_path);
        let settings = args.to_simplify_settings(indexes.len() as u32 / 3);

        println!("Simplifying OBJ: {} -> {} triangles", indexes.len() / 3, settings.target_num_tris);

        let attribute_weights = vec![];
        unsafe {
            simplify_mesh(
                verts.as_mut_ptr(),
                (verts.len() / 3) as u32,
                indexes.as_mut_ptr(),
                indexes.len() as u32,
                materials.as_mut_ptr(),
                0,
                attribute_weights.as_ptr(),
                settings,
            );
        }
        
        save_obj(Path::new(&args.output), &verts, &indexes);
        println!("Saved simplified OBJ to {}", args.output);
    } else if ext.to_lowercase() == "glb" || ext.to_lowercase() == "gltf" {
        handle_glb(&args);
    } else {
        println!("Unsupported format: {}", ext);
    }
}

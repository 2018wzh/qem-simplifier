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

    /// Reduction ratio (0.0 to 1.0)
    #[arg(short, long, default_value_t = 0.5)]
    ratio: f32,

    /// Use UE5 surface area preservation
    #[arg(short, long, default_value_t = true)]
    preserve: bool,
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

fn handle_glb(input: &str, output: &str, ratio: f32, preserve: bool) {
    let (document, buffers, _) = gltf::import(input).expect("Failed to load GLB");
    
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

            let target_tris = ((indexes.len() / 3) as f32 * ratio) as u32;
            let settings = SimplifySettings {
                target_num_tris: target_tris,
                preserve_surface_area: preserve,
                ..SimplifySettings::default()
            };

            println!("Simplifying GLB Primitive: {} -> {} triangles", indexes.len() / 3, target_tris);

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

            let out_path = Path::new(output);
            save_obj(out_path, &verts, &indexes);
            println!("Saved simplified mesh to {} (OBJ format)", output);
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
        let target_tris = ((indexes.len() / 3) as f32 * args.ratio) as u32;
        
        let settings = SimplifySettings {
            target_num_tris: target_tris,
            preserve_surface_area: args.preserve,
            ..SimplifySettings::default()
        };

        println!("Simplifying OBJ: {} -> {} triangles", indexes.len() / 3, target_tris);

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
        handle_glb(&args.input, &args.output, args.ratio, args.preserve);
    } else {
        println!("Unsupported format: {}", ext);
    }
}

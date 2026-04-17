use crate::QemSimplifyOptions;
use clap::{Parser, Subcommand};

pub mod model;
pub mod progress;
pub mod scene;

#[derive(Parser, Debug)]
#[command(author, version, about = "QEM Mesh Simplifier CLI", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Simplify a single mesh model (OBJ, GLB)
    Model(ModelArgs),
    /// Simplify a scene with multiple meshes and instances (FBX)
    Scene(SceneArgs),
}

#[derive(Parser, Debug)]
pub struct ModelArgs {
    /// Input model file (OBJ or GLB)
    #[arg(short, long)]
    pub input: String,

    /// Output model file (OBJ or GLB)
    #[arg(short, long)]
    pub output: String,

    /// Reduction ratio (0.0 to 1.0). Used if target-triangles is not specified.
    #[arg(short, long, default_value_t = 0.5)]
    pub ratio: f32,

    /// Target number of triangles.
    #[arg(long)]
    pub target_triangles: Option<u32>,

    /// Target number of vertices.
    #[arg(long)]
    pub target_vertices: Option<u32>,

    /// Target error.
    #[arg(long, default_value_t = 0.0)]
    pub target_error: f32,

    /// Minimum number of vertices.
    #[arg(long, default_value_t = 0)]
    pub min_vertices: u32,

    /// Minimum number of triangles.
    #[arg(long, default_value_t = 0)]
    pub min_triangles: u32,

    /// Limit error.
    #[arg(long, default_value_t = 1e10)]
    pub limit_error: f32,

    /// Edge weight.
    #[arg(long, default_value_t = 8.0)]
    pub edge_weight: f32,

    /// Max edge length factor (0.0 to disable).
    #[arg(long, default_value_t = 0.0)]
    pub max_edge_length_factor: f32,

    /// Preserve surface area.
    #[arg(short, long, default_value_t = true)]
    pub preserve: bool,
}

impl ModelArgs {
    pub fn to_simplify_options(&self, original_tris: u32) -> QemSimplifyOptions {
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

#[derive(Parser, Debug)]
pub struct SceneArgs {
    /// Input scene file (FBX or GLB)
    #[arg(short, long)]
    pub input: String,

    /// Output scene file (GLB)
    #[arg(short, long)]
    pub output: String,

    /// Target triangle ratio for the whole scene (0.0 to 1.0)
    #[arg(short, long, default_value_t = 0.5)]
    pub ratio: f32,

    /// Minimum ratio for any single mesh (0.0 to 1.0)
    #[arg(long, default_value_t = 0.05)]
    pub min_mesh_ratio: f32,

    /// Maximum ratio for any single mesh (0.0 to 1.0)
    #[arg(long, default_value_t = 1.0)]
    pub max_mesh_ratio: f32,

    /// Importance weight mode (0: Uniform, 1: Volume, 2: Volume * Instances)
    #[arg(long, default_value_t = 2)]
    pub weight_mode: u32,

    /// Use world scale for importance weighting.
    #[arg(long, default_value_t = true)]
    pub use_world_scale: bool,
}

pub fn run() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Model(args) => {
            if let Err(e) = model::handle_model(&args) {
                eprintln!("Error simplifying model: {}", e);
            }
        }
        Commands::Scene(args) => {
            if let Err(e) = scene::handle_scene(&args) {
                eprintln!("Error simplifying scene: {}", e);
            }
        }
    }
}

pub mod math_util;
pub mod quadric;
pub mod hash;
pub mod binary_heap;
pub mod disjoint_set;
pub mod simplifier;

use simplifier::FMeshSimplifier;
use std::slice;
use std::ffi::CString;
use std::os::raw::c_char;
use std::sync::Mutex;

pub type LogCallback = unsafe extern "C" fn(*const c_char);

static LOG_CALLBACK: Mutex<Option<LogCallback>> = Mutex::new(None);

#[no_mangle]
pub unsafe extern "C" fn register_log_callback(callback: LogCallback) {
    let mut lock = LOG_CALLBACK.lock().unwrap();
    *lock = Some(callback);
}

pub fn log_internal(msg: &str) {
    if let Ok(lock) = LOG_CALLBACK.lock() {
        if let Some(callback) = *lock {
            if let Ok(c_str) = CString::new(msg) {
                unsafe { callback(c_str.as_ptr()); }
            }
        }
    }
}

#[repr(C)]
pub struct SimplifySettings {
    pub target_num_verts: u32,
    pub target_num_tris: u32,
    pub target_error: f32,
    pub limit_num_verts: u32,
    pub limit_num_tris: u32,
    pub limit_error: f32,
    pub edge_weight: f32,
    pub max_edge_length_factor: f32,
    pub preserve_surface_area: bool,
}

impl Default for SimplifySettings {
    fn default() -> Self {
        Self {
            target_num_verts: 0,
            target_num_tris: 0,
            target_error: 0.0,
            limit_num_verts: 0,
            limit_num_tris: 0,
            limit_error: 1e10,
            edge_weight: 8.0,
            max_edge_length_factor: 0.0,
            preserve_surface_area: true,
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn simplify_mesh(
    verts: *mut f32,
    num_verts: u32,
    indexes: *mut u32,
    num_indexes: u32,
    material_indexes: *mut i32,
    num_attributes: u32,
    attribute_weights: *const f32,
    settings: SimplifySettings,
) -> f32 {
    let verts_slice = unsafe { slice::from_raw_parts_mut(verts, (num_verts * (3 + num_attributes)) as usize) };
    let indexes_slice = unsafe { slice::from_raw_parts_mut(indexes, num_indexes as usize) };
    let material_indexes_slice = unsafe { slice::from_raw_parts_mut(material_indexes, (num_indexes / 3) as usize) };
    let attribute_weights_slice = unsafe { slice::from_raw_parts(attribute_weights, num_attributes as usize) };

    log_internal(&format!("Starting mesh simplification: target_tris={}", settings.target_num_tris));

    let mut simplifier = FMeshSimplifier::new(
        verts_slice,
        num_verts,
        indexes_slice,
        num_indexes,
        material_indexes_slice,
        num_attributes,
        attribute_weights_slice,
    );

    simplifier.edge_weight = settings.edge_weight;
    simplifier.max_edge_length_factor = settings.max_edge_length_factor;

    let error = simplifier.simplify(
        settings.target_num_verts,
        settings.target_num_tris,
        settings.target_error,
        settings.limit_num_verts,
        settings.limit_num_tris,
        settings.limit_error,
    );

    if settings.preserve_surface_area {
        log_internal("Preserving surface area...");
        simplifier.preserve_surface_area();
    }

    log_internal("Compacting mesh...");
    simplifier.compact();
    log_internal(&format!("Simplification complete. Max error: {}", error));

    error
}

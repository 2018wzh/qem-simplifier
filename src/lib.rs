pub mod binary_heap;
pub mod disjoint_set;
pub mod hash;
pub mod math_util;
pub mod quadric;
pub mod simplifier;

use simplifier::FMeshSimplifier;
use std::ffi::c_void;
use std::ffi::CString;
use std::os::raw::c_char;
use std::slice;
use std::sync::Mutex;

pub type LogCallback = unsafe extern "C" fn(*const c_char);

static LOG_CALLBACK: Mutex<Option<LogCallback>> = Mutex::new(None);

pub const QEM_STATUS_SUCCESS: i32 = 0;
pub const QEM_STATUS_INVALID_ARGUMENT: i32 = -1;
pub const QEM_STATUS_PANIC: i32 = -2;

#[derive(Clone, Copy, Debug, Default)]
struct SimplifyResultInfo {
    num_verts: u32,
    num_indexes: u32,
    num_tris: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct QemMeshView {
    pub verts: *mut f32,
    pub num_verts: u32,
    pub indexes: *mut u32,
    pub num_indexes: u32,
    pub material_indexes: *mut i32,
    pub num_attributes: u32,
    pub attribute_weights: *const f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct QemSimplifyOptions {
    pub target_num_verts: u32,
    pub target_num_tris: u32,
    pub target_error: f32,
    pub limit_num_verts: u32,
    pub limit_num_tris: u32,
    pub limit_error: f32,
    pub edge_weight: f32,
    pub max_edge_length_factor: f32,
    pub preserve_surface_area: u8,
}

impl Default for QemSimplifyOptions {
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
            preserve_surface_area: 1,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct QemSimplifyResult {
    pub status: i32,
    pub max_error: f32,
    pub num_verts: u32,
    pub num_indexes: u32,
    pub num_tris: u32,
}

#[derive(Debug, Default)]
struct QemContextState {
    last_result: QemSimplifyResult,
}

static LAST_SIMPLIFY_RESULT: Mutex<SimplifyResultInfo> = Mutex::new(SimplifyResultInfo {
    num_verts: 0,
    num_indexes: 0,
    num_tris: 0,
});

impl From<QemSimplifyOptions> for SimplifySettings {
    fn from(options: QemSimplifyOptions) -> Self {
        Self {
            target_num_verts: options.target_num_verts,
            target_num_tris: options.target_num_tris,
            target_error: options.target_error,
            limit_num_verts: options.limit_num_verts,
            limit_num_tris: options.limit_num_tris,
            limit_error: options.limit_error,
            edge_weight: options.edge_weight,
            max_edge_length_factor: options.max_edge_length_factor,
            preserve_surface_area: options.preserve_surface_area != 0,
        }
    }
}

fn run_simplify_internal(
    verts: *mut f32,
    num_verts: u32,
    indexes: *mut u32,
    num_indexes: u32,
    material_indexes: *mut i32,
    num_attributes: u32,
    attribute_weights: *const f32,
    settings: SimplifySettings,
) -> Result<(f32, SimplifyResultInfo), i32> {
    if verts.is_null()
        || indexes.is_null()
        || material_indexes.is_null()
        || num_verts == 0
        || num_indexes == 0
        || num_indexes % 3 != 0
        || (num_attributes > 0 && attribute_weights.is_null())
    {
        return Err(QEM_STATUS_INVALID_ARGUMENT);
    }

    let run_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        let verts_slice =
            slice::from_raw_parts_mut(verts, (num_verts * (3 + num_attributes)) as usize);
        let indexes_slice = slice::from_raw_parts_mut(indexes, num_indexes as usize);
        let material_indexes_slice =
            slice::from_raw_parts_mut(material_indexes, (num_indexes / 3) as usize);
        let attribute_weights_slice =
            slice::from_raw_parts(attribute_weights, num_attributes as usize);

        log_internal(&format!(
            "Starting mesh simplification: target_tris={}",
            settings.target_num_tris
        ));

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

        let final_num_verts = simplifier.remaining_num_verts;
        let final_num_tris = simplifier.remaining_num_tris;

        log_internal("Compacting mesh...");
        simplifier.compact();

        log_internal(&format!("Simplification complete. Max error: {}", error));

        (
            error,
            SimplifyResultInfo {
                num_verts: final_num_verts,
                num_tris: final_num_tris,
                num_indexes: final_num_tris * 3,
            },
        )
    }));

    match run_result {
        Ok(result) => Ok(result),
        Err(_) => Err(QEM_STATUS_PANIC),
    }
}

#[no_mangle]
pub unsafe extern "C" fn register_log_callback(callback: LogCallback) {
    let mut lock = LOG_CALLBACK.lock().unwrap();
    *lock = Some(callback);
}

#[no_mangle]
pub extern "C" fn qem_get_abi_version() -> u32 {
    2
}

#[no_mangle]
pub extern "C" fn qem_context_create() -> *mut c_void {
    Box::into_raw(Box::new(QemContextState::default())) as *mut c_void
}

#[no_mangle]
pub unsafe extern "C" fn qem_context_destroy(context: *mut c_void) {
    if !context.is_null() {
        unsafe {
            drop(Box::from_raw(context as *mut QemContextState));
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn qem_get_last_result(
    context: *const c_void,
    out_result: *mut QemSimplifyResult,
) -> i32 {
    if context.is_null() || out_result.is_null() {
        return QEM_STATUS_INVALID_ARGUMENT;
    }

    unsafe {
        *out_result = (*(context as *const QemContextState)).last_result;
    }
    QEM_STATUS_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn qem_simplify(
    context: *mut c_void,
    mesh: *mut QemMeshView,
    options: *const QemSimplifyOptions,
    out_result: *mut QemSimplifyResult,
) -> i32 {
    if context.is_null() || mesh.is_null() || options.is_null() || out_result.is_null() {
        return QEM_STATUS_INVALID_ARGUMENT;
    }

    let mesh_view = unsafe { *mesh };
    let settings: SimplifySettings = unsafe { *options }.into();

    let (status, result) = match run_simplify_internal(
        mesh_view.verts,
        mesh_view.num_verts,
        mesh_view.indexes,
        mesh_view.num_indexes,
        mesh_view.material_indexes,
        mesh_view.num_attributes,
        mesh_view.attribute_weights,
        settings,
    ) {
        Ok((max_error, info)) => (
            QEM_STATUS_SUCCESS,
            QemSimplifyResult {
                status: QEM_STATUS_SUCCESS,
                max_error,
                num_verts: info.num_verts,
                num_indexes: info.num_indexes,
                num_tris: info.num_tris,
            },
        ),
        Err(code) => (
            code,
            QemSimplifyResult {
                status: code,
                max_error: 0.0,
                num_verts: 0,
                num_indexes: 0,
                num_tris: 0,
            },
        ),
    };

    unsafe {
        (*(context as *mut QemContextState)).last_result = result;
        *out_result = result;
    }

    status
}

pub fn log_internal(msg: &str) {
    if let Ok(lock) = LOG_CALLBACK.lock() {
        if let Some(callback) = *lock {
            if let Ok(c_str) = CString::new(msg) {
                unsafe {
                    callback(c_str.as_ptr());
                }
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
#[deprecated(
    since = "0.2.0",
    note = "Use qem_simplify (ABI v2). This legacy ABI is retained only for internal correctness testing."
)]
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
    match run_simplify_internal(
        verts,
        num_verts,
        indexes,
        num_indexes,
        material_indexes,
        num_attributes,
        attribute_weights,
        settings,
    ) {
        Ok((error, info)) => {
            if let Ok(mut lock) = LAST_SIMPLIFY_RESULT.lock() {
                *lock = info;
            }
            error
        }
        Err(code) => {
            log_internal(&format!("Legacy simplify_mesh failed. status={}", code));
            0.0
        }
    }
}

#[no_mangle]
#[deprecated(
    since = "0.2.0",
    note = "Use qem_get_last_result (ABI v2). This legacy API is retained only for internal correctness testing."
)]
pub unsafe extern "C" fn get_last_simplify_result(
    out_num_verts: *mut u32,
    out_num_indexes: *mut u32,
    out_num_tris: *mut u32,
) {
    if let Ok(lock) = LAST_SIMPLIFY_RESULT.lock() {
        if !out_num_verts.is_null() {
            unsafe {
                *out_num_verts = lock.num_verts;
            }
        }
        if !out_num_indexes.is_null() {
            unsafe {
                *out_num_indexes = lock.num_indexes;
            }
        }
        if !out_num_tris.is_null() {
            unsafe {
                *out_num_tris = lock.num_tris;
            }
        }
    }
}

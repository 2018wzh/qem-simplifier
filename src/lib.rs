pub mod cli;
pub mod math;
pub mod quadric;
pub mod scene;
pub mod simplifier;
pub mod util;

use simplifier::MeshSimplifier;
use std::ffi::c_void;
use std::ffi::CString;
use std::os::raw::c_char;
use std::slice;
use std::sync::Mutex;

pub type LogCallback = unsafe extern "C" fn(*const c_char);
pub type ProgressCallback = unsafe extern "C" fn(*const QemProgressEvent, *mut c_void);

static LOG_CALLBACK: Mutex<Option<LogCallback>> = Mutex::new(None);

pub const QEM_STATUS_SUCCESS: i32 = 0;
pub const QEM_STATUS_INVALID_ARGUMENT: i32 = -1;
pub const QEM_STATUS_PANIC: i32 = -2;
pub const QEM_STATUS_INSUFFICIENT_BUFFER: i32 = -3;

pub const QEM_PROGRESS_SCOPE_MESH: u32 = 0;
pub const QEM_PROGRESS_SCOPE_SCENE: u32 = 1;

pub const QEM_PROGRESS_STAGE_BEGIN: u32 = 0;
pub const QEM_PROGRESS_STAGE_UPDATE: u32 = 1;
pub const QEM_PROGRESS_STAGE_END: u32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct QemProgressEvent {
    pub scope: u32,
    pub stage: u32,
    pub percent: f32,
    pub mesh_index: u32,
    pub mesh_count: u32,
    pub source_triangles: u32,
    pub target_triangles: u32,
    pub output_triangles: u32,
    pub status: i32,
}

#[derive(Clone, Copy, Debug, Default)]
struct SimplifyResultInfo {
    num_vertices: u32,
    num_indices: u32,
    num_triangles: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct QemMeshView {
    pub vertices: *mut f32,
    pub num_vertices: u32,
    pub indices: *mut u32,
    pub num_indices: u32,
    pub material_ids: *mut i32,
    pub num_attributes: u32,
    pub attribute_weights: *const f32,
}

unsafe impl Send for QemMeshView {}
unsafe impl Sync for QemMeshView {}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct QemSimplifyOptions {
    pub target_vertices: u32,
    pub target_triangles: u32,
    pub target_error: f32,
    pub min_vertices: u32,
    pub min_triangles: u32,
    pub limit_error: f32,
    pub edge_weight: f32,
    pub max_edge_length_factor: f32,
    pub preserve_surface_area: u8,
}

impl Default for QemSimplifyOptions {
    fn default() -> Self {
        Self {
            target_vertices: 0,
            target_triangles: 0,
            target_error: 0.0,
            min_vertices: 0,
            min_triangles: 0,
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
    pub num_vertices: u32,
    pub num_indices: u32,
    pub num_triangles: u32,
}

#[derive(Debug, Default)]
struct QemContextState {
    last_result: QemSimplifyResult,
    progress_callback: Option<ProgressCallback>,
    progress_user_data: *mut c_void,
}

#[derive(Clone, Copy, Debug)]
struct CoreSimplifySettings {
    target_vertices: u32,
    target_triangles: u32,
    target_error: f32,
    min_vertices: u32,
    min_triangles: u32,
    limit_error: f32,
    edge_weight: f32,
    max_edge_length_factor: f32,
    preserve_surface_area: bool,
}

impl From<QemSimplifyOptions> for CoreSimplifySettings {
    fn from(options: QemSimplifyOptions) -> Self {
        Self {
            target_vertices: options.target_vertices,
            target_triangles: options.target_triangles,
            target_error: options.target_error,
            min_vertices: options.min_vertices,
            min_triangles: options.min_triangles,
            limit_error: options.limit_error,
            edge_weight: options.edge_weight,
            max_edge_length_factor: options.max_edge_length_factor,
            preserve_surface_area: options.preserve_surface_area != 0,
        }
    }
}

fn run_simplify_internal(
    context: *mut c_void,
    vertices: *mut f32,
    num_vertices: u32,
    indices: *mut u32,
    num_indices: u32,
    material_ids: *mut i32,
    num_attributes: u32,
    attribute_weights: *const f32,
    settings: CoreSimplifySettings,
) -> Result<(f32, SimplifyResultInfo), i32> {
    if vertices.is_null()
        || indices.is_null()
        || material_ids.is_null()
        || num_vertices == 0
        || num_indices == 0
        || num_indices % 3 != 0
        || (num_attributes > 0 && attribute_weights.is_null())
    {
        return Err(QEM_STATUS_INVALID_ARGUMENT);
    }

    let source_triangles = num_indices / 3;
    report_progress_event(
        context,
        QemProgressEvent {
            scope: QEM_PROGRESS_SCOPE_MESH,
            stage: QEM_PROGRESS_STAGE_BEGIN,
            percent: 0.0,
            mesh_index: 0,
            mesh_count: 1,
            source_triangles,
            target_triangles: settings.target_triangles,
            output_triangles: source_triangles,
            status: QEM_STATUS_SUCCESS,
        },
    );

    let run_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        let vertices_slice =
            slice::from_raw_parts_mut(vertices, (num_vertices * (3 + num_attributes)) as usize);
        let indices_slice = slice::from_raw_parts_mut(indices, num_indices as usize);
        let material_ids_slice =
            slice::from_raw_parts_mut(material_ids, (num_indices / 3) as usize);
        let attribute_weights_slice = if num_attributes == 0 {
            &[]
        } else {
            slice::from_raw_parts(attribute_weights, num_attributes as usize)
        };

        log_internal(&format!(
            "Starting mesh simplification: target_triangles={}",
            settings.target_triangles
        ));

        let mut simplifier = MeshSimplifier::new(
            vertices_slice,
            num_vertices,
            indices_slice,
            num_indices,
            material_ids_slice,
            num_attributes,
            attribute_weights_slice,
        );

        simplifier.edge_weight = settings.edge_weight;
        simplifier.max_edge_length_factor = settings.max_edge_length_factor;

        let error = simplifier.simplify(
            settings.target_vertices,
            settings.target_triangles,
            settings.target_error,
            settings.min_vertices,
            settings.min_triangles,
            settings.limit_error,
        );

        if settings.preserve_surface_area {
            log_internal("Preserving surface area...");
            simplifier.preserve_surface_area();
        }

        let final_vertex_count = simplifier.remaining_vertices;
        let final_triangle_count = simplifier.remaining_triangles;

        log_internal("Compacting mesh...");
        simplifier.compact();

        log_internal(&format!("Simplification complete. Max error: {}", error));

        (
            error,
            SimplifyResultInfo {
                num_vertices: final_vertex_count,
                num_triangles: final_triangle_count,
                num_indices: final_triangle_count * 3,
            },
        )
    }));

    match run_result {
        Ok((error, info)) => {
            report_progress_event(
                context,
                QemProgressEvent {
                    scope: QEM_PROGRESS_SCOPE_MESH,
                    stage: QEM_PROGRESS_STAGE_END,
                    percent: 1.0,
                    mesh_index: 0,
                    mesh_count: 1,
                    source_triangles,
                    target_triangles: settings.target_triangles,
                    output_triangles: info.num_triangles,
                    status: QEM_STATUS_SUCCESS,
                },
            );
            Ok((error, info))
        }
        Err(_) => {
            report_progress_event(
                context,
                QemProgressEvent {
                    scope: QEM_PROGRESS_SCOPE_MESH,
                    stage: QEM_PROGRESS_STAGE_END,
                    percent: 1.0,
                    mesh_index: 0,
                    mesh_count: 1,
                    source_triangles,
                    target_triangles: settings.target_triangles,
                    output_triangles: source_triangles,
                    status: QEM_STATUS_PANIC,
                },
            );
            Err(QEM_STATUS_PANIC)
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn register_log_callback(callback: LogCallback) {
    let mut lock = LOG_CALLBACK.lock().unwrap();
    *lock = Some(callback);
}

#[no_mangle]
pub unsafe extern "C" fn qem_context_set_progress_callback(
    context: *mut c_void,
    callback: ProgressCallback,
    user_data: *mut c_void,
) -> i32 {
    if context.is_null() {
        return QEM_STATUS_INVALID_ARGUMENT;
    }

    let context_state = unsafe { &mut *(context as *mut QemContextState) };
    context_state.progress_callback = Some(callback);
    context_state.progress_user_data = user_data;
    QEM_STATUS_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn qem_context_clear_progress_callback(context: *mut c_void) -> i32 {
    if context.is_null() {
        return QEM_STATUS_INVALID_ARGUMENT;
    }

    let context_state = unsafe { &mut *(context as *mut QemContextState) };
    context_state.progress_callback = None;
    context_state.progress_user_data = std::ptr::null_mut();
    QEM_STATUS_SUCCESS
}

#[no_mangle]
pub extern "C" fn qem_get_abi_version() -> u32 {
    7
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

    let mesh_view = unsafe { &mut *mesh };
    let settings: CoreSimplifySettings = unsafe { *options }.into();

    let (status, result) = match run_simplify_internal(
        context,
        mesh_view.vertices,
        mesh_view.num_vertices,
        mesh_view.indices,
        mesh_view.num_indices,
        mesh_view.material_ids,
        mesh_view.num_attributes,
        mesh_view.attribute_weights,
        settings,
    ) {
        Ok((max_error, info)) => (
            QEM_STATUS_SUCCESS,
            QemSimplifyResult {
                status: QEM_STATUS_SUCCESS,
                max_error,
                num_vertices: info.num_vertices,
                num_indices: info.num_indices,
                num_triangles: info.num_triangles,
            },
        ),
        Err(code) => (
            code,
            QemSimplifyResult {
                status: code,
                max_error: 0.0,
                num_vertices: 0,
                num_indices: 0,
                num_triangles: 0,
            },
        ),
    };

    if status == QEM_STATUS_SUCCESS && result.status == QEM_STATUS_SUCCESS {
        mesh_view.num_vertices = result.num_vertices;
        mesh_view.num_indices = result.num_indices;
    }

    unsafe {
        (*(context as *mut QemContextState)).last_result = result;
        *out_result = result;
    }

    status
}

pub(crate) fn report_progress_event(context: *mut c_void, mut event: QemProgressEvent) {
    if context.is_null() {
        return;
    }

    event.percent = event.percent.clamp(0.0, 1.0);

    let context_state = unsafe { &*(context as *const QemContextState) };
    if let Some(callback) = context_state.progress_callback {
        unsafe {
            callback(
                &event as *const QemProgressEvent,
                context_state.progress_user_data,
            );
        }
    }
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

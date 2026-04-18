use crate::{
    log_internal, qem_context_create, qem_context_destroy, qem_simplify, report_progress_event,
    QemMeshView, QemProgressEvent, QemSimplifyOptions, QemSimplifyResult, QEM_PROGRESS_SCOPE_SCENE,
    QEM_PROGRESS_STAGE_BEGIN, QEM_PROGRESS_STAGE_END, QEM_PROGRESS_STAGE_UPDATE,
    QEM_STATUS_INSUFFICIENT_BUFFER, QEM_STATUS_INVALID_ARGUMENT, QEM_STATUS_PANIC,
    QEM_STATUS_SUCCESS,
};
use rayon::prelude::*;
use rayon::ThreadPoolBuilder;
use std::ffi::{c_char, c_void};
use std::fmt::Write as _;
use std::ptr;
use std::slice;
use std::sync::atomic::{AtomicU32, Ordering};

pub const QEM_SCENE_WEIGHT_UNIFORM: u32 = 0;
pub const QEM_SCENE_WEIGHT_MESH_VOLUME: u32 = 1;
pub const QEM_SCENE_WEIGHT_MESH_VOLUME_X_INSTANCES: u32 = 2;
pub const QEM_SCENE_WEIGHT_EXTERNAL: u32 = 3;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct QemSceneMeshView {
    pub mesh_id: u32,
    pub mesh: QemMeshView,
}

unsafe impl Send for QemSceneMeshView {}
unsafe impl Sync for QemSceneMeshView {}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct QemSceneGraphNodeView {
    pub parent_index: i32,
    pub local_matrix: [f32; 16],
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct QemSceneGraphMeshBindingView {
    pub node_index: u32,
    pub mesh_index: u32,
    pub mesh_to_node_matrix: [f32; 16],
    pub use_mesh_to_node_matrix: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct QemSceneGraphView {
    pub meshes: *mut QemSceneMeshView,
    pub num_meshes: u32,
    pub nodes: *const QemSceneGraphNodeView,
    pub num_nodes: u32,
    pub mesh_bindings: *const QemSceneGraphMeshBindingView,
    pub num_mesh_bindings: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct QemScenePolicy {
    pub target_triangle_ratio: f32,
    pub min_mesh_ratio: f32,
    pub max_mesh_ratio: f32,
    pub weight_mode: u32,
    pub use_world_scale: u8,
    pub target_total_triangles: u64,
    pub min_triangles_per_mesh: u32,
    pub weight_exponent: f32,
    pub enable_parallel: u8,
    pub max_parallel_tasks: u32,
    pub external_importance_weights: *const f32,
    pub external_importance_count: u32,
}

impl Default for QemScenePolicy {
    fn default() -> Self {
        Self {
            target_triangle_ratio: 0.5,
            min_mesh_ratio: 0.05,
            max_mesh_ratio: 1.0,
            weight_mode: QEM_SCENE_WEIGHT_MESH_VOLUME_X_INSTANCES,
            use_world_scale: 1,
            target_total_triangles: 0,
            min_triangles_per_mesh: 64,
            weight_exponent: 1.15,
            enable_parallel: 1,
            max_parallel_tasks: 0,
            external_importance_weights: std::ptr::null(),
            external_importance_count: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct QemSceneExecutionOptions {
    pub enable_parallel: u8,
    pub max_parallel_tasks: u32,
    pub retry_count: u32,
    pub fallback_relaxation_step: f32,
}

impl Default for QemSceneExecutionOptions {
    fn default() -> Self {
        Self {
            enable_parallel: 1,
            max_parallel_tasks: 0,
            retry_count: 1,
            fallback_relaxation_step: 0.15,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct QemSceneMeshStatistics {
    pub mesh_index: u32,
    pub mesh_id: u32,
    pub status: i32,
    pub source_triangles: u32,
    pub target_triangles: u32,
    pub output_triangles: u32,
    pub source_effective_triangles: f64,
    pub target_effective_triangles: f64,
    pub output_effective_triangles: f64,
    pub target_ratio: f32,
    pub achieved_ratio: f32,
    pub budget_deviation: f32,
    pub max_error: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct QemSceneStatisticsSummary {
    pub status: i32,
    pub num_meshes: u32,
    pub num_failed_meshes: u32,
    pub num_simplified_meshes: u32,
    pub source_triangles: u64,
    pub target_triangles: u64,
    pub output_triangles: u64,
    pub target_scene_ratio: f32,
    pub achieved_scene_ratio: f32,
    pub target_hit_ratio: f32,
    pub mean_abs_budget_deviation: f32,
    pub max_abs_budget_deviation: f32,
    pub mean_max_error: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct QemSceneMeshFeature {
    pub mesh_index: u32,
    pub mesh_id: u32,
    pub source_triangles: u32,
    pub instance_count: u32,
    pub world_scale_sum: f64,
    pub bbox_volume: f64,
    pub importance_weight: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct QemSceneMeshDecision {
    pub mesh_index: u32,
    pub mesh_id: u32,
    pub source_triangles: u32,
    pub source_effective_triangles: f64,
    pub importance_weight: f64,
    pub keep_ratio: f32,
    pub target_triangles: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct QemSceneMeshResult {
    pub mesh_index: u32,
    pub mesh_id: u32,
    pub status: i32,
    pub source_triangles: u32,
    pub requested_triangles: u32,
    pub output_triangles: u32,
    pub max_error: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct QemSceneSimplifyResult {
    pub status: i32,
    pub num_meshes: u32,
    pub num_decisions: u32,
    pub num_simplified_meshes: u32,
    pub source_triangles: u64,
    pub target_triangles: u64,
    pub output_triangles: u64,
    pub source_effective_triangles: f64,
    pub target_effective_triangles: f64,
}

#[derive(Clone, Copy, Debug, Default)]
struct MeshSceneMetrics {
    mesh_index: u32,
    mesh_id: u32,
    source_triangles: u32,
    source_effective_triangles: f64,
    instance_count: u32,
    world_scale_sum: f64,
    bbox_volume: f64,
    importance_weight: f64,
}

fn clamp01(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn matrix_abs_det3x3(m: &[f32; 16]) -> f64 {
    let m00 = m[0] as f64;
    let m01 = m[1] as f64;
    let m02 = m[2] as f64;

    let m10 = m[4] as f64;
    let m11 = m[5] as f64;
    let m12 = m[6] as f64;

    let m20 = m[8] as f64;
    let m21 = m[9] as f64;
    let m22 = m[10] as f64;

    let det = m00 * (m11 * m22 - m12 * m21) - m01 * (m10 * m22 - m12 * m20)
        + m02 * (m10 * m21 - m11 * m20);

    if det.is_finite() {
        det.abs().max(1.0e-6)
    } else {
        1.0
    }
}

fn mesh_volume(mesh: &QemMeshView) -> f64 {
    if mesh.vertices.is_null() || mesh.num_vertices == 0 {
        return 1.0;
    }

    let stride = 3usize.saturating_add(mesh.num_attributes as usize);
    if stride < 3 {
        return 1.0;
    }

    let total_len = (mesh.num_vertices as usize).saturating_mul(stride);
    if total_len < 3 {
        return 1.0;
    }

    let vertices = unsafe { slice::from_raw_parts(mesh.vertices as *const f32, total_len) };

    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut min_z = f32::INFINITY;

    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    let mut max_z = f32::NEG_INFINITY;

    for i in 0..mesh.num_vertices as usize {
        let base = i * stride;
        let x = vertices[base];
        let y = vertices[base + 1];
        let z = vertices[base + 2];

        min_x = min_x.min(x);
        min_y = min_y.min(y);
        min_z = min_z.min(z);

        max_x = max_x.max(x);
        max_y = max_y.max(y);
        max_z = max_z.max(z);
    }

    let dx = (max_x - min_x).max(1.0e-6) as f64;
    let dy = (max_y - min_y).max(1.0e-6) as f64;
    let dz = (max_z - min_z).max(1.0e-6) as f64;

    let volume = dx * dy * dz;
    if volume.is_finite() {
        volume.max(1.0e-6)
    } else {
        1.0
    }
}

fn allocate_with_bounds(
    importance: &[f64],
    min_values: &[f64],
    max_values: &[f64],
    target_total: f64,
) -> Vec<f64> {
    let count = importance.len();
    let mut values = vec![0.0; count];
    let mut active = vec![true; count];

    let mut remaining = target_total;
    for i in 0..count {
        if min_values[i] >= max_values[i] {
            values[i] = max_values[i];
            active[i] = false;
            remaining -= values[i];
        }
    }

    let mut guard = 0usize;
    while guard < count.saturating_mul(2).saturating_add(4) {
        guard += 1;

        let mut active_indices = Vec::new();
        let mut positive_weight_sum = 0.0;
        for i in 0..count {
            if active[i] {
                active_indices.push(i);
                if importance[i] > 0.0 {
                    positive_weight_sum += importance[i];
                }
            }
        }

        if active_indices.is_empty() {
            break;
        }

        let mut effective_weights = Vec::with_capacity(active_indices.len());
        let mut effective_weight_sum = 0.0;

        if positive_weight_sum <= 0.0 {
            for _ in &active_indices {
                effective_weights.push(1.0);
                effective_weight_sum += 1.0;
            }
        } else {
            let fallback_weight = positive_weight_sum / active_indices.len() as f64;
            for &i in &active_indices {
                let weight = if importance[i] > 0.0 {
                    importance[i]
                } else {
                    fallback_weight
                };
                effective_weights.push(weight);
                effective_weight_sum += weight;
            }
        }

        if effective_weight_sum <= 0.0 {
            effective_weight_sum = active_indices.len() as f64;
        }

        let mut hit_bound = false;
        let free_min_sum = active_indices.iter().map(|&i| min_values[i]).sum::<f64>();
        let free_max_sum = active_indices.iter().map(|&i| max_values[i]).sum::<f64>();

        let free_target = remaining.clamp(free_min_sum, free_max_sum);

        for (active_pos, &i) in active_indices.iter().enumerate() {
            let weight = effective_weights[active_pos];
            let mut proposed = free_target * (weight / effective_weight_sum);

            if proposed < min_values[i] {
                proposed = min_values[i];
                active[i] = false;
                hit_bound = true;
            } else if proposed > max_values[i] {
                proposed = max_values[i];
                active[i] = false;
                hit_bound = true;
            }

            values[i] = proposed;
        }

        if !hit_bound {
            break;
        }

        remaining = target_total;
        for v in &values {
            remaining -= *v;
        }
    }

    for i in 0..count {
        values[i] = values[i].clamp(min_values[i], max_values[i]);
    }

    values
}

fn solve_integer_targets(
    keep_effective: &[f64],
    min_targets: &[u32],
    max_targets: &[u32],
    desired_total: u64,
) -> Vec<u32> {
    let sum_min: u64 = min_targets.iter().map(|&v| v as u64).sum();
    let sum_max: u64 = max_targets.iter().map(|&v| v as u64).sum();
    let desired = desired_total.clamp(sum_min, sum_max);

    let mut targets: Vec<u32> = keep_effective
        .iter()
        .enumerate()
        .map(|(i, &value)| {
            let floor_value = if value.is_finite() && value > 0.0 {
                value.floor() as u64
            } else {
                0
            };
            floor_value.clamp(min_targets[i] as u64, max_targets[i] as u64) as u32
        })
        .collect();

    let current: u64 = targets.iter().map(|&v| v as u64).sum();

    if current < desired {
        let mut diff = desired - current;
        let mut order: Vec<usize> = (0..targets.len()).collect();
        order.sort_by(|&a, &b| {
            let ra = if keep_effective[a].is_finite() {
                keep_effective[a] - keep_effective[a].floor()
            } else {
                0.0
            };
            let rb = if keep_effective[b].is_finite() {
                keep_effective[b] - keep_effective[b].floor()
            } else {
                0.0
            };
            rb.partial_cmp(&ra).unwrap_or(std::cmp::Ordering::Equal)
        });

        for &i in &order {
            if diff == 0 {
                break;
            }
            if targets[i] < max_targets[i] {
                targets[i] += 1;
                diff -= 1;
            }
        }

        if diff > 0 {
            for &i in &order {
                if diff == 0 {
                    break;
                }
                let room = (max_targets[i] - targets[i]) as u64;
                let add = room.min(diff);
                targets[i] += add as u32;
                diff -= add;
            }
        }
    } else if current > desired {
        let mut diff = current - desired;
        let mut order: Vec<usize> = (0..targets.len()).collect();
        order.sort_by(|&a, &b| {
            let ra = if keep_effective[a].is_finite() {
                keep_effective[a] - keep_effective[a].floor()
            } else {
                0.0
            };
            let rb = if keep_effective[b].is_finite() {
                keep_effective[b] - keep_effective[b].floor()
            } else {
                0.0
            };
            ra.partial_cmp(&rb).unwrap_or(std::cmp::Ordering::Equal)
        });

        for &i in &order {
            if diff == 0 {
                break;
            }
            if targets[i] > min_targets[i] {
                targets[i] -= 1;
                diff -= 1;
            }
        }

        if diff > 0 {
            for &i in &order {
                if diff == 0 {
                    break;
                }
                let removable = (targets[i] - min_targets[i]) as u64;
                let remove = removable.min(diff);
                targets[i] -= remove as u32;
                diff -= remove;
            }
        }
    }

    targets
}

fn gather_scene_graph_slices<'a>(
    graph: *const QemSceneGraphView,
) -> Result<
    (
        &'a [QemSceneMeshView],
        &'a [QemSceneGraphNodeView],
        &'a [QemSceneGraphMeshBindingView],
    ),
    i32,
> {
    if graph.is_null() {
        return Err(QEM_STATUS_INVALID_ARGUMENT);
    }

    let graph_ref = unsafe { &*graph };

    if graph_ref.num_meshes == 0 || graph_ref.meshes.is_null() {
        return Err(QEM_STATUS_INVALID_ARGUMENT);
    }

    if graph_ref.num_nodes > 0 && graph_ref.nodes.is_null() {
        return Err(QEM_STATUS_INVALID_ARGUMENT);
    }

    if graph_ref.num_mesh_bindings > 0 && graph_ref.mesh_bindings.is_null() {
        return Err(QEM_STATUS_INVALID_ARGUMENT);
    }

    let meshes = unsafe {
        slice::from_raw_parts(
            graph_ref.meshes as *const QemSceneMeshView,
            graph_ref.num_meshes as usize,
        )
    };
    let nodes = if graph_ref.num_nodes == 0 {
        &[][..]
    } else {
        unsafe { slice::from_raw_parts(graph_ref.nodes, graph_ref.num_nodes as usize) }
    };
    let bindings = if graph_ref.num_mesh_bindings == 0 {
        &[][..]
    } else {
        unsafe {
            slice::from_raw_parts(
                graph_ref.mesh_bindings,
                graph_ref.num_mesh_bindings as usize,
            )
        }
    };

    Ok((meshes, nodes, bindings))
}

fn resolve_node_world_scale(
    node_index: usize,
    nodes: &[QemSceneGraphNodeView],
    cache: &mut [f64],
    state: &mut [u8],
) -> Result<f64, i32> {
    match state[node_index] {
        2 => return Ok(cache[node_index]),
        1 => return Err(QEM_STATUS_INVALID_ARGUMENT),
        _ => {}
    }

    state[node_index] = 1;
    let node = &nodes[node_index];
    let local_scale = matrix_abs_det3x3(&node.local_matrix);

    let parent_scale = if node.parent_index < 0 {
        1.0
    } else {
        let parent_index = node.parent_index as usize;
        if parent_index >= nodes.len() {
            return Err(QEM_STATUS_INVALID_ARGUMENT);
        }
        resolve_node_world_scale(parent_index, nodes, cache, state)?
    };

    let world_scale = parent_scale * local_scale;
    cache[node_index] = if world_scale.is_finite() {
        world_scale.max(1.0e-6)
    } else {
        1.0
    };
    state[node_index] = 2;

    Ok(cache[node_index])
}

fn compute_scene_graph_world_scales(nodes: &[QemSceneGraphNodeView]) -> Result<Vec<f64>, i32> {
    let mut cache = vec![1.0; nodes.len()];
    let mut state = vec![0u8; nodes.len()];

    for node_index in 0..nodes.len() {
        resolve_node_world_scale(node_index, nodes, &mut cache, &mut state)?;
    }

    Ok(cache)
}

fn compute_scene_graph_metrics(
    graph: *const QemSceneGraphView,
    policy: QemScenePolicy,
) -> Result<Vec<MeshSceneMetrics>, i32> {
    let (meshes, nodes, bindings) = gather_scene_graph_slices(graph)?;

    let mut instance_count = vec![0u32; meshes.len()];
    let mut instance_scale_sum = vec![0.0f64; meshes.len()];

    let node_world_scales = if policy.use_world_scale != 0 && !nodes.is_empty() {
        Some(compute_scene_graph_world_scales(nodes)?)
    } else {
        None
    };

    for binding in bindings {
        let node_index = binding.node_index as usize;
        let mesh_index = binding.mesh_index as usize;

        if mesh_index >= meshes.len() {
            log_internal(&format!(
                "qem_scene_graph_compute_decisions: invalid binding mesh_index {} (mesh_count={})",
                binding.mesh_index,
                meshes.len()
            ));
            return Err(QEM_STATUS_INVALID_ARGUMENT);
        }
        if node_index >= nodes.len() {
            log_internal(&format!(
                "qem_scene_graph_compute_decisions: invalid binding node_index {} (node_count={})",
                binding.node_index,
                nodes.len()
            ));
            return Err(QEM_STATUS_INVALID_ARGUMENT);
        }

        instance_count[mesh_index] += 1;

        let mut scale = if let Some(world_scales) = &node_world_scales {
            world_scales[node_index]
        } else {
            1.0
        };

        if policy.use_world_scale != 0 && binding.use_mesh_to_node_matrix != 0 {
            scale *= matrix_abs_det3x3(&binding.mesh_to_node_matrix);
        }

        instance_scale_sum[mesh_index] += scale;
    }

    let external_weights: Option<&[f32]> = if policy.weight_mode == QEM_SCENE_WEIGHT_EXTERNAL {
        if policy.external_importance_weights.is_null()
            || policy.external_importance_count < meshes.len() as u32
        {
            log_internal(&format!(
                "qem_scene_graph_compute_decisions: invalid external weights. ptr={:p}, count={}, mesh_count={}",
                policy.external_importance_weights,
                policy.external_importance_count,
                meshes.len()
            ));
            return Err(QEM_STATUS_INVALID_ARGUMENT);
        }
        Some(unsafe {
            slice::from_raw_parts(
                policy.external_importance_weights,
                policy.external_importance_count as usize,
            )
        })
    } else {
        None
    };

    let mut metrics = Vec::with_capacity(meshes.len());
    for (mesh_index, scene_mesh) in meshes.iter().enumerate() {
        let mesh = &scene_mesh.mesh;
        if mesh.indices.is_null() || mesh.vertices.is_null() || mesh.material_ids.is_null() {
            log_internal(&format!(
                "qem_scene_graph_compute_decisions: mesh[{}] invalid pointers. vertices={:p}, indices={:p}, material_ids={:p}",
                mesh_index,
                mesh.vertices,
                mesh.indices,
                mesh.material_ids
            ));
            return Err(QEM_STATUS_INVALID_ARGUMENT);
        }
        if mesh.num_vertices == 0 || mesh.num_indices == 0 || mesh.num_indices % 3 != 0 {
            log_internal(&format!(
                "qem_scene_graph_compute_decisions: mesh[{}] invalid geometry. num_vertices={}, num_indices={}",
                mesh_index, mesh.num_vertices, mesh.num_indices
            ));
            return Err(QEM_STATUS_INVALID_ARGUMENT);
        }

        let source_triangles = mesh.num_indices / 3;
        let instances = if instance_count[mesh_index] == 0 {
            1
        } else {
            instance_count[mesh_index]
        };
        let instance_scale = if instance_scale_sum[mesh_index] <= 0.0 {
            instances as f64
        } else {
            instance_scale_sum[mesh_index]
        };

        let source_effective_triangles = source_triangles as f64 * instance_scale.max(1.0e-6);
        let volume = mesh_volume(mesh);

        let weight_exponent = if policy.weight_exponent.is_finite() {
            policy.weight_exponent.max(0.01) as f64
        } else {
            1.0
        };

        let base_weight = match policy.weight_mode {
            QEM_SCENE_WEIGHT_UNIFORM => 1.0,
            QEM_SCENE_WEIGHT_MESH_VOLUME => volume,
            QEM_SCENE_WEIGHT_MESH_VOLUME_X_INSTANCES => volume * instance_scale.max(1.0),
            QEM_SCENE_WEIGHT_EXTERNAL => external_weights
                .and_then(|w| w.get(mesh_index).copied())
                .unwrap_or(0.0) as f64,
            _ => {
                log_internal(&format!(
                    "qem_scene_graph_compute_decisions: invalid weight_mode {}",
                    policy.weight_mode
                ));
                return Err(QEM_STATUS_INVALID_ARGUMENT);
            }
        };
        let importance_weight = base_weight.max(1.0e-6).powf(weight_exponent);

        metrics.push(MeshSceneMetrics {
            mesh_index: mesh_index as u32,
            mesh_id: scene_mesh.mesh_id,
            source_triangles,
            source_effective_triangles,
            instance_count: instances,
            world_scale_sum: instance_scale,
            bbox_volume: volume,
            importance_weight,
        });
    }

    Ok(metrics)
}

fn compute_decisions_from_metrics(
    metrics: &[MeshSceneMetrics],
    policy: QemScenePolicy,
) -> Result<(Vec<QemSceneMeshDecision>, QemSceneSimplifyResult), i32> {
    if metrics.is_empty() {
        log_internal("qem_scene_graph_compute_decisions: metrics are empty");
        return Err(QEM_STATUS_INVALID_ARGUMENT);
    }

    let mut source_triangles: u64 = 0;
    let mut source_effective_triangles_sum = 0.0;

    let min_ratio = clamp01(policy.min_mesh_ratio);
    let max_ratio = clamp01(policy.max_mesh_ratio).max(min_ratio);
    let target_ratio = clamp01(policy.target_triangle_ratio);

    let mut min_values = Vec::with_capacity(metrics.len());
    let mut max_values = Vec::with_capacity(metrics.len());
    let mut importance = Vec::with_capacity(metrics.len());

    let mut min_targets = Vec::with_capacity(metrics.len());
    let mut max_targets = Vec::with_capacity(metrics.len());

    for metric in metrics {
        source_triangles += metric.source_triangles as u64;
        source_effective_triangles_sum += metric.source_effective_triangles;
        let min_ratio_tri = ((metric.source_triangles as f64) * min_ratio as f64).ceil() as u32;
        let max_ratio_tri = ((metric.source_triangles as f64) * max_ratio as f64).ceil() as u32;
        let min_floor_tri = policy.min_triangles_per_mesh.min(metric.source_triangles);

        let min_tri = min_ratio_tri
            .max(min_floor_tri)
            .min(metric.source_triangles);
        let max_tri = max_ratio_tri.max(min_tri).min(metric.source_triangles);

        min_targets.push(min_tri);
        max_targets.push(max_tri);

        min_values.push(min_tri as f64);
        max_values.push(max_tri as f64);
        importance.push(metric.importance_weight);
    }

    let min_total = min_values.iter().sum::<f64>();
    let max_total = max_values.iter().sum::<f64>();
    let unclamped_target_total = if policy.target_total_triangles > 0 {
        policy.target_total_triangles as f64
    } else {
        source_triangles as f64 * target_ratio as f64
    };
    let target_total = unclamped_target_total.clamp(min_total, max_total);

    let keep_effective = allocate_with_bounds(&importance, &min_values, &max_values, target_total);
    let integer_targets = solve_integer_targets(
        &keep_effective,
        &min_targets,
        &max_targets,
        target_total.round() as u64,
    );

    let mut decisions = Vec::with_capacity(metrics.len());
    let mut target_triangles: u64 = 0;
    let mut target_effective_triangles = 0.0;

    for (i, metric) in metrics.iter().enumerate() {
        let source_tri = metric.source_triangles.max(1);
        let target_tri = integer_targets[i].clamp(min_targets[i], max_targets[i]);

        let keep_ratio = (target_tri as f32 / source_tri as f32).clamp(min_ratio, max_ratio);

        target_triangles += target_tri as u64;
        target_effective_triangles += target_tri as f64;

        decisions.push(QemSceneMeshDecision {
            mesh_index: metric.mesh_index,
            mesh_id: metric.mesh_id,
            source_triangles: source_tri,
            source_effective_triangles: metric.source_effective_triangles,
            importance_weight: metric.importance_weight,
            keep_ratio,
            target_triangles: target_tri,
        });
    }

    let summary = QemSceneSimplifyResult {
        status: QEM_STATUS_SUCCESS,
        num_meshes: metrics.len() as u32,
        num_decisions: decisions.len() as u32,
        num_simplified_meshes: 0,
        source_triangles,
        target_triangles,
        output_triangles: source_triangles,
        source_effective_triangles: source_effective_triangles_sum,
        target_effective_triangles: target_effective_triangles,
    };

    Ok((decisions, summary))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocate_with_bounds_preserves_total_with_mixed_zero_weights() {
        let importance = [10.0, 0.0];
        let min_values = [0.0, 0.0];
        let max_values = [100.0, 100.0];
        let target_total = 60.0;

        let values = allocate_with_bounds(&importance, &min_values, &max_values, target_total);
        let sum: f64 = values.iter().sum();

        assert!((sum - target_total).abs() < 1.0e-6);
        assert!(values[0] > values[1]);
    }

    #[test]
    fn compute_decisions_respects_global_budget_after_integer_rounding() {
        let metrics = vec![
            MeshSceneMetrics {
                mesh_index: 0,
                mesh_id: 100,
                source_triangles: 1,
                source_effective_triangles: 1.0,
                instance_count: 1,
                world_scale_sum: 1.0,
                bbox_volume: 1.0,
                importance_weight: 1.0,
            },
            MeshSceneMetrics {
                mesh_index: 1,
                mesh_id: 101,
                source_triangles: 1,
                source_effective_triangles: 1.0,
                instance_count: 1,
                world_scale_sum: 1.0,
                bbox_volume: 1.0,
                importance_weight: 1.0,
            },
        ];

        let policy = QemScenePolicy {
            target_triangle_ratio: 0.5,
            min_mesh_ratio: 0.0,
            max_mesh_ratio: 1.0,
            min_triangles_per_mesh: 0,
            target_total_triangles: 0,
            ..QemScenePolicy::default()
        };

        let (decisions, summary) = compute_decisions_from_metrics(&metrics, policy)
            .expect("decision compute should succeed");

        let total: u64 = decisions.iter().map(|d| d.target_triangles as u64).sum();
        assert_eq!(total, 1);
        assert_eq!(summary.target_triangles, 1);
        assert!(decisions.iter().all(|d| d.target_triangles <= 1));
    }
}

fn compute_decisions_graph_internal(
    graph: *const QemSceneGraphView,
    policy: QemScenePolicy,
) -> Result<(Vec<QemSceneMeshDecision>, QemSceneSimplifyResult), i32> {
    let metrics = compute_scene_graph_metrics(graph, policy)?;
    compute_decisions_from_metrics(&metrics, policy)
}

fn resolve_execution_options(
    policy: QemScenePolicy,
    execution_options: *const QemSceneExecutionOptions,
) -> QemSceneExecutionOptions {
    let mut resolved = if execution_options.is_null() {
        QemSceneExecutionOptions {
            enable_parallel: policy.enable_parallel,
            max_parallel_tasks: policy.max_parallel_tasks,
            retry_count: 1,
            fallback_relaxation_step: 0.15,
        }
    } else {
        unsafe { *execution_options }
    };

    resolved.retry_count = resolved.retry_count.max(1);
    if !resolved.fallback_relaxation_step.is_finite() || resolved.fallback_relaxation_step < 0.0 {
        resolved.fallback_relaxation_step = 0.15;
    }
    resolved
}

fn run_mesh_with_retry(
    context: *mut c_void,
    use_local_context: bool,
    mesh: &mut QemMeshView,
    requested_triangles: u32,
    base_options: QemSimplifyOptions,
    execution: QemSceneExecutionOptions,
) -> (i32, QemSimplifyResult, u32) {
    let source_triangles = mesh.num_indices / 3;
    let mut last_status = QEM_STATUS_PANIC;
    let mut last_result = QemSimplifyResult::default();
    let mut last_requested = requested_triangles.min(source_triangles);

    for attempt in 0..execution.retry_count {
        let relax = 1.0 + execution.fallback_relaxation_step * attempt as f32;
        let attempt_target = ((requested_triangles as f32) * relax).ceil() as u32;
        let target_triangles = attempt_target.min(source_triangles).max(1);
        last_requested = target_triangles;

        let mut options = base_options;
        options.target_triangles = target_triangles;

        let mut result = QemSimplifyResult::default();
        let status = if use_local_context {
            let local_context = qem_context_create();
            if local_context.is_null() {
                QEM_STATUS_PANIC
            } else {
                let status = unsafe { qem_simplify(local_context, mesh, &options, &mut result) };
                unsafe {
                    qem_context_destroy(local_context);
                }
                status
            }
        } else {
            unsafe { qem_simplify(context, mesh, &options, &mut result) }
        };

        last_status = status;
        last_result = result;

        if status == QEM_STATUS_SUCCESS
            && result.status == QEM_STATUS_SUCCESS
            && result.num_triangles > 0
            && result.num_indices == result.num_triangles.saturating_mul(3)
        {
            return (status, result, target_triangles);
        }
    }

    (
        last_status,
        QemSimplifyResult {
            status: last_status,
            max_error: last_result.max_error,
            num_vertices: mesh.num_vertices,
            num_indices: mesh.num_indices,
            num_triangles: source_triangles,
        },
        last_requested,
    )
}

#[no_mangle]
pub unsafe extern "C" fn qem_scene_compute_statistics(
    decisions: *const QemSceneMeshDecision,
    num_decisions: u32,
    mesh_results: *const QemSceneMeshResult,
    num_mesh_results: u32,
    out_statistics: *mut QemSceneMeshStatistics,
    statistics_capacity: u32,
    out_statistics_count: *mut u32,
    out_summary: *mut QemSceneStatisticsSummary,
) -> i32 {
    if decisions.is_null()
        || mesh_results.is_null()
        || out_statistics_count.is_null()
        || out_summary.is_null()
        || num_decisions == 0
        || num_mesh_results == 0
    {
        return QEM_STATUS_INVALID_ARGUMENT;
    }

    if out_statistics.is_null() && statistics_capacity != 0 {
        return QEM_STATUS_INVALID_ARGUMENT;
    }

    let decisions_slice = unsafe { slice::from_raw_parts(decisions, num_decisions as usize) };
    let results_slice = unsafe { slice::from_raw_parts(mesh_results, num_mesh_results as usize) };

    let mut decision_by_mesh = std::collections::BTreeMap::new();
    for decision in decisions_slice {
        if decision_by_mesh
            .insert(decision.mesh_index, *decision)
            .is_some()
        {
            return QEM_STATUS_INVALID_ARGUMENT;
        }
    }

    let mut result_by_mesh = std::collections::BTreeMap::new();
    for result in results_slice {
        if !decision_by_mesh.contains_key(&result.mesh_index) {
            return QEM_STATUS_INVALID_ARGUMENT;
        }
        if result_by_mesh.insert(result.mesh_index, *result).is_some() {
            return QEM_STATUS_INVALID_ARGUMENT;
        }
    }

    if decision_by_mesh.len() != result_by_mesh.len() {
        return QEM_STATUS_INVALID_ARGUMENT;
    }

    let mut statistics = Vec::with_capacity(decision_by_mesh.len());

    let mut source_triangles = 0u64;
    let mut target_triangles = 0u64;
    let mut output_triangles = 0u64;
    let mut num_failed_meshes = 0u32;
    let mut num_simplified_meshes = 0u32;
    let mut sum_abs_budget_deviation = 0.0f64;
    let mut max_abs_budget_deviation = 0.0f64;
    let mut sum_max_error = 0.0f64;
    let mut counted_error_meshes = 0u32;
    let mut first_error = QEM_STATUS_SUCCESS;

    for (mesh_index, decision) in &decision_by_mesh {
        let result = match result_by_mesh.get(mesh_index) {
            Some(value) => value,
            None => return QEM_STATUS_INVALID_ARGUMENT,
        };

        let source_tri = decision.source_triangles.max(1);
        let requested_tri = result
            .requested_triangles
            .max(1)
            .min(result.source_triangles.max(1));
        let output_tri = result.output_triangles.min(result.source_triangles.max(1));

        let source_effective = decision.source_effective_triangles.max(1.0e-6);
        let effective_per_triangle = source_effective / source_tri as f64;
        let target_effective = effective_per_triangle * requested_tri as f64;
        let output_effective = effective_per_triangle * output_tri as f64;

        let target_ratio = clamp01(requested_tri as f32 / source_tri as f32);
        let achieved_ratio = clamp01(output_tri as f32 / source_tri as f32);
        let budget_deviation = (output_tri as f64 - requested_tri as f64) / requested_tri as f64;

        source_triangles += source_tri as u64;
        target_triangles += requested_tri as u64;
        output_triangles += output_tri as u64;

        if result.status != QEM_STATUS_SUCCESS {
            num_failed_meshes += 1;
            if first_error == QEM_STATUS_SUCCESS {
                first_error = result.status;
            }
        } else if output_tri < source_tri {
            num_simplified_meshes += 1;
        }

        let abs_deviation = budget_deviation.abs();
        sum_abs_budget_deviation += abs_deviation;
        max_abs_budget_deviation = max_abs_budget_deviation.max(abs_deviation);

        if result.status == QEM_STATUS_SUCCESS {
            sum_max_error += result.max_error as f64;
            counted_error_meshes += 1;
        }

        statistics.push(QemSceneMeshStatistics {
            mesh_index: *mesh_index,
            mesh_id: decision.mesh_id,
            status: result.status,
            source_triangles: source_tri,
            target_triangles: requested_tri,
            output_triangles: output_tri,
            source_effective_triangles: source_effective,
            target_effective_triangles: target_effective,
            output_effective_triangles: output_effective,
            target_ratio,
            achieved_ratio,
            budget_deviation: budget_deviation as f32,
            max_error: result.max_error,
        });
    }

    unsafe {
        *out_statistics_count = statistics.len() as u32;
    }

    let num_meshes = statistics.len() as u32;
    let target_scene_ratio = if source_triangles > 0 {
        target_triangles as f32 / source_triangles as f32
    } else {
        0.0
    };
    let achieved_scene_ratio = if source_triangles > 0 {
        output_triangles as f32 / source_triangles as f32
    } else {
        0.0
    };
    let target_hit_ratio = if target_triangles > 0 {
        output_triangles as f32 / target_triangles as f32
    } else {
        0.0
    };
    let mean_abs_budget_deviation = if num_meshes > 0 {
        (sum_abs_budget_deviation / num_meshes as f64) as f32
    } else {
        0.0
    };
    let mean_max_error = if counted_error_meshes > 0 {
        (sum_max_error / counted_error_meshes as f64) as f32
    } else {
        0.0
    };

    let mut summary = QemSceneStatisticsSummary {
        status: if first_error == QEM_STATUS_SUCCESS {
            QEM_STATUS_SUCCESS
        } else {
            first_error
        },
        num_meshes,
        num_failed_meshes,
        num_simplified_meshes,
        source_triangles,
        target_triangles,
        output_triangles,
        target_scene_ratio,
        achieved_scene_ratio,
        target_hit_ratio,
        mean_abs_budget_deviation,
        max_abs_budget_deviation: max_abs_budget_deviation as f32,
        mean_max_error,
    };

    if !out_statistics.is_null() {
        if statistics_capacity < statistics.len() as u32 {
            summary.status = QEM_STATUS_INSUFFICIENT_BUFFER;
            unsafe {
                *out_summary = summary;
            }
            return QEM_STATUS_INSUFFICIENT_BUFFER;
        }

        unsafe {
            ptr::copy_nonoverlapping(statistics.as_ptr(), out_statistics, statistics.len());
        }
    }

    unsafe {
        *out_summary = summary;
    }

    QEM_STATUS_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn qem_scene_export_statistics_csv(
    mesh_statistics: *const QemSceneMeshStatistics,
    num_mesh_statistics: u32,
    summary: *const QemSceneStatisticsSummary,
    out_buffer: *mut c_char,
    buffer_capacity: u32,
    out_required_size: *mut u32,
) -> i32 {
    if summary.is_null() || out_required_size.is_null() {
        return QEM_STATUS_INVALID_ARGUMENT;
    }

    if (mesh_statistics.is_null() && num_mesh_statistics > 0)
        || (out_buffer.is_null() && buffer_capacity > 0)
    {
        return QEM_STATUS_INVALID_ARGUMENT;
    }

    let stats_slice = if num_mesh_statistics == 0 {
        &[][..]
    } else {
        unsafe { slice::from_raw_parts(mesh_statistics, num_mesh_statistics as usize) }
    };
    let summary_ref = unsafe { &*summary };

    let mut csv = String::new();
    csv.push_str("section,mesh_index,mesh_id,status,source_triangles,target_triangles,output_triangles,target_ratio,achieved_ratio,budget_deviation,max_error\\n");

    for stat in stats_slice {
        let _ = writeln!(
            csv,
            "mesh,{},{},{},{},{},{},{:.6},{:.6},{:.6},{:.6}",
            stat.mesh_index,
            stat.mesh_id,
            stat.status,
            stat.source_triangles,
            stat.target_triangles,
            stat.output_triangles,
            stat.target_ratio,
            stat.achieved_ratio,
            stat.budget_deviation,
            stat.max_error,
        );
    }

    csv.push_str("section,status,num_meshes,num_failed_meshes,num_simplified_meshes,source_triangles,target_triangles,output_triangles,target_scene_ratio,achieved_scene_ratio,target_hit_ratio,mean_abs_budget_deviation,max_abs_budget_deviation,mean_max_error\\n");
    let _ = writeln!(
        csv,
        "summary,{},{},{},{},{},{},{},{:.6},{:.6},{:.6},{:.6},{:.6},{:.6}",
        summary_ref.status,
        summary_ref.num_meshes,
        summary_ref.num_failed_meshes,
        summary_ref.num_simplified_meshes,
        summary_ref.source_triangles,
        summary_ref.target_triangles,
        summary_ref.output_triangles,
        summary_ref.target_scene_ratio,
        summary_ref.achieved_scene_ratio,
        summary_ref.target_hit_ratio,
        summary_ref.mean_abs_budget_deviation,
        summary_ref.max_abs_budget_deviation,
        summary_ref.mean_max_error,
    );

    let required_size_usize = csv.len().saturating_add(1);
    if required_size_usize > u32::MAX as usize {
        return QEM_STATUS_INVALID_ARGUMENT;
    }

    unsafe {
        *out_required_size = required_size_usize as u32;
    }

    if out_buffer.is_null() {
        return QEM_STATUS_SUCCESS;
    }

    if buffer_capacity < required_size_usize as u32 {
        return QEM_STATUS_INSUFFICIENT_BUFFER;
    }

    unsafe {
        ptr::copy_nonoverlapping(csv.as_ptr(), out_buffer as *mut u8, csv.len());
        *(out_buffer as *mut u8).add(csv.len()) = 0;
    }

    QEM_STATUS_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn qem_scene_graph_extract_features(
    graph: *const QemSceneGraphView,
    policy: *const QemScenePolicy,
    out_features: *mut QemSceneMeshFeature,
    feature_capacity: u32,
    out_feature_count: *mut u32,
    out_result: *mut QemSceneSimplifyResult,
) -> i32 {
    if graph.is_null() || policy.is_null() || out_feature_count.is_null() || out_result.is_null() {
        return QEM_STATUS_INVALID_ARGUMENT;
    }

    let policy_value = unsafe { *policy };
    let metrics = match compute_scene_graph_metrics(graph, policy_value) {
        Ok(v) => v,
        Err(code) => {
            unsafe {
                *out_feature_count = 0;
                *out_result = QemSceneSimplifyResult {
                    status: code,
                    ..QemSceneSimplifyResult::default()
                };
            }
            return code;
        }
    };

    unsafe {
        *out_feature_count = metrics.len() as u32;
    }

    if !out_features.is_null() {
        if feature_capacity < metrics.len() as u32 {
            unsafe {
                *out_result = QemSceneSimplifyResult {
                    status: QEM_STATUS_INSUFFICIENT_BUFFER,
                    num_meshes: metrics.len() as u32,
                    num_decisions: 0,
                    num_simplified_meshes: 0,
                    source_triangles: metrics.iter().map(|m| m.source_triangles as u64).sum(),
                    target_triangles: 0,
                    output_triangles: 0,
                    source_effective_triangles: 0.0,
                    target_effective_triangles: 0.0,
                };
            }
            return QEM_STATUS_INSUFFICIENT_BUFFER;
        }

        let features: Vec<QemSceneMeshFeature> = metrics
            .iter()
            .map(|m| QemSceneMeshFeature {
                mesh_index: m.mesh_index,
                mesh_id: m.mesh_id,
                source_triangles: m.source_triangles,
                instance_count: m.instance_count,
                world_scale_sum: m.world_scale_sum,
                bbox_volume: m.bbox_volume,
                importance_weight: m.importance_weight,
            })
            .collect();

        unsafe {
            ptr::copy_nonoverlapping(features.as_ptr(), out_features, features.len());
        }
    } else if feature_capacity != 0 {
        return QEM_STATUS_INVALID_ARGUMENT;
    }

    unsafe {
        *out_result = QemSceneSimplifyResult {
            status: QEM_STATUS_SUCCESS,
            num_meshes: metrics.len() as u32,
            num_decisions: 0,
            num_simplified_meshes: 0,
            source_triangles: metrics.iter().map(|m| m.source_triangles as u64).sum(),
            target_triangles: 0,
            output_triangles: 0,
            source_effective_triangles: 0.0,
            target_effective_triangles: 0.0,
        };
    }

    QEM_STATUS_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn qem_scene_graph_compute_decisions(
    graph: *const QemSceneGraphView,
    policy: *const QemScenePolicy,
    out_decisions: *mut QemSceneMeshDecision,
    decision_capacity: u32,
    out_decision_count: *mut u32,
    out_result: *mut QemSceneSimplifyResult,
) -> i32 {
    if graph.is_null() || policy.is_null() || out_decision_count.is_null() || out_result.is_null() {
        return QEM_STATUS_INVALID_ARGUMENT;
    }

    let policy_value = unsafe { *policy };

    let (decisions, mut summary) = match compute_decisions_graph_internal(graph, policy_value) {
        Ok(value) => value,
        Err(code) => {
            unsafe {
                *out_decision_count = 0;
                *out_result = QemSceneSimplifyResult {
                    status: code,
                    ..QemSceneSimplifyResult::default()
                };
            }
            return code;
        }
    };

    unsafe {
        *out_decision_count = decisions.len() as u32;
    }

    if !out_decisions.is_null() {
        if decision_capacity < decisions.len() as u32 {
            summary.status = QEM_STATUS_INSUFFICIENT_BUFFER;
            unsafe {
                *out_result = summary;
            }
            return QEM_STATUS_INSUFFICIENT_BUFFER;
        }

        unsafe {
            ptr::copy_nonoverlapping(decisions.as_ptr(), out_decisions, decisions.len());
        }
    } else if decision_capacity != 0 {
        summary.status = QEM_STATUS_INVALID_ARGUMENT;
        unsafe {
            *out_result = summary;
        }
        return QEM_STATUS_INVALID_ARGUMENT;
    }

    unsafe {
        *out_result = summary;
    }

    QEM_STATUS_SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn qem_scene_graph_apply_decisions(
    context: *mut c_void,
    scene_graph: *mut QemSceneGraphView,
    decisions: *const QemSceneMeshDecision,
    num_decisions: u32,
    base_options: *const QemSimplifyOptions,
    out_mesh_results: *mut QemSceneMeshResult,
    mesh_result_capacity: u32,
    out_result: *mut QemSceneSimplifyResult,
) -> i32 {
    unsafe {
        qem_scene_graph_apply_decisions_ex(
            context,
            scene_graph,
            std::ptr::null(),
            decisions,
            num_decisions,
            base_options,
            std::ptr::null(),
            out_mesh_results,
            mesh_result_capacity,
            out_result,
        )
    }
}

#[no_mangle]
pub unsafe extern "C" fn qem_scene_graph_apply_decisions_ex(
    context: *mut c_void,
    scene_graph: *mut QemSceneGraphView,
    policy: *const QemScenePolicy,
    decisions: *const QemSceneMeshDecision,
    num_decisions: u32,
    base_options: *const QemSimplifyOptions,
    execution_options: *const QemSceneExecutionOptions,
    out_mesh_results: *mut QemSceneMeshResult,
    mesh_result_capacity: u32,
    out_result: *mut QemSceneSimplifyResult,
) -> i32 {
    if context.is_null()
        || scene_graph.is_null()
        || decisions.is_null()
        || base_options.is_null()
        || out_result.is_null()
    {
        return QEM_STATUS_INVALID_ARGUMENT;
    }

    let policy_value = if policy.is_null() {
        QemScenePolicy::default()
    } else {
        unsafe { *policy }
    };
    let execution = resolve_execution_options(policy_value, execution_options);

    let graph_ref = unsafe { &mut *scene_graph };
    if graph_ref.num_meshes == 0 || graph_ref.meshes.is_null() {
        return QEM_STATUS_INVALID_ARGUMENT;
    }

    if !out_mesh_results.is_null() {
        if mesh_result_capacity < graph_ref.num_meshes {
            return QEM_STATUS_INSUFFICIENT_BUFFER;
        }
    } else if mesh_result_capacity != 0 {
        return QEM_STATUS_INVALID_ARGUMENT;
    }

    let meshes = unsafe {
        slice::from_raw_parts_mut(
            graph_ref.meshes as *mut QemSceneMeshView,
            graph_ref.num_meshes as usize,
        )
    };
    let decisions_slice = unsafe { slice::from_raw_parts(decisions, num_decisions as usize) };

    let mut decision_by_mesh = vec![None; meshes.len()];
    for decision in decisions_slice {
        let mesh_index = decision.mesh_index as usize;
        if mesh_index >= meshes.len() || decision_by_mesh[mesh_index].is_some() {
            return QEM_STATUS_INVALID_ARGUMENT;
        }
        decision_by_mesh[mesh_index] = Some(*decision);
    }

    if decision_by_mesh.iter().any(|v| v.is_none()) {
        return QEM_STATUS_INVALID_ARGUMENT;
    }

    let mut source_triangles = 0u64;
    let mut target_triangles = 0u64;
    let mut output_triangles = 0u64;
    let mut num_simplified_meshes = 0u32;

    let scene_source_triangles = meshes
        .iter()
        .map(|m| (m.mesh.num_indices / 3) as u64)
        .sum::<u64>();
    let scene_target_triangles = decisions_slice
        .iter()
        .map(|d| d.target_triangles as u64)
        .sum::<u64>();

    let mesh_count = meshes.len() as u32;

    report_progress_event(
        context,
        QemProgressEvent {
            scope: QEM_PROGRESS_SCOPE_SCENE,
            stage: QEM_PROGRESS_STAGE_BEGIN,
            percent: 0.0,
            mesh_index: 0,
            mesh_count,
            source_triangles: scene_source_triangles.min(u32::MAX as u64) as u32,
            target_triangles: scene_target_triangles.min(u32::MAX as u64) as u32,
            output_triangles: scene_source_triangles.min(u32::MAX as u64) as u32,
            status: QEM_STATUS_SUCCESS,
        },
    );

    let mut decisions_indexed = vec![QemSceneMeshDecision::default(); meshes.len()];
    for (idx, decision) in decision_by_mesh.into_iter().enumerate() {
        decisions_indexed[idx] = decision.expect("decision exists");
    }

    let base_options_value = unsafe { *base_options };
    let run_parallel = execution.enable_parallel != 0 && meshes.len() > 1;
    let progress_context_addr = context as usize;

    let mut mesh_results: Vec<QemSceneMeshResult> = if run_parallel {
        let completed_count = AtomicU32::new(0);

        let mut collect_parallel = || {
            meshes
                .par_iter_mut()
                .enumerate()
                .map(|(mesh_index, scene_mesh)| {
                    let mesh_decision = decisions_indexed[mesh_index];
                    let source_tri = scene_mesh.mesh.num_indices / 3;
                    let (status, simplify_result, requested_used) = run_mesh_with_retry(
                        std::ptr::null_mut(),
                        true,
                        &mut scene_mesh.mesh,
                        mesh_decision.target_triangles.min(source_tri),
                        base_options_value,
                        execution,
                    );

                    let output_tri = if status == QEM_STATUS_SUCCESS
                        && simplify_result.status == QEM_STATUS_SUCCESS
                    {
                        simplify_result.num_triangles
                    } else {
                        source_tri
                    };

                    let mesh_result = QemSceneMeshResult {
                        mesh_index: mesh_index as u32,
                        mesh_id: scene_mesh.mesh_id,
                        status,
                        source_triangles: source_tri,
                        requested_triangles: requested_used,
                        output_triangles: output_tri,
                        max_error: simplify_result.max_error,
                    };

                    let completed = completed_count.fetch_add(1, Ordering::SeqCst) + 1;
                    report_progress_event(
                        progress_context_addr as *mut c_void,
                        QemProgressEvent {
                            scope: QEM_PROGRESS_SCOPE_SCENE,
                            stage: QEM_PROGRESS_STAGE_UPDATE,
                            percent: (completed as f32) / (mesh_count as f32),
                            mesh_index: mesh_result.mesh_index,
                            mesh_count,
                            source_triangles: mesh_result.source_triangles,
                            target_triangles: mesh_result.requested_triangles,
                            output_triangles: mesh_result.output_triangles,
                            status: mesh_result.status,
                        },
                    );

                    mesh_result
                })
                .collect::<Vec<_>>()
        };

        if execution.max_parallel_tasks > 0 {
            if let Ok(pool) = ThreadPoolBuilder::new()
                .num_threads(execution.max_parallel_tasks as usize)
                .build()
            {
                pool.install(collect_parallel)
            } else {
                collect_parallel()
            }
        } else {
            collect_parallel()
        }
    } else {
        meshes
            .iter_mut()
            .enumerate()
            .map(|(mesh_index, scene_mesh)| {
                let mesh_decision = decisions_indexed[mesh_index];
                let source_tri = scene_mesh.mesh.num_indices / 3;
                let (status, simplify_result, requested_used) = run_mesh_with_retry(
                    context,
                    false,
                    &mut scene_mesh.mesh,
                    mesh_decision.target_triangles.min(source_tri),
                    base_options_value,
                    execution,
                );

                let output_tri = if status == QEM_STATUS_SUCCESS
                    && simplify_result.status == QEM_STATUS_SUCCESS
                {
                    simplify_result.num_triangles
                } else {
                    source_tri
                };

                QemSceneMeshResult {
                    mesh_index: mesh_index as u32,
                    mesh_id: scene_mesh.mesh_id,
                    status,
                    source_triangles: source_tri,
                    requested_triangles: requested_used,
                    output_triangles: output_tri,
                    max_error: simplify_result.max_error,
                }
            })
            .collect()
    };

    mesh_results.sort_by_key(|r| r.mesh_index);

    let mut first_error = QEM_STATUS_SUCCESS;
    for (mesh_index, result) in mesh_results.iter().enumerate() {
        source_triangles += result.source_triangles as u64;
        target_triangles += result.requested_triangles as u64;
        output_triangles += result.output_triangles as u64;

        if result.status == QEM_STATUS_SUCCESS && result.output_triangles < result.source_triangles
        {
            num_simplified_meshes += 1;
        } else if result.status != QEM_STATUS_SUCCESS && first_error == QEM_STATUS_SUCCESS {
            first_error = result.status;
        }

        if !run_parallel {
            report_progress_event(
                context,
                QemProgressEvent {
                    scope: QEM_PROGRESS_SCOPE_SCENE,
                    stage: QEM_PROGRESS_STAGE_UPDATE,
                    percent: (mesh_index as f32 + 1.0) / (mesh_count as f32),
                    mesh_index: result.mesh_index,
                    mesh_count,
                    source_triangles: result.source_triangles,
                    target_triangles: result.requested_triangles,
                    output_triangles: result.output_triangles,
                    status: result.status,
                },
            );
        }
    }

    if !out_mesh_results.is_null() {
        unsafe {
            ptr::copy_nonoverlapping(mesh_results.as_ptr(), out_mesh_results, mesh_results.len());
        }
    }

    let status = if first_error == QEM_STATUS_SUCCESS {
        QEM_STATUS_SUCCESS
    } else {
        first_error
    };

    unsafe {
        *out_result = QemSceneSimplifyResult {
            status,
            num_meshes: meshes.len() as u32,
            num_decisions,
            num_simplified_meshes,
            source_triangles,
            target_triangles,
            output_triangles,
            source_effective_triangles: source_triangles as f64,
            target_effective_triangles: target_triangles as f64,
        };
    }

    report_progress_event(
        context,
        QemProgressEvent {
            scope: QEM_PROGRESS_SCOPE_SCENE,
            stage: QEM_PROGRESS_STAGE_END,
            percent: 1.0,
            mesh_index: meshes.len().saturating_sub(1) as u32,
            mesh_count,
            source_triangles: source_triangles.min(u32::MAX as u64) as u32,
            target_triangles: target_triangles.min(u32::MAX as u64) as u32,
            output_triangles: output_triangles.min(u32::MAX as u64) as u32,
            status,
        },
    );

    status
}

#[no_mangle]
pub unsafe extern "C" fn qem_scene_graph_simplify(
    context: *mut c_void,
    scene_graph: *mut QemSceneGraphView,
    policy: *const QemScenePolicy,
    base_options: *const QemSimplifyOptions,
    out_decisions: *mut QemSceneMeshDecision,
    decision_capacity: u32,
    out_decision_count: *mut u32,
    out_mesh_results: *mut QemSceneMeshResult,
    mesh_result_capacity: u32,
    out_result: *mut QemSceneSimplifyResult,
) -> i32 {
    unsafe {
        qem_scene_graph_simplify_ex(
            context,
            scene_graph,
            policy,
            base_options,
            std::ptr::null(),
            out_decisions,
            decision_capacity,
            out_decision_count,
            out_mesh_results,
            mesh_result_capacity,
            out_result,
        )
    }
}

#[no_mangle]
pub unsafe extern "C" fn qem_scene_graph_simplify_ex(
    context: *mut c_void,
    scene_graph: *mut QemSceneGraphView,
    policy: *const QemScenePolicy,
    base_options: *const QemSimplifyOptions,
    execution_options: *const QemSceneExecutionOptions,
    out_decisions: *mut QemSceneMeshDecision,
    decision_capacity: u32,
    out_decision_count: *mut u32,
    out_mesh_results: *mut QemSceneMeshResult,
    mesh_result_capacity: u32,
    out_result: *mut QemSceneSimplifyResult,
) -> i32 {
    if context.is_null()
        || scene_graph.is_null()
        || policy.is_null()
        || base_options.is_null()
        || out_decision_count.is_null()
        || out_result.is_null()
    {
        return QEM_STATUS_INVALID_ARGUMENT;
    }

    let policy_value = unsafe { *policy };
    report_progress_event(
        context,
        QemProgressEvent {
            scope: QEM_PROGRESS_SCOPE_SCENE,
            stage: QEM_PROGRESS_STAGE_BEGIN,
            percent: 0.0,
            mesh_index: 0,
            mesh_count: 0,
            source_triangles: 0,
            target_triangles: 0,
            output_triangles: 0,
            status: QEM_STATUS_SUCCESS,
        },
    );

    let (decisions, mut summary) = match compute_decisions_graph_internal(scene_graph, policy_value)
    {
        Ok(v) => v,
        Err(code) => {
            unsafe {
                *out_decision_count = 0;
                *out_result = QemSceneSimplifyResult {
                    status: code,
                    ..QemSceneSimplifyResult::default()
                };
            }
            return code;
        }
    };

    report_progress_event(
        context,
        QemProgressEvent {
            scope: QEM_PROGRESS_SCOPE_SCENE,
            stage: QEM_PROGRESS_STAGE_UPDATE,
            percent: 0.1,
            mesh_index: 0,
            mesh_count: decisions.len() as u32,
            source_triangles: summary.source_triangles.min(u32::MAX as u64) as u32,
            target_triangles: summary.target_triangles.min(u32::MAX as u64) as u32,
            output_triangles: summary.source_triangles.min(u32::MAX as u64) as u32,
            status: QEM_STATUS_SUCCESS,
        },
    );

    unsafe {
        *out_decision_count = decisions.len() as u32;
    }

    if !out_decisions.is_null() {
        if decision_capacity < decisions.len() as u32 {
            summary.status = QEM_STATUS_INSUFFICIENT_BUFFER;
            unsafe {
                *out_result = summary;
            }
            return QEM_STATUS_INSUFFICIENT_BUFFER;
        }
        unsafe {
            ptr::copy_nonoverlapping(decisions.as_ptr(), out_decisions, decisions.len());
        }
    } else if decision_capacity != 0 {
        return QEM_STATUS_INVALID_ARGUMENT;
    }

    unsafe {
        qem_scene_graph_apply_decisions_ex(
            context,
            scene_graph,
            policy,
            decisions.as_ptr(),
            decisions.len() as u32,
            base_options,
            execution_options,
            out_mesh_results,
            mesh_result_capacity,
            out_result,
        )
    }
}

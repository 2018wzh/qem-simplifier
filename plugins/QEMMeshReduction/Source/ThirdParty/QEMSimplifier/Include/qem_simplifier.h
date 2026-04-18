#include <cstdarg>
#include <cstdint>
#include <cstdlib>
#include <ostream>
#include <new>

constexpr static const int32_t QEM_STATUS_SUCCESS = 0;

constexpr static const int32_t QEM_STATUS_INVALID_ARGUMENT = -1;

constexpr static const int32_t QEM_STATUS_PANIC = -2;

constexpr static const int32_t QEM_STATUS_INSUFFICIENT_BUFFER = -3;

constexpr static const uint32_t QEM_PROGRESS_SCOPE_MESH = 0;

constexpr static const uint32_t QEM_PROGRESS_SCOPE_SCENE = 1;

constexpr static const uint32_t QEM_PROGRESS_STAGE_BEGIN = 0;

constexpr static const uint32_t QEM_PROGRESS_STAGE_UPDATE = 1;

constexpr static const uint32_t QEM_PROGRESS_STAGE_END = 2;

constexpr static const uint32_t QEM_SCENE_WEIGHT_UNIFORM = 0;

constexpr static const uint32_t QEM_SCENE_WEIGHT_MESH_VOLUME = 1;

constexpr static const uint32_t QEM_SCENE_WEIGHT_MESH_VOLUME_X_INSTANCES = 2;

constexpr static const uint32_t QEM_SCENE_WEIGHT_EXTERNAL = 3;

using LogCallback = void(*)(const char*);

struct QemProgressEvent {
  uint32_t scope;
  uint32_t stage;
  float percent;
  uint32_t mesh_index;
  uint32_t mesh_count;
  uint32_t source_triangles;
  uint32_t target_triangles;
  uint32_t output_triangles;
  int32_t status;
};

using ProgressCallback = void(*)(const QemProgressEvent*, void*);

struct QemSimplifyResult {
  int32_t status;
  float max_error;
  uint32_t num_vertices;
  uint32_t num_indices;
  uint32_t num_triangles;
};

struct QemMeshView {
  float *vertices;
  uint32_t num_vertices;
  uint32_t *indices;
  uint32_t num_indices;
  int32_t *material_ids;
  uint32_t num_attributes;
  const float *attribute_weights;
};

struct QemSimplifyOptions {
  uint32_t target_vertices;
  uint32_t target_triangles;
  float target_error;
  uint32_t min_vertices;
  uint32_t min_triangles;
  float limit_error;
  float edge_weight;
  float max_edge_length_factor;
  uint8_t preserve_surface_area;
};

struct QemSceneMeshDecision {
  uint32_t mesh_index;
  uint32_t mesh_id;
  uint32_t source_triangles;
  double source_effective_triangles;
  double importance_weight;
  float keep_ratio;
  uint32_t target_triangles;
};

struct QemSceneMeshResult {
  uint32_t mesh_index;
  uint32_t mesh_id;
  int32_t status;
  uint32_t source_triangles;
  uint32_t requested_triangles;
  uint32_t output_triangles;
  float max_error;
};

struct QemSceneMeshStatistics {
  uint32_t mesh_index;
  uint32_t mesh_id;
  int32_t status;
  uint32_t source_triangles;
  uint32_t target_triangles;
  uint32_t output_triangles;
  double source_effective_triangles;
  double target_effective_triangles;
  double output_effective_triangles;
  float target_ratio;
  float achieved_ratio;
  float budget_deviation;
  float max_error;
};

struct QemSceneStatisticsSummary {
  int32_t status;
  uint32_t num_meshes;
  uint32_t num_failed_meshes;
  uint32_t num_simplified_meshes;
  uint64_t source_triangles;
  uint64_t target_triangles;
  uint64_t output_triangles;
  float target_scene_ratio;
  float achieved_scene_ratio;
  float target_hit_ratio;
  float mean_abs_budget_deviation;
  float max_abs_budget_deviation;
  float mean_max_error;
};

struct QemSceneMeshView {
  uint32_t mesh_id;
  QemMeshView mesh;
};

struct QemSceneGraphNodeView {
  int32_t parent_index;
  float local_matrix[16];
};

struct QemSceneGraphMeshBindingView {
  uint32_t node_index;
  uint32_t mesh_index;
  float mesh_to_node_matrix[16];
  uint8_t use_mesh_to_node_matrix;
};

struct QemSceneGraphView {
  QemSceneMeshView *meshes;
  uint32_t num_meshes;
  const QemSceneGraphNodeView *nodes;
  uint32_t num_nodes;
  const QemSceneGraphMeshBindingView *mesh_bindings;
  uint32_t num_mesh_bindings;
};

struct QemScenePolicy {
  float target_triangle_ratio;
  float min_mesh_ratio;
  float max_mesh_ratio;
  uint32_t weight_mode;
  uint8_t use_world_scale;
  uint64_t target_total_triangles;
  uint32_t min_triangles_per_mesh;
  float weight_exponent;
  uint8_t enable_parallel;
  uint32_t max_parallel_tasks;
  const float *external_importance_weights;
  uint32_t external_importance_count;
};

struct QemSceneMeshFeature {
  uint32_t mesh_index;
  uint32_t mesh_id;
  uint32_t source_triangles;
  uint32_t instance_count;
  double world_scale_sum;
  double bbox_volume;
  double importance_weight;
};

struct QemSceneSimplifyResult {
  int32_t status;
  uint32_t num_meshes;
  uint32_t num_decisions;
  uint32_t num_simplified_meshes;
  uint64_t source_triangles;
  uint64_t target_triangles;
  uint64_t output_triangles;
  double source_effective_triangles;
  double target_effective_triangles;
};

struct QemSceneExecutionOptions {
  uint8_t enable_parallel;
  uint32_t max_parallel_tasks;
  uint32_t retry_count;
  float fallback_relaxation_step;
};

extern "C" {

void register_log_callback(LogCallback callback);

int32_t qem_context_set_progress_callback(void *context,
                                          ProgressCallback callback,
                                          void *user_data);

int32_t qem_context_clear_progress_callback(void *context);

uint32_t qem_get_abi_version();

void *qem_context_create();

void qem_context_destroy(void *context);

int32_t qem_get_last_result(const void *context, QemSimplifyResult *out_result);

int32_t qem_simplify(void *context,
                     QemMeshView *mesh,
                     const QemSimplifyOptions *options,
                     QemSimplifyResult *out_result);

int32_t qem_scene_compute_statistics(const QemSceneMeshDecision *decisions,
                                     uint32_t num_decisions,
                                     const QemSceneMeshResult *mesh_results,
                                     uint32_t num_mesh_results,
                                     QemSceneMeshStatistics *out_statistics,
                                     uint32_t statistics_capacity,
                                     uint32_t *out_statistics_count,
                                     QemSceneStatisticsSummary *out_summary);

int32_t qem_scene_export_statistics_csv(const QemSceneMeshStatistics *mesh_statistics,
                                        uint32_t num_mesh_statistics,
                                        const QemSceneStatisticsSummary *summary,
                                        char *out_buffer,
                                        uint32_t buffer_capacity,
                                        uint32_t *out_required_size);

int32_t qem_scene_graph_extract_features(const QemSceneGraphView *graph,
                                         const QemScenePolicy *policy,
                                         QemSceneMeshFeature *out_features,
                                         uint32_t feature_capacity,
                                         uint32_t *out_feature_count,
                                         QemSceneSimplifyResult *out_result);

int32_t qem_scene_graph_compute_decisions(const QemSceneGraphView *graph,
                                          const QemScenePolicy *policy,
                                          QemSceneMeshDecision *out_decisions,
                                          uint32_t decision_capacity,
                                          uint32_t *out_decision_count,
                                          QemSceneSimplifyResult *out_result);

int32_t qem_scene_graph_apply_decisions(void *context,
                                        QemSceneGraphView *scene_graph,
                                        const QemSceneMeshDecision *decisions,
                                        uint32_t num_decisions,
                                        const QemSimplifyOptions *base_options,
                                        QemSceneMeshResult *out_mesh_results,
                                        uint32_t mesh_result_capacity,
                                        QemSceneSimplifyResult *out_result);

int32_t qem_scene_graph_apply_decisions_ex(void *context,
                                           QemSceneGraphView *scene_graph,
                                           const QemScenePolicy *policy,
                                           const QemSceneMeshDecision *decisions,
                                           uint32_t num_decisions,
                                           const QemSimplifyOptions *base_options,
                                           const QemSceneExecutionOptions *execution_options,
                                           QemSceneMeshResult *out_mesh_results,
                                           uint32_t mesh_result_capacity,
                                           QemSceneSimplifyResult *out_result);

int32_t qem_scene_graph_simplify(void *context,
                                 QemSceneGraphView *scene_graph,
                                 const QemScenePolicy *policy,
                                 const QemSimplifyOptions *base_options,
                                 QemSceneMeshDecision *out_decisions,
                                 uint32_t decision_capacity,
                                 uint32_t *out_decision_count,
                                 QemSceneMeshResult *out_mesh_results,
                                 uint32_t mesh_result_capacity,
                                 QemSceneSimplifyResult *out_result);

int32_t qem_scene_graph_simplify_ex(void *context,
                                    QemSceneGraphView *scene_graph,
                                    const QemScenePolicy *policy,
                                    const QemSimplifyOptions *base_options,
                                    const QemSceneExecutionOptions *execution_options,
                                    QemSceneMeshDecision *out_decisions,
                                    uint32_t decision_capacity,
                                    uint32_t *out_decision_count,
                                    QemSceneMeshResult *out_mesh_results,
                                    uint32_t mesh_result_capacity,
                                    QemSceneSimplifyResult *out_result);

} // extern "C"

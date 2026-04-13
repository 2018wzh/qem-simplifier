#include <cstdarg>
#include <cstdint>
#include <cstdlib>
#include <ostream>
#include <new>

constexpr static const int32_t QEM_STATUS_SUCCESS = 0;

constexpr static const int32_t QEM_STATUS_INVALID_ARGUMENT = -1;

constexpr static const int32_t QEM_STATUS_PANIC = -2;

using LogCallback = void(*)(const char*);

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

extern "C" {

void register_log_callback(LogCallback callback);

uint32_t qem_get_abi_version();

void *qem_context_create();

void qem_context_destroy(void *context);

int32_t qem_get_last_result(const void *context, QemSimplifyResult *out_result);

int32_t qem_simplify(void *context,
                     QemMeshView *mesh,
                     const QemSimplifyOptions *options,
                     QemSimplifyResult *out_result);

} // extern "C"

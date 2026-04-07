#include <cstdarg>
#include <cstdint>
#include <cstdlib>
#include <ostream>
#include <new>

constexpr static const int32_t QEM_STATUS_SUCCESS = 0;

constexpr static const int32_t QEM_STATUS_INVALID_ARGUMENT = -1;

constexpr static const int32_t QEM_STATUS_PANIC = -2;

constexpr static const double SMALL_NUMBER = 1e-8;

constexpr static const double KINDA_SMALL_NUMBER = 1e-4;

constexpr static const uint8_t FMeshSimplifier_MERGE_MASK = 3;

constexpr static const uint8_t FMeshSimplifier_ADJ_TRI_MASK = (1 << 2);

constexpr static const uint8_t FMeshSimplifier_LOCKED_VERT_MASK = (1 << 3);

constexpr static const uint8_t FMeshSimplifier_REMOVE_TRI_MASK = (1 << 4);

using LogCallback = void(*)(const char*);

#if defined(_MSC_VER)
#define QEM_DEPRECATED(msg) __declspec(deprecated(msg))
#elif defined(__GNUC__) || defined(__clang__)
#define QEM_DEPRECATED(msg) __attribute__((deprecated(msg)))
#else
#define QEM_DEPRECATED(msg)
#endif

struct QemSimplifyResult {
  int32_t status;
  float max_error;
  uint32_t num_verts;
  uint32_t num_indexes;
  uint32_t num_tris;
};

struct QemMeshView {
  float *verts;
  uint32_t num_verts;
  uint32_t *indexes;
  uint32_t num_indexes;
  int32_t *material_indexes;
  uint32_t num_attributes;
  const float *attribute_weights;
};

struct QemSimplifyOptions {
  uint32_t target_num_verts;
  uint32_t target_num_tris;
  float target_error;
  uint32_t limit_num_verts;
  uint32_t limit_num_tris;
  float limit_error;
  float edge_weight;
  float max_edge_length_factor;
  uint8_t preserve_surface_area;
};

struct SimplifySettings {
  uint32_t target_num_verts;
  uint32_t target_num_tris;
  float target_error;
  uint32_t limit_num_verts;
  uint32_t limit_num_tris;
  float limit_error;
  float edge_weight;
  float max_edge_length_factor;
  bool preserve_surface_area;
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

QEM_DEPRECATED("Use qem_simplify (ABI v2). Legacy ABI is for internal correctness testing only.")
float simplify_mesh(float *verts,
                    uint32_t num_verts,
                    uint32_t *indexes,
                    uint32_t num_indexes,
                    int32_t *material_indexes,
                    uint32_t num_attributes,
                    const float *attribute_weights,
                    SimplifySettings settings);

QEM_DEPRECATED("Use qem_get_last_result (ABI v2). Legacy ABI is for internal correctness testing only.")
void get_last_simplify_result(uint32_t *out_num_verts,
                              uint32_t *out_num_indexes,
                              uint32_t *out_num_tris);

} // extern "C"

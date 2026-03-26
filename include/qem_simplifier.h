#include <cstdarg>
#include <cstdint>
#include <cstdlib>
#include <ostream>
#include <new>

constexpr static const double SMALL_NUMBER = 1e-8;

constexpr static const double KINDA_SMALL_NUMBER = 1e-4;

constexpr static const uint8_t FMeshSimplifier_MERGE_MASK = 3;

constexpr static const uint8_t FMeshSimplifier_ADJ_TRI_MASK = (1 << 2);

constexpr static const uint8_t FMeshSimplifier_LOCKED_VERT_MASK = (1 << 3);

constexpr static const uint8_t FMeshSimplifier_REMOVE_TRI_MASK = (1 << 4);

using LogCallback = void(*)(const char*);

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

float simplify_mesh(float *verts,
                    uint32_t num_verts,
                    uint32_t *indexes,
                    uint32_t num_indexes,
                    int32_t *material_indexes,
                    uint32_t num_attributes,
                    const float *attribute_weights,
                    SimplifySettings settings);

} // extern "C"

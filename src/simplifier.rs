use crate::log_internal;
use crate::quadric::*;
use crate::util::binary_heap::*;
use crate::util::disjoint_set::*;
use crate::util::hash::*;

// TODO: Use smallvec to implement TInlineAllocator

pub fn hash_position(position: Vec3f) -> u32 {
    let mut x_bits = position.x.to_bits();
    let mut y_bits = position.y.to_bits();
    let mut z_bits = position.z.to_bits();

    if position.x == 0.0 {
        x_bits = 0;
    }
    if position.y == 0.0 {
        y_bits = 0;
    }
    if position.z == 0.0 {
        z_bits = 0;
    }

    murmur32(&[x_bits, y_bits, z_bits])
}

#[inline(always)]
pub fn cycle3(value: u32) -> u32 {
    let value_mod3 = value % 3;
    let value1_mod3 = (1 << value_mod3) & 3;
    value - value_mod3 + value1_mod3
}

#[inline(always)]
pub fn cycle3_offset(value: u32, offset: u32) -> u32 {
    value - value % 3 + (value + offset) % 3
}

#[derive(Clone, Copy, Debug)]
pub struct VertexPair {
    pub position0: Vec3f,
    pub position1: Vec3f,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MaterialDeltas {
    pub surface_area: f32,
    pub num_tris: i32,
    pub num_disjoint: i32,
}

pub struct MeshSimplifier<'a> {
    pub vertex_count: u32,
    pub index_count: u32,
    pub num_attributes: u32,
    pub triangle_count: u32,

    pub remaining_vertices: u32,
    pub remaining_triangles: u32,

    pub vertices: &'a mut [f32],
    pub indices: &'a mut [u32],
    pub material_ids: &'a mut [i32],

    pub attribute_weights: &'a [f32],
    pub edge_weight: f32,
    pub max_edge_length_factor: f32,
    pub correct_attributes: Option<fn(&mut [f32])>,
    pub limit_error_to_surface_area: bool,
    pub zero_weights: bool,

    pub vert_hash: HashTable,
    pub corner_hash: HashTable,

    pub vert_ref_count: Vec<u32>,
    pub corner_flags: Vec<u8>,
    pub tri_removed: Vec<bool>,

    pub per_material_deltas: Vec<MaterialDeltas>,

    pub pairs: Vec<VertexPair>,
    pub pair_hash0: HashTable,
    pub pair_hash1: HashTable,
    pub pair_heap: BinaryHeap<f32, u32>,

    pub moved_vertices: Vec<u32>,
    pub moved_corners: Vec<u32>,
    pub moved_pairs: Vec<u32>,
    pub reevaluate_pairs: Vec<u32>,

    pub tri_quadrics: Vec<QuadricAttr>,
    pub edge_quadrics: Vec<EdgeQuadric>,
    pub edge_quadrics_valid: Vec<bool>,

    pub wedge_attributes: Vec<f32>,
    pub wedge_disjoint_set: DisjointSet,

    pub degree_limit: i32,
    pub degree_penalty: f32,
    pub lock_penalty: f32,
    pub inversion_penalty: f32,
}

struct WedgeVertex {
    vert_index: u32,
    adj_tri_index: u32,
}

impl<'a> MeshSimplifier<'a> {
    // TODO: Consider use bitflags
    pub const MERGE_MASK: u8 = 3;
    pub const ADJ_TRI_MASK: u8 = 1 << 2;
    pub const LOCKED_VERT_MASK: u8 = 1 << 3;
    pub const REMOVE_TRI_MASK: u8 = 1 << 4;

    pub fn new(
        vertices: &'a mut [f32],
        vertex_count: u32,
        indices: &'a mut [u32],
        index_count: u32,
        material_ids: &'a mut [i32],
        num_attributes: u32,
        attribute_weights: &'a [f32],
    ) -> Self {
        let triangle_count = index_count / 3;
        let vert_hash_size = 1 << 16.min((vertex_count as f32).log2() as u32);
        let corner_hash_size = 1 << 16.min((index_count as f32).log2() as u32);
        let mut vert_hash = HashTable::new(vert_hash_size, vertex_count);
        let corner_hash = HashTable::new(corner_hash_size, index_count);

        for vert_index in 0..vertex_count {
            let pos = Self::get_position_static(vertices, num_attributes, vert_index);
            vert_hash.add(hash_position(pos), vert_index);
        }

        let vert_ref_count = vec![0u32; vertex_count as usize];
        let corner_flags = vec![0u8; index_count as usize];

        let edge_quadrics = vec![EdgeQuadric::default(); index_count as usize];
        let edge_quadrics_valid = vec![false; index_count as usize];

        let num_edges_guess = index_count
            .min(3 * vertex_count - 6)
            .min(triangle_count + vertex_count);
        let pair_hash_size = 1 << 16.min((num_edges_guess as f32).log2() as u32);
        let pair_hash0 = HashTable::new(pair_hash_size, num_edges_guess);
        let pair_hash1 = HashTable::new(pair_hash_size, num_edges_guess);

        let mut simplifier = Self {
            vertex_count,
            index_count,
            num_attributes,
            triangle_count,
            remaining_vertices: vertex_count,
            remaining_triangles: triangle_count,
            vertices,
            indices,
            material_ids,
            attribute_weights,
            edge_weight: 8.0,
            max_edge_length_factor: 0.0,
            correct_attributes: None,
            limit_error_to_surface_area: true,
            zero_weights: false,
            vert_hash,
            corner_hash,
            vert_ref_count,
            corner_flags,
            tri_removed: vec![false; triangle_count as usize],
            per_material_deltas: Vec::new(),
            pairs: Vec::with_capacity(num_edges_guess as usize),
            pair_hash0,
            pair_hash1,
            pair_heap: BinaryHeap::new(num_edges_guess, num_edges_guess),
            moved_vertices: Vec::new(),
            moved_corners: Vec::new(),
            moved_pairs: Vec::new(),
            reevaluate_pairs: Vec::new(),
            tri_quadrics: vec![QuadricAttr::default(); triangle_count as usize],
            edge_quadrics,
            edge_quadrics_valid,
            wedge_attributes: Vec::new(),
            wedge_disjoint_set: DisjointSet::new(0),
            degree_limit: 24,
            degree_penalty: 0.5,
            lock_penalty: 1e8,
            inversion_penalty: 100.0,
        };

        for corner in 0..index_count {
            let vert_index = simplifier.indices[corner as usize];

            simplifier.vert_ref_count[vert_index as usize] += 1;

            let pos = simplifier.get_position(vert_index);
            simplifier.corner_hash.add(hash_position(pos), corner);

            let mut pair = VertexPair {
                position0: pos,
                position1: simplifier.get_position(simplifier.indices[cycle3(corner) as usize]),
            };

            let pair_index = simplifier.pairs.len() as u32;
            if simplifier.add_unique_pair(&mut pair, pair_index) {
                simplifier.pairs.push(pair);
            }
        }

        log_internal(&format!(
            "Simplifier initialized: {} vertices, {} triangles, {} pairs",
            vertex_count,
            triangle_count,
            simplifier.pairs.len()
        ));

        simplifier
    }

    fn get_position_static(vertices: &[f32], num_attributes: u32, vert_index: u32) -> Vec3f {
        let base = (3 + num_attributes) as usize * vert_index as usize;
        Vec3f::new(vertices[base], vertices[base + 1], vertices[base + 2])
    }

    // TODO: 当前通过 unsafe 返回连续内存中的 Vec3f 可变引用，后续可进一步封装以减少 unsafe 暴露。
    pub fn get_position(&self, vert_index: u32) -> Vec3f {
        Self::get_position_static(self.vertices, self.num_attributes, vert_index)
    }

    pub fn get_position_mut(&mut self, vert_index: u32) -> &mut Vec3f {
        let base = (3 + self.num_attributes) as usize * vert_index as usize;
        unsafe { &mut *(self.vertices.as_mut_ptr().add(base) as *mut Vec3f) }
    }

    pub fn get_attributes(&self, vert_index: u32) -> &[f32] {
        let base = (3 + self.num_attributes) as usize * vert_index as usize + 3;
        &self.vertices[base..base + self.num_attributes as usize]
    }

    pub fn get_attributes_mut(&mut self, vert_index: u32) -> &mut [f32] {
        let base = (3 + self.num_attributes) as usize * vert_index as usize + 3;
        &mut self.vertices[base..base + self.num_attributes as usize]
    }

    fn add_unique_pair(&mut self, pair: &mut VertexPair, pair_index: u32) -> bool {
        let mut hash0 = hash_position(pair.position0);
        let mut hash1 = hash_position(pair.position1);

        if hash0 > hash1 {
            std::mem::swap(&mut hash0, &mut hash1);
            std::mem::swap(&mut pair.position0, &mut pair.position1);
        }

        let mut other_pair_index = self.pair_hash0.first(hash0);
        while self.pair_hash0.is_valid(other_pair_index) {
            // assert_ne!(pair_index, other_pair_index);
            let other_pair = &self.pairs[other_pair_index as usize];
            if pair.position0 == other_pair.position0 && pair.position1 == other_pair.position1 {
                return false;
            }
            other_pair_index = self.pair_hash0.next(other_pair_index);
        }

        self.pair_hash0.add(hash0, pair_index);
        self.pair_hash1.add(hash1, pair_index);
        true
    }

    // TODO: 可进一步优化三角形二次误差计算路径。
    pub fn calc_tri_quadric(&mut self, tri_index: u32) {
        let i0 = self.indices[(tri_index * 3 + 0) as usize];
        let i1 = self.indices[(tri_index * 3 + 1) as usize];
        let i2 = self.indices[(tri_index * 3 + 2) as usize];

        self.tri_quadrics[tri_index as usize] = QuadricAttr::new(
            self.get_position(i0).into(),
            self.get_position(i1).into(),
            self.get_position(i2).into(),
            self.get_attributes(i0),
            self.get_attributes(i1),
            self.get_attributes(i2),
            self.attribute_weights,
            self.num_attributes as usize,
        );
    }

    pub fn calc_edge_quadric(&mut self, edge_index: u32) {
        let tri_index = edge_index / 3;
        if self.tri_removed[tri_index as usize] {
            self.edge_quadrics_valid[edge_index as usize] = false;
            return;
        }

        let material_index = self.material_ids[tri_index as usize];

        let vert_index0 = self.indices[edge_index as usize];
        let vert_index1 = self.indices[cycle3(edge_index) as usize];

        let position0 = self.get_position(vert_index0);
        let position1 = self.get_position(vert_index1);

        let hash = hash_position(position1);
        let mut corner = self.corner_hash.first(hash);
        while self.corner_hash.is_valid(corner) {
            let other_vert_index0 = self.indices[corner as usize];
            let other_vert_index1 = self.indices[cycle3(corner) as usize];

            if vert_index0 == other_vert_index1
                && vert_index1 == other_vert_index0
                && material_index == self.material_ids[(corner / 3) as usize]
            {
                self.edge_quadrics_valid[edge_index as usize] = false;
                return;
            }
            corner = self.corner_hash.next(corner);
        }

        let mut weight = self.edge_weight;
        corner = self.corner_hash.first(hash);
        while self.corner_hash.is_valid(corner) {
            let other_vert_index0 = self.indices[corner as usize];
            let other_vert_index1 = self.indices[cycle3(corner) as usize];

            if position0 == self.get_position(other_vert_index1)
                && position1 == self.get_position(other_vert_index0)
            {
                weight *= 0.5;
                break;
            }
            corner = self.corner_hash.next(corner);
        }

        self.edge_quadrics[edge_index as usize] =
            EdgeQuadric::new_with_weight(position0.into(), position1.into(), weight);
        self.edge_quadrics_valid[edge_index as usize] = true;
    }

    pub fn lock_position(&mut self, position: Vec3f) {
        let hash = hash_position(position);
        let mut corner = self.corner_hash.first(hash);
        while self.corner_hash.is_valid(corner) {
            if self.get_position(self.indices[corner as usize]) == position {
                self.corner_flags[corner as usize] |= Self::LOCKED_VERT_MASK;
            }
            corner = self.corner_hash.next(corner);
        }
    }

    #[allow(dead_code)]
    fn for_all_corners<F>(&self, position: Vec3f, mut func: F)
    where
        F: FnMut(u32),
    {
        let hash = hash_position(position);
        let mut corner = self.corner_hash.first(hash);
        while self.corner_hash.is_valid(corner) {
            if self.get_position(self.indices[corner as usize]) == position {
                func(corner);
            }
            corner = self.corner_hash.next(corner);
        }
    }

    fn for_all_pairs<F>(&self, position: Vec3f, mut func: F)
    where
        F: FnMut(u32),
    {
        let hash = hash_position(position);
        let mut pair_index = self.pair_hash0.first(hash);
        while self.pair_hash0.is_valid(pair_index) {
            if self.pairs[pair_index as usize].position0 == position {
                func(pair_index);
            }
            pair_index = self.pair_hash0.next(pair_index);
        }

        pair_index = self.pair_hash1.first(hash);
        while self.pair_hash1.is_valid(pair_index) {
            if self.pairs[pair_index as usize].position1 == position {
                func(pair_index);
            }
            pair_index = self.pair_hash1.next(pair_index);
        }
    }

    pub fn evaluate_merge(
        &mut self,
        position0: Vec3f,
        position1: Vec3f,
        move_vertices: bool,
    ) -> f32 {
        if position0 == position1 {
            return 0.0;
        }

        self.wedge_disjoint_set.reset();
        let mut adj_tris = Vec::new();
        let mut wedge_vertices0: Vec<WedgeVertex> = Vec::new();
        let mut wedge_vertices1: Vec<WedgeVertex> = Vec::new();
        let mut vert_degree = 0;

        let mut flags_union0 = 0u32;
        let mut flags_union1 = 0u32;

        for (pos, index, flags_union, wedge_vertices) in [
            (position0, 0, &mut flags_union0, &mut wedge_vertices0),
            (position1, 1, &mut flags_union1, &mut wedge_vertices1),
        ] {
            let hash = hash_position(pos);
            let mut corner = self.corner_hash.first(hash);
            while self.corner_hash.is_valid(corner) {
                if self.get_position(self.indices[corner as usize]) == pos {
                    vert_degree += 1;
                    self.corner_flags[corner as usize] |= 1 << index;
                    *flags_union |= self.corner_flags[corner as usize] as u32;

                    let tri_index = corner / 3;
                    let adj_tri_index;
                    let mut new_triangle = true;

                    let first_corner_flag = &mut self.corner_flags[(tri_index * 3) as usize];
                    if (*first_corner_flag & Self::ADJ_TRI_MASK) == 0 {
                        *first_corner_flag |= Self::ADJ_TRI_MASK;
                        adj_tri_index = adj_tris.len() as u32;
                        adj_tris.push(tri_index);
                        self.wedge_disjoint_set.add_defaulted();
                    } else {
                        adj_tri_index =
                            adj_tris.iter().position(|&x| x == tri_index).unwrap() as u32;
                        new_triangle = false;
                    }

                    let vert_index = self.indices[corner as usize];
                    let mut other_adj_tri_index = u32::MAX;
                    for wv in wedge_vertices.iter() {
                        if wv.vert_index == vert_index {
                            other_adj_tri_index = wv.adj_tri_index;
                            break;
                        }
                    }

                    if other_adj_tri_index == u32::MAX {
                        wedge_vertices.push(WedgeVertex {
                            vert_index,
                            adj_tri_index,
                        });
                    } else {
                        if new_triangle {
                            self.wedge_disjoint_set
                                .union_sequential(adj_tri_index, other_adj_tri_index);
                        } else {
                            self.wedge_disjoint_set
                                .union(adj_tri_index, other_adj_tri_index);
                        }
                    }
                }
                corner = self.corner_hash.next(corner);
            }
        }

        if vert_degree == 0 {
            return 0.0;
        }

        if vert_degree as u32 == self.remaining_triangles * 2 {
            for &tri_index in &adj_tris {
                for corner_index in 0..3 {
                    self.corner_flags[(tri_index * 3 + corner_index) as usize] &=
                        !(Self::MERGE_MASK | Self::ADJ_TRI_MASK);
                }
            }
            return 0.0;
        }

        let is_locked0 = (flags_union0 & Self::LOCKED_VERT_MASK as u32) != 0;
        let is_locked1 = (flags_union1 & Self::LOCKED_VERT_MASK as u32) != 0;

        let mut penalty = 0.0f32;
        if vert_degree > self.degree_limit {
            penalty += self.degree_penalty * (vert_degree - self.degree_limit) as f32;
        }

        let mut wedge_ids = Vec::new();
        let mut wedge_quadrics: Vec<QuadricAttr> = Vec::new();

        for (adj_tri_idx, &tri_index) in adj_tris.iter().enumerate() {
            let tri_quadric = self.tri_quadrics[tri_index as usize].clone();
            let wedge_id = self.wedge_disjoint_set.find(adj_tri_idx as u32);

            if let Some(wedge_index) = wedge_ids.iter().position(|&id| id == wedge_id) {
                let vert_index0 = self.indices[(tri_index * 3) as usize];
                let pos0 = self.get_position(vert_index0);
                let attrs0 = self.get_attributes(vert_index0);
                wedge_quadrics[wedge_index].add(
                    &tri_quadric,
                    (pos0 - position0).into(),
                    attrs0,
                    self.attribute_weights,
                    self.num_attributes as usize,
                );
            } else {
                wedge_ids.push(wedge_id);
                let mut wedge_quadric = tri_quadric;
                let vert_index0 = self.indices[(tri_index * 3) as usize];
                let pos0 = self.get_position(vert_index0);
                let attrs0 = self.get_attributes(vert_index0);
                wedge_quadric.rebase(
                    (pos0 - position0).into(),
                    attrs0,
                    self.attribute_weights,
                    self.num_attributes as usize,
                );
                wedge_quadrics.push(wedge_quadric);
            }
        }

        let mut quadric_optimizer = QuadricAttrOptimizer::default();
        for wq in &wedge_quadrics {
            quadric_optimizer.add_quadric_attr(wq, self.num_attributes as usize);
        }

        let mut bounds_min = Vec3f::new(f32::MAX, f32::MAX, f32::MAX);
        let mut bounds_max = Vec3f::new(f32::MIN, f32::MIN, f32::MIN);
        let mut edge_quadric = Quadric::default();
        edge_quadric.zero();

        for &tri_index in &adj_tris {
            for corner_index in 0..3 {
                let corner = tri_index * 3 + corner_index;
                let pos = self.get_position(self.indices[corner as usize]);
                bounds_min.x = bounds_min.x.min(pos.x);
                bounds_min.y = bounds_min.y.min(pos.y);
                bounds_min.z = bounds_min.z.min(pos.z);
                bounds_max.x = bounds_max.x.max(pos.x);
                bounds_max.y = bounds_max.y.max(pos.y);
                bounds_max.z = bounds_max.z.max(pos.z);

                if self.edge_quadrics_valid[corner as usize] {
                    let mut edge_flags = self.corner_flags[corner as usize];
                    edge_flags |=
                        self.corner_flags[(tri_index * 3 + ((1 << corner_index) & 3)) as usize];
                    if (edge_flags & Self::MERGE_MASK) != 0 {
                        let vert_index0 = self.indices[corner as usize];
                        edge_quadric.add_edge_quadric(
                            &self.edge_quadrics[corner as usize],
                            (self.get_position(vert_index0) - position0).into(),
                        );
                    }
                }
            }
        }

        quadric_optimizer.add_quadric(&edge_quadric);
        let is_valid_position =
            |pos: Vec3f, simplifier: &MeshSimplifier, adj_tris: &Vec<u32>| -> bool {
                let mut dist_sq = 0.0f32;
                if pos.x < bounds_min.x {
                    dist_sq += (bounds_min.x - pos.x).powi(2);
                } else if pos.x > bounds_max.x {
                    dist_sq += (pos.x - bounds_max.x).powi(2);
                }
                if pos.y < bounds_min.y {
                    dist_sq += (bounds_min.y - pos.y).powi(2);
                } else if pos.y > bounds_max.y {
                    dist_sq += (pos.y - bounds_max.y).powi(2);
                }
                if pos.z < bounds_min.z {
                    dist_sq += (bounds_min.z - pos.z).powi(2);
                } else if pos.z > bounds_max.z {
                    dist_sq += (pos.z - bounds_max.z).powi(2);
                }

                let bounds_size_sq = (bounds_max - bounds_min).length_sq();
                if dist_sq > bounds_size_sq * 4.0 {
                    return false;
                }

                for &tri_idx in adj_tris {
                    if simplifier.tri_will_invert(tri_idx, pos) {
                        return false;
                    }
                }
                true
            };

        let mut final_new_position = Vec3f::default();

        if is_locked0 && is_locked1 {
            penalty += self.lock_penalty;
        }

        let mut found_pos = false;
        if is_locked0 && !is_locked1 {
            final_new_position = position0;
            if !is_valid_position(final_new_position, self, &adj_tris) {
                penalty += self.inversion_penalty;
            }
        } else if is_locked1 && !is_locked0 {
            final_new_position = position1;
            if !is_valid_position(final_new_position, self, &adj_tris) {
                penalty += self.inversion_penalty;
            }
        } else {
            let mut pos = QVec3::default();
            if quadric_optimizer.optimize_volume(&mut pos) {
                let p: Vec3f = (pos + position0.into()).into();
                if is_valid_position(p, self, &adj_tris) {
                    final_new_position = p;
                    found_pos = true;
                }
            }
            if !found_pos && quadric_optimizer.optimize(&mut pos) {
                let p: Vec3f = (pos + position0.into()).into();
                if is_valid_position(p, self, &adj_tris) {
                    final_new_position = p;
                    found_pos = true;
                }
            }
            if !found_pos {
                if quadric_optimizer.optimize_linear(
                    QVec3::default(),
                    (position1 - position0).into(),
                    &mut pos,
                ) {
                    let p: Vec3f = (pos + position0.into()).into();
                    if is_valid_position(p, self, &adj_tris) {
                        final_new_position = p;
                        found_pos = true;
                    }
                }
            }
            if !found_pos {
                final_new_position = (position0 + position1) * 0.5;
                if !is_valid_position(final_new_position, self, &adj_tris) {
                    penalty += self.inversion_penalty;
                }
            }
        }

        let num_wedges = wedge_ids.len();
        self.wedge_attributes.clear();
        self.wedge_attributes
            .resize(num_wedges * self.num_attributes as usize, 0.0);

        let new_position_rebase: QVec3 = (final_new_position - position0).into();

        if is_locked0 != is_locked1 || self.zero_weights {
            let dist_sq0 = (final_new_position - position0).length_sq();
            let dist_sq1 = (final_new_position - position1).length_sq();
            let farthest = if dist_sq0 > dist_sq1 { 0 } else { 1 };

            for j in 0..2 {
                let wedge_vertices = if ((farthest + j) & 1) == 0 {
                    &wedge_vertices0
                } else {
                    &wedge_vertices1
                };
                for wv in wedge_vertices {
                    let root_id = self.wedge_disjoint_set.find(wv.adj_tri_index);
                    let wedge_index = wedge_ids.iter().position(|&id| id == root_id).unwrap();
                    let start = wedge_index * self.num_attributes as usize;
                    let num_attrs = self.num_attributes as usize;

                    let base = (3 + self.num_attributes) as usize * wv.vert_index as usize + 3;
                    for i in 0..num_attrs {
                        self.wedge_attributes[start + i] = self.vertices[base + i];
                    }
                }
            }
        }

        let mut error = 0.0f32;
        let edge_error = edge_quadric.evaluate(new_position_rebase) as f32;
        let mut surface_area = 0.0f32;

        for wedge_index in 0..num_wedges {
            let start = wedge_index * self.num_attributes as usize;
            let num_attrs = self.num_attributes as usize;
            let wedge_attrs = &mut self.wedge_attributes[start..start + num_attrs];

            let wedge_quadric = &wedge_quadrics[wedge_index];

            if wedge_quadric.base.a > 1e-8 {
                let mut wedge_error: f64;
                if is_locked0 != is_locked1 {
                    wedge_error = wedge_quadric.evaluate(
                        new_position_rebase,
                        wedge_attrs,
                        self.attribute_weights,
                        self.num_attributes as usize,
                    );
                } else {
                    wedge_error = wedge_quadric.calc_attributes_and_evaluate(
                        new_position_rebase,
                        wedge_attrs,
                        self.attribute_weights,
                        self.num_attributes as usize,
                    );
                    if let Some(correct) = self.correct_attributes {
                        correct(wedge_attrs);
                    }
                }
                if self.limit_error_to_surface_area {
                    wedge_error = wedge_error.min(wedge_quadric.base.a);
                }
                error += wedge_error as f32;
            } else {
                wedge_attrs.fill(0.0);
            }
            surface_area += wedge_quadric.base.a as f32;
        }

        error += edge_error;
        let is_disjoint = adj_tris.len() == 1 || (adj_tris.len() == 2 && vert_degree == 4);

        if self.limit_error_to_surface_area {
            error = error.min(surface_area);
            if is_disjoint {
                error = surface_area;
            }
        }

        if self.max_edge_length_factor > 0.0 {
            for &tri_index in &adj_tris {
                let index_moved = self.corner_index_moved(tri_index);
                if index_moved < 3 {
                    let corner = tri_index * 3 + index_moved;
                    let p1 = self.get_position(self.indices[cycle3(corner) as usize]);
                    let p2 = self.get_position(self.indices[cycle3_offset(corner, 2) as usize]);
                    error = error.max(
                        (final_new_position - p1).length_sq()
                            / (self.max_edge_length_factor * self.max_edge_length_factor),
                    );
                    error = error.max(
                        (final_new_position - p2).length_sq()
                            / (self.max_edge_length_factor * self.max_edge_length_factor),
                    );
                }
            }
        }

        if move_vertices {
            self.begin_move_position(position0);
            self.begin_move_position(position1);

            for (adj_tri_idx, &tri_index) in adj_tris.iter().enumerate() {
                let root_id = self.wedge_disjoint_set.find(adj_tri_idx as u32);
                let wedge_index = wedge_ids.iter().position(|&id| id == root_id).unwrap();

                for corner_index in 0..3 {
                    let corner = tri_index * 3 + corner_index;
                    let vert_index = self.indices[corner as usize];
                    let old_pos = self.get_position(vert_index);
                    if old_pos == position0 || old_pos == position1 {
                        *self.get_position_mut(vert_index) = final_new_position;
                        if wedge_quadrics[wedge_index].base.a > 1e-8 {
                            let start = wedge_index * self.num_attributes as usize;
                            let num_attrs = self.num_attributes as usize;
                            let base = (3 + self.num_attributes) as usize * vert_index as usize + 3;
                            for i in 0..num_attrs {
                                self.vertices[base + i] = self.wedge_attributes[start + i];
                            }
                        }
                        if is_locked0 || is_locked1 {
                            self.corner_flags[corner as usize] |= Self::LOCKED_VERT_MASK;
                        }
                    }
                }
            }

            for &pair_index in &self.moved_pairs {
                let pair = &mut self.pairs[pair_index as usize];
                if pair.position0 == position0 || pair.position0 == position1 {
                    pair.position0 = final_new_position;
                }
                if pair.position1 == position0 || pair.position1 == position1 {
                    pair.position1 = final_new_position;
                }
            }

            self.end_move_positions();

            let mut adjacent_vertices = Vec::new();
            for &tri_index in &adj_tris {
                for corner_index in 0..3 {
                    let v_idx = self.indices[(tri_index * 3 + corner_index) as usize];
                    if !adjacent_vertices.contains(&v_idx) {
                        adjacent_vertices.push(v_idx);
                    }
                }
            }

            for v_idx in adjacent_vertices {
                let pos = self.get_position(v_idx);
                let mut p_to_reevaluate = Vec::new();
                self.for_all_pairs(pos, |pair_idx| {
                    p_to_reevaluate.push(pair_idx);
                });
                for pair_idx in p_to_reevaluate {
                    if self.pair_heap.is_present(pair_idx) {
                        self.pair_heap.remove(pair_idx);
                        self.reevaluate_pairs.push(pair_idx);
                    }
                }
            }

            for &tri_index in &adj_tris {
                let material_index = (self.material_ids[tri_index as usize] & 0xffffff) as usize;
                if material_index >= self.per_material_deltas.len() {
                    self.per_material_deltas
                        .resize(material_index + 1, MaterialDeltas::default());
                }
                let old_a = self.tri_quadrics[tri_index as usize].base.a as f32;
                let delta = &mut self.per_material_deltas[material_index];
                delta.surface_area -= old_a;
                delta.num_tris -= 1;
                if is_disjoint {
                    delta.num_disjoint -= 1;
                }

                self.fix_up_tri(tri_index);
                if !self.tri_removed[tri_index as usize] {
                    let new_a = self.tri_quadrics[tri_index as usize].base.a as f32;
                    let delta = &mut self.per_material_deltas[material_index];
                    delta.surface_area += new_a;
                    delta.num_tris += 1;
                }
            }
        } else {
            error += penalty;
        }

        for &tri_index in &adj_tris {
            for corner_index in 0..3 {
                let corner = tri_index * 3 + corner_index;
                if move_vertices {
                    self.calc_edge_quadric(corner);
                }
                self.corner_flags[corner as usize] &= !(Self::MERGE_MASK | Self::ADJ_TRI_MASK);
            }
        }

        error
    }

    pub fn begin_move_position(&mut self, position: Vec3f) {
        let hash = hash_position(position);

        let mut verts_to_move = Vec::new();
        let mut vert_idx = self.vert_hash.first(hash);
        while self.vert_hash.is_valid(vert_idx) {
            if self.get_position(vert_idx) == position {
                verts_to_move.push(vert_idx);
            }
            vert_idx = self.vert_hash.next(vert_idx);
        }
        for v_idx in verts_to_move {
            self.vert_hash.remove(hash, v_idx);
            self.moved_vertices.push(v_idx);
        }

        let mut corners_to_move = Vec::new();
        let mut corner = self.corner_hash.first(hash);
        while self.corner_hash.is_valid(corner) {
            if self.get_position(self.indices[corner as usize]) == position {
                corners_to_move.push(corner);
            }
            corner = self.corner_hash.next(corner);
        }
        for c in corners_to_move {
            self.corner_hash.remove(hash, c);
            self.moved_corners.push(c);
        }

        let mut pairs_to_move = Vec::new();
        self.for_all_pairs(position, |pair_idx| {
            if !pairs_to_move.contains(&pair_idx) {
                pairs_to_move.push(pair_idx);
            }
        });
        for p_idx in pairs_to_move {
            self.pair_hash0
                .remove(hash_position(self.pairs[p_idx as usize].position0), p_idx);
            self.pair_hash1
                .remove(hash_position(self.pairs[p_idx as usize].position1), p_idx);
            self.moved_pairs.push(p_idx);
        }
    }

    pub fn end_move_positions(&mut self) {
        let moved_vertices = std::mem::take(&mut self.moved_vertices);
        for v_idx in moved_vertices {
            self.vert_hash
                .add(hash_position(self.get_position(v_idx)), v_idx);
        }

        let moved_corners = std::mem::take(&mut self.moved_corners);
        for c in moved_corners {
            self.corner_hash.add(
                hash_position(self.get_position(self.indices[c as usize])),
                c,
            );
        }

        let moved_pairs = std::mem::take(&mut self.moved_pairs);
        for p_idx in moved_pairs {
            let mut pair = self.pairs[p_idx as usize];
            if pair.position0 == pair.position1 || !self.add_unique_pair(&mut pair, p_idx) {
                self.pair_heap.remove(p_idx);
            }
            self.pairs[p_idx as usize] = pair;
        }
    }

    pub fn corner_index_moved(&self, tri_index: u32) -> u32 {
        let mut index_moved = 3;
        for corner_index in 0..3 {
            let corner = tri_index * 3 + corner_index;
            if (self.corner_flags[corner as usize] & Self::MERGE_MASK) != 0 {
                if index_moved == 3 {
                    index_moved = corner_index;
                } else {
                    index_moved = 4;
                }
            }
        }
        index_moved
    }

    pub fn tri_will_invert(&self, tri_index: u32, new_position: Vec3f) -> bool {
        let index_moved = self.corner_index_moved(tri_index);
        if index_moved < 3 {
            let corner = tri_index * 3 + index_moved;
            let p0 = self.get_position(self.indices[corner as usize]);
            let p1 = self.get_position(self.indices[cycle3(corner) as usize]);
            let p2 = self.get_position(self.indices[cycle3_offset(corner, 2) as usize]);

            let d21 = p2 - p1;
            let d01 = p0 - p1;
            let dp1 = new_position - p1;

            let n0 = d01.cross(d21);
            let n1 = dp1.cross(d21);
            return n0.dot(n1) < 0.0;
        }
        false
    }

    pub fn remove_tri(&mut self, tri_index: u32) {
        assert!(!self.tri_removed[tri_index as usize]);
        self.tri_removed[tri_index as usize] = true;
        self.remaining_triangles -= 1;

        for k in 0..3 {
            let corner = tri_index * 3 + k;
            let vert_index = self.indices[corner as usize];
            let hash = hash_position(self.get_position(vert_index));
            self.corner_hash.remove(hash, corner);
            self.edge_quadrics_valid[corner as usize] = false;
            self.set_vert_index(corner, u32::MAX);
        }
    }

    pub fn fix_up_tri(&mut self, tri_index: u32) {
        assert!(!self.tri_removed[tri_index as usize]);
        let p0 = self.get_position(self.indices[(tri_index * 3 + 0) as usize]);
        let p1 = self.get_position(self.indices[(tri_index * 3 + 1) as usize]);
        let p2 = self.get_position(self.indices[(tri_index * 3 + 2) as usize]);

        let mut should_remove_triangle =
            (self.corner_flags[(tri_index * 3) as usize] & Self::REMOVE_TRI_MASK) != 0;
        if !should_remove_triangle {
            should_remove_triangle = p0 == p1 || p1 == p2 || p2 == p0;
        }

        if !should_remove_triangle {
            for k in 0..3 {
                self.remove_duplicate_vertices(tri_index * 3 + k);
            }
            should_remove_triangle = self.is_duplicate_tri(tri_index);
        }

        if should_remove_triangle {
            self.remove_tri(tri_index);
        } else {
            self.calc_tri_quadric(tri_index);
        }
    }

    pub fn is_duplicate_tri(&self, tri_index: u32) -> bool {
        let i0 = self.indices[(tri_index * 3 + 0) as usize];
        let i1 = self.indices[(tri_index * 3 + 1) as usize];
        let i2 = self.indices[(tri_index * 3 + 2) as usize];
        let hash = hash_position(self.get_position(i0));
        let mut corner = self.corner_hash.first(hash);
        while self.corner_hash.is_valid(corner) {
            if corner != tri_index * 3
                && i0 == self.indices[corner as usize]
                && i1 == self.indices[cycle3(corner) as usize]
                && i2 == self.indices[cycle3_offset(corner, 2) as usize]
            {
                return true;
            }
            corner = self.corner_hash.next(corner);
        }
        false
    }

    pub fn set_vert_index(&mut self, corner: u32, new_vert_index: u32) {
        let vert_index = self.indices[corner as usize];
        if vert_index == new_vert_index {
            return;
        }

        self.vert_ref_count[vert_index as usize] -= 1;
        if self.vert_ref_count[vert_index as usize] == 0 {
            self.vert_hash
                .remove(hash_position(self.get_position(vert_index)), vert_index);
            self.remaining_vertices -= 1;
        }

        self.indices[corner as usize] = new_vert_index;
        if new_vert_index != u32::MAX {
            self.vert_ref_count[new_vert_index as usize] += 1;
        }
    }

    pub fn remove_duplicate_vertices(&mut self, corner: u32) {
        let vert_index = self.indices[corner as usize];
        let num_floats = 3 + self.num_attributes;
        let mut vert_data = vec![0.0f32; num_floats as usize];
        vert_data.copy_from_slice(
            &self.vertices
                [(num_floats * vert_index) as usize..(num_floats * (vert_index + 1)) as usize],
        );

        let hash = hash_position(self.get_position(vert_index));
        let mut other_vert_index = self.vert_hash.first(hash);
        while self.vert_hash.is_valid(other_vert_index) {
            if vert_index == other_vert_index {
                break;
            }
            let other_vert_data = &self.vertices[(num_floats * other_vert_index) as usize
                ..(num_floats * (other_vert_index + 1)) as usize];
            if vert_data == other_vert_data {
                self.set_vert_index(corner, other_vert_index);
                break;
            }
            other_vert_index = self.vert_hash.next(other_vert_index);
        }
    }

    pub fn simplify(
        &mut self,
        target_vertices: u32,
        target_triangles: u32,
        target_error: f32,
        min_vertices: u32,
        min_triangles: u32,
        limit_error: f32,
    ) -> f32 {
        log_internal(&format!(
            "Starting simplify loop: target_triangles={}",
            target_triangles
        ));
        for i in 0..self.num_attributes {
            if self.attribute_weights[i as usize] == 0.0 {
                self.zero_weights = true;
                break;
            }
        }

        for tri_index in 0..self.triangle_count {
            self.fix_up_tri(tri_index);
        }
        for i in 0..self.index_count {
            self.calc_edge_quadric(i);
        }

        self.pair_heap
            .resize(self.pairs.len() as u32, self.pairs.len() as u32);
        for pair_index in 0..self.pairs.len() as u32 {
            let pair = self.pairs[pair_index as usize];
            let error = self.evaluate_merge(pair.position0, pair.position1, false);
            self.pair_heap.add(error, pair_index);
        }

        let mut max_error = 0.0f32;
        let mut collapse_count = 0;
        while !self.pair_heap.is_empty() {
            if self.pair_heap.get_key(self.pair_heap.top()) > limit_error {
                log_internal("Reached limit error, stopping.");
                break;
            }
            let pair_index = self.pair_heap.pop();
            let pair = self.pairs[pair_index as usize];
            let merge_error = self.evaluate_merge(pair.position0, pair.position1, true);
            max_error = max_error.max(merge_error);
            collapse_count += 1;

            if collapse_count % 1000 == 0 {
                log_internal(&format!(
                    "Progress: {} triangles remaining, max error: {}",
                    self.remaining_triangles, max_error
                ));
            }

            if self.remaining_vertices <= target_vertices
                && self.remaining_triangles <= target_triangles
                && max_error >= target_error
            {
                log_internal("Target reached.");
                break;
            }
            if self.remaining_vertices <= min_vertices
                || self.remaining_triangles <= min_triangles
                || max_error >= limit_error
            {
                log_internal("Limit reached.");
                break;
            }

            let reevaluate = std::mem::take(&mut self.reevaluate_pairs);
            for p_idx in reevaluate {
                let p = self.pairs[p_idx as usize];
                let err = self.evaluate_merge(p.position0, p.position1, false);
                self.pair_heap.add(err, p_idx);
            }
        }

        let mut tri_index = 0;
        while self.remaining_vertices > target_vertices
            || self.remaining_triangles > target_triangles
        {
            if self.remaining_vertices <= min_vertices
                || self.remaining_triangles <= min_triangles
                || max_error >= limit_error
            {
                break;
            }
            while tri_index < self.triangle_count && self.tri_removed[tri_index as usize] {
                tri_index += 1;
            }
            if tri_index >= self.triangle_count {
                break;
            }
            self.remove_tri(tri_index);
        }

        log_internal(&format!(
            "Finished simplify loop. Collapses: {}, Final triangles: {}",
            collapse_count, self.remaining_triangles
        ));
        max_error
    }

    pub fn compact(&mut self) {
        let mut output_vert_index = 0u32;
        let num_floats = 3 + self.num_attributes;
        let mut new_vert_indices = vec![0u32; self.vertex_count as usize];

        for vert_index in 0..self.vertex_count {
            if self.vert_ref_count[vert_index as usize] > 0 {
                if vert_index != output_vert_index {
                    let src_start = (num_floats * vert_index) as usize;
                    let dst_start = (num_floats * output_vert_index) as usize;
                    for i in 0..num_floats as usize {
                        self.vertices[dst_start + i] = self.vertices[src_start + i];
                    }
                }
                new_vert_indices[vert_index as usize] = output_vert_index;
                output_vert_index += 1;
            }
        }

        let mut output_tri_index = 0u32;
        for tri_index in 0..self.triangle_count {
            if !self.tri_removed[tri_index as usize] {
                for k in 0..3 {
                    let vert_index = self.indices[(tri_index * 3 + k) as usize];
                    self.indices[(output_tri_index * 3 + k) as usize] =
                        new_vert_indices[vert_index as usize];
                }
                self.material_ids[output_tri_index as usize] =
                    self.material_ids[tri_index as usize];
                output_tri_index += 1;
            }
        }
    }

    pub fn shrink_tri_group_with_most_surface_area_loss(&mut self, shrink_amount: f32) {
        let mut shrink_material_index = -1i32;
        let mut shrink_surface_area = 0.0f32;
        for (idx, delta) in self.per_material_deltas.iter().enumerate() {
            if delta.surface_area < shrink_surface_area {
                shrink_material_index = idx as i32;
                shrink_surface_area = delta.surface_area;
            }
        }

        if shrink_material_index == -1 {
            return;
        }

        let no_center_id = 0u32;
        let vertex_locked_mask = 0x80000000u32;
        let center_id_mask = 0x7fffffffu32;

        let mut island_centers: Vec<[f32; 4]> = Vec::new();
        let mut vert_to_center = vec![no_center_id; self.vertex_count as usize];
        let mut tri_visited = vec![false; self.triangle_count as usize];
        let mut pending_tris = Vec::new();

        for tri_index in 0..self.triangle_count {
            if tri_visited[tri_index as usize]
                || self.tri_removed[tri_index as usize]
                || (self.material_ids[tri_index as usize] & 0xffffff) != shrink_material_index
            {
                continue;
            }

            let center_id = (island_centers.len() + 1) as u32;
            island_centers.push([0.0, 0.0, 0.0, 0.0]);
            let cur_center_idx = island_centers.len() - 1;

            pending_tris.push(tri_index);
            tri_visited[tri_index as usize] = true;

            while let Some(cur_tri_index) = pending_tris.pop() {
                for corner_index in 0..3 {
                    let edge_index = cur_tri_index * 3 + corner_index;
                    let vert_index0 = self.indices[edge_index as usize];
                    let vert_index1 = self.indices[cycle3(edge_index) as usize];
                    let pos0 = self.get_position(vert_index0);
                    let pos1 = self.get_position(vert_index1);

                    if vert_to_center[vert_index0 as usize] == no_center_id {
                        vert_to_center[vert_index0 as usize] = center_id;
                        let c = &mut island_centers[cur_center_idx];
                        c[0] += pos0.x;
                        c[1] += pos0.y;
                        c[2] += pos0.z;
                        c[3] += 1.0;
                    } else if (vert_to_center[vert_index0 as usize] & center_id_mask) != center_id {
                        vert_to_center[vert_index0 as usize] = u32::MAX;
                    }

                    if (self.corner_flags[edge_index as usize] & Self::LOCKED_VERT_MASK) != 0 {
                        vert_to_center[vert_index0 as usize] |= vertex_locked_mask;
                    }

                    let hash = hash_position(pos1);
                    let mut corner = self.corner_hash.first(hash);
                    while self.corner_hash.is_valid(corner) {
                        let other_v0 = self.indices[corner as usize];
                        let other_v1 = self.indices[cycle3(corner) as usize];
                        if pos0 == self.get_position(other_v1)
                            && pos1 == self.get_position(other_v0)
                        {
                            let other_tri = corner / 3;
                            if !tri_visited[other_tri as usize]
                                && !self.tri_removed[other_tri as usize]
                                && (self.material_ids[other_tri as usize] & 0xffffff)
                                    == shrink_material_index
                            {
                                pending_tris.push(other_tri);
                                tri_visited[other_tri as usize] = true;
                            }
                        }
                        corner = self.corner_hash.next(corner);
                    }
                }
            }
        }

        for c in &mut island_centers {
            if c[3] > 0.0 {
                c[0] /= c[3];
                c[1] /= c[3];
                c[2] /= c[3];
            }
        }

        for vert_index in 0..self.vertex_count {
            let cid_raw = vert_to_center[vert_index as usize];
            if cid_raw != no_center_id && (cid_raw & vertex_locked_mask) == 0 && cid_raw != u32::MAX
            {
                let center = island_centers[(cid_raw - 1) as usize];
                let pos = self.get_position_mut(vert_index);
                pos.x += (center[0] - pos.x) * shrink_amount;
                pos.y += (center[1] - pos.y) * shrink_amount;
                pos.z += (center[2] - pos.z) * shrink_amount;
            }
        }
    }

    pub fn shrink_voxel_triangles(&mut self, shrink_amount: f32, voxel_triangles: &[bool]) {
        for tri_index in 0..self.triangle_count {
            if !voxel_triangles[tri_index as usize] || self.tri_removed[tri_index as usize] {
                continue;
            }
            let mut center = Vec3f::default();
            for k in 0..3 {
                center = center + self.get_position(self.indices[(tri_index * 3 + k) as usize]);
            }
            center = center * (1.0 / 3.0);
            for k in 0..3 {
                let vert_index = self.indices[(tri_index * 3 + k) as usize];
                let pos = self.get_position_mut(vert_index);
                pos.x += (center.x - pos.x) * shrink_amount;
                pos.y += (center.y - pos.y) * shrink_amount;
                pos.z += (center.z - pos.z) * shrink_amount;
            }
        }
    }

    pub fn preserve_surface_area(&mut self) {
        let mut dilate_material_index = -1i32;
        let mut dilate_surface_area = 0.0f32;
        for (idx, delta) in self.per_material_deltas.iter().enumerate() {
            if delta.surface_area < dilate_surface_area {
                dilate_material_index = idx as i32;
                dilate_surface_area = delta.surface_area;
            }
        }
        if dilate_material_index == -1 {
            return;
        }

        let mut edge_normals = vec![[0.0f32; 4]; self.vertex_count as usize];
        let mut perimeter = 0.0f32;
        let mut this_area = 0.0f32;
        let mut num_edges = 0u32;

        for tri_index in 0..self.triangle_count {
            if self.tri_removed[tri_index as usize] {
                continue;
            }
            let surface_area = self.tri_quadrics[tri_index as usize].base.a as f32;
            if (self.material_ids[tri_index as usize] & 0xffffff) != dilate_material_index {
                continue;
            }
            this_area += surface_area;

            for corner_index in 0..3 {
                let edge_index = tri_index * 3 + corner_index;
                if self.edge_quadrics_valid[edge_index as usize] {
                    let v0 = self.indices[edge_index as usize];
                    let v1 = self.indices[cycle3(edge_index) as usize];
                    let pos0 = self.get_position(v0);
                    let pos1 = self.get_position(v1);

                    let mut found_matching = false;
                    let mut corner = self.corner_hash.first(hash_position(pos1));
                    while self.corner_hash.is_valid(corner) {
                        if pos0 == self.get_position(self.indices[cycle3(corner) as usize])
                            && pos1 == self.get_position(self.indices[corner as usize])
                        {
                            found_matching = true;
                            break;
                        }
                        corner = self.corner_hash.next(corner);
                    }

                    if !found_matching {
                        num_edges += 1;
                        let p0 = self.get_position(self.indices[(tri_index * 3) as usize]);
                        let p1 = self.get_position(self.indices[(tri_index * 3 + 1) as usize]);
                        let p2 = self.get_position(self.indices[(tri_index * 3 + 2) as usize]);
                        let edge = pos1 - pos0;
                        let face_normal = (p2 - p0).cross(p1 - p0);
                        let mut edge_normal = face_normal.cross(edge);
                        let edge_len = edge.length();
                        if edge_len > 1e-6 {
                            edge_normal = edge_normal * (1.0 / edge_normal.length());
                            perimeter += edge_len;
                            edge_normal = edge_normal * edge_len;

                            let mut add_norm =
                                |simplifier: &MeshSimplifier, v_idx: u32, en: Vec3f, el: f32| {
                                    let hash = hash_position(simplifier.get_position(v_idx));
                                    let mut cur_v = simplifier.vert_hash.first(hash);
                                    while simplifier.vert_hash.is_valid(cur_v) {
                                        if simplifier.get_position(cur_v)
                                            == simplifier.get_position(v_idx)
                                        {
                                            let en_data = &mut unsafe {
                                                edge_normals.as_mut_ptr().add(cur_v as usize).read()
                                            };
                                            en_data[0] += en.x;
                                            en_data[1] += en.y;
                                            en_data[2] += en.z;
                                            en_data[3] += el;
                                            unsafe {
                                                edge_normals
                                                    .as_mut_ptr()
                                                    .add(cur_v as usize)
                                                    .write(*en_data)
                                            };
                                        }
                                        cur_v = simplifier.vert_hash.next(cur_v);
                                    }
                                };
                            if (self.corner_flags[edge_index as usize] & Self::LOCKED_VERT_MASK)
                                == 0
                            {
                                add_norm(self, v0, edge_normal, edge_len);
                            }
                            if (self.corner_flags[cycle3(edge_index) as usize]
                                & Self::LOCKED_VERT_MASK)
                                == 0
                            {
                                add_norm(self, v1, edge_normal, edge_len);
                            }
                        }
                    }
                }
            }
        }

        if -dilate_surface_area > 4.0 * this_area || perimeter < 1e-6 {
            return;
        }
        let dilate_dist = -dilate_surface_area / perimeter;

        let mut seed = num_edges;
        for vert_index in 0..self.vertex_count {
            let en = edge_normals[vert_index as usize];
            if en[3] > 1e-6 {
                seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
                let rand_val = (seed & 0x7fffffff) as f32 / 0x7fffffff as f32;
                let scale = rand_val * 0.5 + 0.75;
                let norm = Vec3f::new(en[0] / en[3], en[1] / en[3], en[2] / en[3]);
                let len_sq = norm.length_sq();
                if len_sq > 0.1 {
                    let move_vec = norm * (scale * dilate_dist / len_sq);
                    let pos = self.get_position_mut(vert_index);
                    *pos = *pos + move_vec;
                }
            }
        }
    }
}

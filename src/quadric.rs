use std::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Sub, SubAssign};
use crate::math::lup::*;
use crate::math::*;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3f {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3f {
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    pub fn cross(self, v: Self) -> Self {
        Self {
            x: self.y * v.z - self.z * v.y,
            y: self.z * v.x - self.x * v.z,
            z: self.x * v.y - self.y * v.x,
        }
    }

    pub fn length_sq(self) -> f32 {
        self.dot(self)
    }

    pub fn length(self) -> f32 {
        self.length_sq().sqrt()
    }
}

impl From<Vec3f> for QVec3 {
    fn from(v: Vec3f) -> Self {
        Self::new(v.x as f64, v.y as f64, v.z as f64)
    }
}

impl From<QVec3> for Vec3f {
    fn from(v: QVec3) -> Self {
        Self::new(v.x as f32, v.y as f32, v.z as f32)
    }
}

impl Add for Vec3f {
    type Output = Self;
    fn add(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }
}

impl Sub for Vec3f {
    type Output = Self;
    fn sub(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }
}

impl Mul<f32> for Vec3f {
    type Output = Self;
    fn mul(self, scalar: f32) -> Self {
        Self::new(self.x * scalar, self.y * scalar, self.z * scalar)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct QVec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl QVec3 {
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    pub const fn splat(v: f64) -> Self {
        Self { x: v, y: v, z: v }
    }

    pub fn dot(self, other: Self) -> f64 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    pub fn cross(self, v: Self) -> Self {
        Self {
            x: self.y * v.z - self.z * v.y,
            y: self.z * v.x - self.x * v.z,
            z: self.x * v.y - self.y * v.x,
        }
    }

    pub fn length_sq(self) -> f64 {
        self.dot(self)
    }

    pub fn length(self) -> f64 {
        self.length_sq().sqrt()
    }
}

impl Add for QVec3 {
    type Output = Self;
    fn add(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }
}

impl AddAssign for QVec3 {
    fn add_assign(&mut self, other: Self) {
        self.x += other.x;
        self.y += other.y;
        self.z += other.z;
    }
}

impl Sub for QVec3 {
    type Output = Self;
    fn sub(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }
}

impl SubAssign for QVec3 {
    fn sub_assign(&mut self, other: Self) {
        self.x -= other.x;
        self.y -= other.y;
        self.z -= other.z;
    }
}

impl Mul<f64> for QVec3 {
    type Output = Self;
    fn mul(self, scalar: f64) -> Self {
        Self::new(self.x * scalar, self.y * scalar, self.z * scalar)
    }
}

impl Mul<QVec3> for f64 {
    type Output = QVec3;
    fn mul(self, v: QVec3) -> QVec3 {
        v * self
    }
}

impl MulAssign<f64> for QVec3 {
    fn mul_assign(&mut self, scalar: f64) {
        self.x *= scalar;
        self.y *= scalar;
        self.z *= scalar;
    }
}

impl Div<f64> for QVec3 {
    type Output = Self;
    fn div(self, scalar: f64) -> Self {
        Self::new(self.x / scalar, self.y / scalar, self.z / scalar)
    }
}

impl DivAssign<f64> for QVec3 {
    fn div_assign(&mut self, scalar: f64) {
        self.x /= scalar;
        self.y /= scalar;
        self.z /= scalar;
    }
}

impl Neg for QVec3 {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y, -self.z)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct EdgeQuadric {
    pub nxx: f64,
    pub nyy: f64,
    pub nzz: f64,
    pub nxy: f64,
    pub nxz: f64,
    pub nyz: f64,
    pub n: QVec3,
    pub a: f64,
}

impl EdgeQuadric {
    pub fn zero(&mut self) {
        self.nxx = 0.0;
        self.nyy = 0.0;
        self.nzz = 0.0;
        self.nxy = 0.0;
        self.nxz = 0.0;
        self.nyz = 0.0;
        self.n = QVec3::splat(0.0);
        self.a = 0.0;
    }

    pub fn new_with_weight(p0: QVec3, p1: QVec3, weight: f32) -> Self {
        let mut q = Self::default();
        q.n = p1 - p0;
        let length = q.n.length();
        if length < SMALL_NUMBER {
            q.zero();
        } else {
            q.n /= length;
            q.a = weight as f64 * length;
            q.nxx = q.a - q.a * q.n.x * q.n.x;
            q.nyy = q.a - q.a * q.n.y * q.n.y;
            q.nzz = q.a - q.a * q.n.z * q.n.z;
            q.nxy = -q.a * q.n.x * q.n.y;
            q.nxz = -q.a * q.n.x * q.n.z;
            q.nyz = -q.a * q.n.y * q.n.z;
        }
        q
    }

    pub fn new_with_face_normal(p0: QVec3, p1: QVec3, face_normal: QVec3, weight: f32) -> Self {
        let mut q = Self::default();
        let p01 = p1 - p0;
        q.n = p01.cross(face_normal);
        let length = q.n.length();
        if length < SMALL_NUMBER {
            q.zero();
        } else {
            q.n /= length;
            q.a = weight as f64 * p01.length();
            q.nxx = q.a * q.n.x * q.n.x;
            q.nyy = q.a * q.n.y * q.n.y;
            q.nzz = q.a * q.n.z * q.n.z;
            q.nxy = q.a * q.n.x * q.n.y;
            q.nxz = q.a * q.n.x * q.n.z;
            q.nyz = q.a * q.n.y * q.n.z;
        }
        q
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Quadric {
    pub nxx: f64,
    pub nyy: f64,
    pub nzz: f64,
    pub nxy: f64,
    pub nxz: f64,
    pub nyz: f64,
    pub dn: QVec3,
    pub d2: f64,
    pub a: f64,
}

impl Quadric {
    pub fn zero(&mut self) {
        self.nxx = 0.0;
        self.nyy = 0.0;
        self.nzz = 0.0;
        self.nxy = 0.0;
        self.nxz = 0.0;
        self.nyz = 0.0;
        self.dn = QVec3::splat(0.0);
        self.d2 = 0.0;
        self.a = 0.0;
    }

    pub fn from_triangle(p0: QVec3, p1: QVec3, p2: QVec3) -> Self {
        let mut q = Self::default();
        let p01 = p1 - p0;
        let p02 = p2 - p0;
        let mut n = p02.cross(p01);
        let length = n.length();
        let area = 0.5 * length;
        if length < SMALL_NUMBER {
            q.zero();
            return q;
        }
        n /= length;
        q.nxx = n.x * n.x;
        q.nyy = n.y * n.y;
        q.nzz = n.z * n.z;
        q.nxy = n.x * n.y;
        q.nxz = n.x * n.z;
        q.nyz = n.y * n.z;
        let dist = -n.dot(p0);
        q.dn = n * dist;
        q.d2 = dist * dist;

        #[cfg(feature = "weight_by_area")]
        {
            q.nxx *= area;
            q.nyy *= area;
            q.nzz *= area;
            q.nxy *= area;
            q.nxz *= area;
            q.nyz *= area;
            q.dn *= area;
            q.d2 *= area;
            q.a = area;
        }
        #[cfg(not(feature = "weight_by_area"))]
        {
            q.a = 1.0;
        }
        q
    }

    pub fn from_point(p: QVec3) -> Self {
        let mut q = Self::default();
        q.nxx = 1.0;
        q.nyy = 1.0;
        q.nzz = 1.0;
        q.nxy = 0.0;
        q.nxz = 0.0;
        q.nyz = 0.0;
        q.dn = -p;
        q.d2 = p.length_sq();
        q.a = 0.0;
        q
    }

    pub fn from_line(n: QVec3, p: QVec3) -> Self {
        let mut q = Self::default();
        q.nxx = 1.0 - n.x * n.x;
        q.nyy = 1.0 - n.y * n.y;
        q.nzz = 1.0 - n.z * n.z;
        q.nxy = -n.x * n.y;
        q.nxz = -n.x * n.z;
        q.nyz = -n.y * n.z;
        let dist = -n.dot(p);
        q.dn = -p - n * dist;
        q.d2 = p.length_sq() - dist * dist;
        q.a = 0.0;
        q
    }

    pub fn add_assign(&mut self, q: &Quadric) {
        self.nxx += q.nxx;
        self.nyy += q.nyy;
        self.nzz += q.nzz;
        self.nxy += q.nxy;
        self.nxz += q.nxz;
        self.nyz += q.nyz;
        self.dn += q.dn;
        self.d2 += q.d2;
        self.a += q.a;
    }

    pub fn add_edge_quadric(&mut self, eq: &EdgeQuadric, point: QVec3) {
        let dist = -eq.n.dot(point);
        self.nxx += eq.nxx;
        self.nyy += eq.nyy;
        self.nzz += eq.nzz;
        self.nxy += eq.nxy;
        self.nxz += eq.nxz;
        self.nyz += eq.nyz;
        let a_dist = eq.a * dist;
        self.dn += eq.a * (-point) - eq.n * a_dist;
        self.d2 += eq.a * point.length_sq() - dist * a_dist;
    }

    pub fn evaluate(&self, p: QVec3) -> f64 {
        let x = p.dot(QVec3::new(self.nxx, self.nxy, self.nxz));
        let y = p.dot(QVec3::new(self.nxy, self.nyy, self.nyz));
        let z = p.dot(QVec3::new(self.nxz, self.nyz, self.nzz));
        let v_av = p.dot(QVec3::new(x, y, z));
        let btv = p.dot(self.dn);
        let mut q = v_av + 2.0 * btv + self.d2;
        if q < 0.0 || !q.is_finite() {
            q = 0.0;
        }
        q
    }
}

#[derive(Clone, Debug, Default)]
pub struct QuadricAttr {
    pub base: Quadric,
    pub nv: QVec3,
    pub dv: f64,
    pub g: Vec<QVec3>,
    pub d: Vec<f64>,
}

impl QuadricAttr {
    pub fn new(
        p0: QVec3, p1: QVec3, p2: QVec3,
        attr0: &[f32], attr1: &[f32], attr2: &[f32],
        attribute_weights: &[f32], num_attributes: usize
    ) -> Self {
        let mut qa = Self::default();
        let p01 = p1 - p0;
        let p02 = p2 - p0;
        let mut n = p02.cross(p01);

        qa.nv = n;
        qa.dv = -n.dot(p0);

        let length = n.length();
        let area = 0.5 * length;
        if area < 1e-12 {
            qa.zero(num_attributes);
            return qa;
        }
        n /= length;

        qa.base.nxx = n.x * n.x;
        qa.base.nyy = n.y * n.y;
        qa.base.nzz = n.z * n.z;
        qa.base.nxy = n.x * n.y;
        qa.base.nxz = n.x * n.z;
        qa.base.nyz = n.y * n.z;

        let dist = -n.dot(p0);
        qa.base.dn = n * dist;
        qa.base.d2 = dist * dist;

        let mut a_mat = [
            p01.x, p01.y, p01.z,
            p02.x, p02.y, p02.z,
            n.x, n.y, n.z
        ];
        let mut pivot = [0u32; 3];
        let is_invertible = lup_factorize(&mut a_mat, &mut pivot, 3, 1e-12);

        qa.g.resize(num_attributes, QVec3::default());
        qa.d.resize(num_attributes, 0.0);

        for i in 0..num_attributes {
            if attribute_weights[i] == 0.0 {
                continue;
            }

            let mut a0 = (attribute_weights[i] * attr0[i]) as f64;
            let mut a1 = (attribute_weights[i] * attr1[i]) as f64;
            let mut a2 = (attribute_weights[i] * attr2[i]) as f64;

            if !a0.is_finite() { a0 = 0.0; }
            if !a1.is_finite() { a1 = 0.0; }
            if !a2.is_finite() { a2 = 0.0; }

            let mut grad = QVec3::default();
            if is_invertible {
                let b = [a1 - a0, a2 - a0, 0.0];
                let mut grad_arr = [0.0; 3];
                lup_solve(&a_mat, &pivot, 3, &b, &mut grad_arr);
                grad = QVec3::new(grad_arr[0], grad_arr[1], grad_arr[2]);

                let residual = [
                    b[0] - grad.dot(p01),
                    b[1] - grad.dot(p02),
                    b[2] - grad.dot(n)
                ];
                let mut error_arr = [0.0; 3];
                lup_solve(&a_mat, &pivot, 3, &residual, &mut error_arr);
                grad += QVec3::new(error_arr[0], error_arr[1], error_arr[2]);
            }

            qa.g[i] = grad;
            qa.d[i] = a0 - grad.dot(p0);

            qa.base.nxx += grad.x * grad.x;
            qa.base.nyy += grad.y * grad.y;
            qa.base.nzz += grad.z * grad.z;
            qa.base.nxy += grad.x * grad.y;
            qa.base.nxz += grad.x * grad.z;
            qa.base.nyz += grad.y * grad.z;

            qa.base.dn += grad * qa.d[i];
            qa.base.d2 += qa.d[i] * qa.d[i];
        }

        #[cfg(feature = "weight_by_area")]
        {
            qa.base.nxx *= area;
            qa.base.nyy *= area;
            qa.base.nzz *= area;
            qa.base.nxy *= area;
            qa.base.nxz *= area;
            qa.base.nyz *= area;
            qa.base.dn *= area;
            qa.base.d2 *= area;

            for i in 0..num_attributes {
                qa.g[i] *= area;
                qa.d[i] *= area;
            }
            qa.base.a = area;
        }
        #[cfg(not(feature = "weight_by_area"))]
        {
            qa.base.a = 1.0;
        }
        qa
    }

    pub fn zero(&mut self, num_attributes: usize) {
        self.base.zero();
        self.nv = QVec3::splat(0.0);
        self.dv = 0.0;
        self.g.clear();
        self.g.resize(num_attributes, QVec3::default());
        self.d.clear();
        self.d.resize(num_attributes, 0.0);
    }

    pub fn rebase(&mut self, point: QVec3, attribute: &[f32], attribute_weights: &[f32], num_attributes: usize) {
        if self.base.a < 1e-12 {
            return;
        }

        let inv_a = 1.0 / self.base.a;
        let dist_2a = -self.nv.dot(point);
        let dist_half = 0.25 * dist_2a * inv_a;

        self.base.dn = self.nv * dist_half;
        self.base.d2 = dist_half * dist_2a;
        self.dv = dist_2a;

        for i in 0..num_attributes {
            if attribute_weights[i] == 0.0 {
                continue;
            }
            let a0 = (attribute_weights[i] * attribute[i]) as f64;
            let qd = a0 - self.g[i].dot(point) * inv_a;
            self.d[i] = qd * self.base.a;
            self.base.dn += self.g[i] * qd;
            self.base.d2 += qd * self.d[i];
        }
    }

    pub fn add(&mut self, q: &QuadricAttr, point: QVec3, attribute: &[f32], attribute_weights: &[f32], num_attributes: usize) {
        if q.base.a < 1e-12 {
            return;
        }

        self.base.nxx += q.base.nxx;
        self.base.nyy += q.base.nyy;
        self.base.nzz += q.base.nzz;
        self.base.nxy += q.base.nxy;
        self.base.nxz += q.base.nxz;
        self.base.nyz += q.base.nyz;

        let inv_a = 1.0 / q.base.a;
        let dist_2a = -q.nv.dot(point);
        let dist_half = 0.25 * dist_2a * inv_a;

        self.base.dn += q.nv * dist_half;
        self.base.d2 += dist_half * dist_2a;

        self.nv += q.nv;
        self.dv += dist_2a;

        for i in 0..num_attributes {
            if attribute_weights[i] == 0.0 {
                continue;
            }
            let a0 = (attribute_weights[i] * attribute[i]) as f64;
            let qd = a0 - q.g[i].dot(point) * inv_a;
            let qda = qd * q.base.a;

            self.g[i] += q.g[i];
            self.d[i] += qda;

            self.base.dn += q.g[i] * qd;
            self.base.d2 += qd * qda;
        }
        self.base.a += q.base.a;
    }

    pub fn evaluate(&self, point: QVec3, attributes: &[f32], attribute_weights: &[f32], num_attributes: usize) -> f64 {
        let x = point.dot(QVec3::new(self.base.nxx, self.base.nxy, self.base.nxz));
        let y = point.dot(QVec3::new(self.base.nxy, self.base.nyy, self.base.nyz));
        let z = point.dot(QVec3::new(self.base.nxz, self.base.nyz, self.base.nzz));

        let mut q = point.dot(QVec3::new(x, y, z)) + 2.0 * point.dot(self.base.dn) + self.base.d2;

        for i in 0..num_attributes {
            let pgd = point.dot(self.g[i]) + self.d[i];
            let s = (attribute_weights[i] * attributes[i]) as f64;
            q += s * (self.base.a * s - 2.0 * pgd);
        }

        if q < 0.0 || !q.is_finite() {
            q = 0.0;
        }
        q
    }

    pub fn calc_attributes_and_evaluate(&self, point: QVec3, attributes: &mut [f32], attribute_weights: &[f32], num_attributes: usize) -> f64 {
        let x = point.dot(QVec3::new(self.base.nxx, self.base.nxy, self.base.nxz));
        let y = point.dot(QVec3::new(self.base.nxy, self.base.nyy, self.base.nyz));
        let z = point.dot(QVec3::new(self.base.nxz, self.base.nyz, self.base.nzz));

        let mut q = point.dot(QVec3::new(x, y, z)) + 2.0 * point.dot(self.base.dn) + self.base.d2;

        for i in 0..num_attributes {
            if attribute_weights[i] != 0.0 {
                let pgd = point.dot(self.g[i]) + self.d[i];
                let s = pgd / self.base.a;
                attributes[i] = (s / attribute_weights[i] as f64) as f32;
                q -= pgd * s;
            }
        }

        if q < 0.0 || !q.is_finite() {
            q = 0.0;
        }
        q
    }
}

#[derive(Default)]
pub struct QuadricAttrOptimizer {
    pub nxx: f64,
    pub nyy: f64,
    pub nzz: f64,
    pub nxy: f64,
    pub nxz: f64,
    pub nyz: f64,
    pub dn: QVec3,
    pub a: f64,
    pub nv: QVec3,
    pub dv: f64,
    pub bbtxx: f64,
    pub bbtyy: f64,
    pub bbtzz: f64,
    pub bbtxy: f64,
    pub bbtxz: f64,
    pub bbtyz: f64,
    pub bd: QVec3,
}

impl QuadricAttrOptimizer {
    pub fn add_quadric(&mut self, q: &Quadric) {
        self.nxx += q.nxx;
        self.nyy += q.nyy;
        self.nzz += q.nzz;
        self.nxy += q.nxy;
        self.nxz += q.nxz;
        self.nyz += q.nyz;
        self.dn += q.dn;
    }

    pub fn add_quadric_attr(&mut self, q: &QuadricAttr, num_attributes: usize) {
        if q.base.a < SMALL_NUMBER {
            return;
        }

        self.nxx += q.base.nxx;
        self.nyy += q.base.nyy;
        self.nzz += q.base.nzz;
        self.nxy += q.base.nxy;
        self.nxz += q.base.nxz;
        self.nyz += q.base.nyz;

        self.dn += q.base.dn;
        self.a += q.base.a;

        self.nv += q.nv;
        self.dv += q.dv;

        for i in 0..num_attributes {
            self.bbtxx += q.g[i].x * q.g[i].x;
            self.bbtyy += q.g[i].y * q.g[i].y;
            self.bbtzz += q.g[i].z * q.g[i].z;
            self.bbtxy += q.g[i].x * q.g[i].y;
            self.bbtxz += q.g[i].x * q.g[i].z;
            self.bbtyz += q.g[i].y * q.g[i].z;
            self.bd += q.g[i] * q.d[i];
        }
    }

    pub fn optimize(&self, position: &mut QVec3) -> bool {
        if self.a < 1e-12 {
            return false;
        }
        let inv_a = 1.0 / self.a;
        let m_xx = self.nxx - self.bbtxx * inv_a;
        let m_yy = self.nyy - self.bbtyy * inv_a;
        let m_zz = self.nzz - self.bbtzz * inv_a;
        let m_xy = self.nxy - self.bbtxy * inv_a;
        let m_xz = self.nxz - self.bbtxz * inv_a;
        let m_yz = self.nyz - self.bbtyz * inv_a;

        let a_bd_dn = self.bd * inv_a - self.dn;

        let m = [
            m_xx, m_xy, m_xz,
            m_xy, m_yy, m_yz,
            m_xz, m_yz, m_zz
        ];
        let b = [a_bd_dn.x, a_bd_dn.y, a_bd_dn.z];
        let mut pivot = [0u32; 3];
        let mut lu = m;
        if lup_factorize(&mut lu, &mut pivot, 3, 1e-12) {
            let mut p = [0.0; 3];
            if lup_solve_iterate(&m, &lu, &pivot, 3, &b, &mut p) {
                position.x = p[0];
                position.y = p[1];
                position.z = p[2];
                return true;
            }
        }
        false
    }

    pub fn optimize_volume(&self, position: &mut QVec3) -> bool {
        if self.a < 1e-12 {
            return false;
        }
        let inv_a = 1.0 / self.a;
        let m_xx = self.nxx - self.bbtxx * inv_a;
        let m_yy = self.nyy - self.bbtyy * inv_a;
        let m_zz = self.nzz - self.bbtzz * inv_a;
        let m_xy = self.nxy - self.bbtxy * inv_a;
        let m_xz = self.nxz - self.bbtxz * inv_a;
        let m_yz = self.nyz - self.bbtyz * inv_a;

        let a_bd_dn = self.bd * inv_a - self.dn;

        if self.nv.length_sq() > 1e-12 {
            let m = [
                m_xx, m_xy, m_xz, self.nv.x,
                m_xy, m_yy, m_yz, self.nv.y,
                m_xz, m_yz, m_zz, self.nv.z,
                self.nv.x, self.nv.y, self.nv.z, 0.0
            ];
            let b = [a_bd_dn.x, a_bd_dn.y, a_bd_dn.z, -self.dv];
            let mut pivot = [0u32; 4];
            let mut lu = m;
            if lup_factorize(&mut lu, &mut pivot, 4, 1e-12) {
                let mut p = [0.0; 4];
                if lup_solve_iterate(&m, &lu, &pivot, 4, &b, &mut p) {
                    position.x = p[0];
                    position.y = p[1];
                    position.z = p[2];
                    return true;
                }
            }
        }
        false
    }

    pub fn optimize_linear(&self, position0: QVec3, position1: QVec3, position: &mut QVec3) -> bool {
        if self.a < 1e-12 {
            return false;
        }
        let inv_a = 1.0 / self.a;
        let m_xx = self.nxx - self.bbtxx * inv_a;
        let m_yy = self.nyy - self.bbtyy * inv_a;
        let m_zz = self.nzz - self.bbtzz * inv_a;
        let m_xy = self.nxy - self.bbtxy * inv_a;
        let m_xz = self.nxz - self.bbtxz * inv_a;
        let m_yz = self.nyz - self.bbtyz * inv_a;

        let a_bd_dn = self.bd * inv_a - self.dn;

        let m0 = QVec3::new(
            position0.x * m_xx + position0.y * m_xy + position0.z * m_xz,
            position0.x * m_xy + position0.y * m_yy + position0.z * m_yz,
            position0.x * m_xz + position0.y * m_yz + position0.z * m_zz,
        );
        let m1 = QVec3::new(
            position1.x * m_xx + position1.y * m_xy + position1.z * m_xz,
            position1.x * m_xy + position1.y * m_yy + position1.z * m_yz,
            position1.x * m_xz + position1.y * m_yz + position1.z * m_zz,
        );
        let m01 = m1 - m0;
        let m01_sqr = m01.length_sq();
        if m01_sqr < 1e-16 {
            return false;
        }

        let bm0 = a_bd_dn - m0;
        let mut t = m01.dot(bm0) / m01_sqr;

        let nv_sqr = self.nv.length_sq();
        if nv_sqr > 1e-12 {
            let nv0 = self.nv.dot(position0);
            let nv01 = self.nv.dot(position1) - nv0;
            let ata_xx = m01_sqr + nv01 * nv01;
            let ata_xy = m01.dot(self.nv);
            let ata_yy = nv_sqr;
            let det = ata_xx * ata_yy - ata_xy * ata_xy;
            if det.abs() > 1e-16 {
                let iata_xx = ata_yy;
                let iata_xy = -ata_xy;
                let atb0 = m01.dot(bm0) - (self.dv + nv0) * nv01;
                let atb1 = self.nv.dot(bm0);
                t = (iata_xx * atb0 + iata_xy * atb1) / det;
            }
        }

        t = t.clamp(0.0, 1.0);
        *position = position0 * (1.0 - t) + position1 * t;
        true
    }
}

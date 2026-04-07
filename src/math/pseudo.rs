pub fn pseudo_inverse(s: &mut [f64], size: usize, tolerance: f64) {
    let mut max_s: f64 = 0.0;
    for i in 0..size {
        max_s = max_s.max(s[i].abs());
    }

    for i in 0..size {
        if s[i].abs() > max_s * tolerance {
            s[i] = 1.0 / s[i];
        } else {
            s[i] = 0.0;
        }
    }
}

pub fn pseudo_solve(v: &[f64], s: &[f64], size: usize, b: &[f64], x: &mut [f64]) {
    x.fill(0.0);
    for i in 0..size {
        let mut svtbi = 0.0;
        for j in 0..size {
            svtbi += v[size * j + i] * b[j];
        }
        svtbi *= s[i];
        for j in 0..size {
            x[j] += v[size * j + i] * svtbi;
        }
    }
}
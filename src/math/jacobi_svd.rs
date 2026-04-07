
fn update(a: &mut [f64], s: f64, tau: f64, d1: usize, d2: usize) {
    let nu1 = a[d1];
    let nu2 = a[d2];
    a[d1] -= s * (nu2 + tau * nu1);
    a[d2] += s * (nu1 - tau * nu2);
}

fn rotation3(a: &mut [f64], v: &mut [f64], z: &mut [f64], tol: f64, j: usize, k: usize, l: usize) -> bool {
    let x = a[3 * j + j];
    let y = a[3 * j + k];
    let z_val = a[3 * k + k];

    let mu1 = z_val - x;
    let mu2 = 2.0 * y;

    if mu2.abs() <= tol * mu1.abs() {
        a[3 * j + k] = 0.0;
        return false;
    }

    let rho = mu1 / mu2;
    let t = (if rho < 0.0 { -1.0 } else { 1.0 }) / (rho.abs() + (1.0 + rho * rho).sqrt());
    let c = 1.0 / (1.0 + t * t).sqrt();
    let s = c * t;
    let tau = s / (1.0 + c);
    let h = t * y;

    z[j] -= h;
    z[k] += h;
    a[3 * j + j] -= h;
    a[3 * k + k] += h;
    a[3 * j + k] = 0.0;

    let idx1 = if l < j { 3 * l + j } else { 3 * j + l };
    let idx2 = if l < k { 3 * l + k } else { 3 * k + l };
    update(a, s, tau, idx1, idx2);

    for i in 0..3 {
        update(v, s, tau, 3 * i + j, 3 * i + k);
    }

    true
}

fn rotation4(a: &mut [f64], v: &mut [f64], z: &mut [f64], tol: f64, j: usize, k: usize, l1: usize, l2: usize) -> bool {
    let x = a[4 * j + j];
    let y = a[4 * j + k];
    let z_val = a[4 * k + k];

    let mu1 = z_val - x;
    let mu2 = 2.0 * y;

    if mu2.abs() <= tol * mu1.abs() {
        a[4 * j + k] = 0.0;
        return false;
    }

    let rho = mu1 / mu2;
    let t = (if rho < 0.0 { -1.0 } else { 1.0 }) / (rho.abs() + (1.0 + rho * rho).sqrt());
    let c = 1.0 / (1.0 + t * t).sqrt();
    let s = c * t;
    let tau = s / (1.0 + c);
    let h = t * y;

    z[j] -= h;
    z[k] += h;
    a[4 * j + j] -= h;
    a[4 * k + k] += h;
    a[4 * j + k] = 0.0;

    let idx1 = if l1 < j { 4 * l1 + j } else { 4 * j + l1 };
    let idx2 = if l1 < k { 4 * l1 + k } else { 4 * k + l1 };
    update(a, s, tau, idx1, idx2);

    let idx3 = if l2 < j { 4 * l2 + j } else { 4 * j + l2 };
    let idx4 = if l2 < k { 4 * l2 + k } else { 4 * k + l2 };
    update(a, s, tau, idx3, idx4);

    for i in 0..4 {
        update(v, s, tau, 4 * i + j, 4 * i + k);
    }

    true
}

fn max_off_diag_symm(a: &[f64], size: usize) -> f64 {
    let mut result: f64 = 0.0;
    for i in 0..size {
        for j in i + 1..size {
            result = result.max(a[size * i + j].abs());
        }
    }
    result
}

pub fn eigen_solver3(a: &mut [f64], s: &mut [f64], v: &mut [f64], tol: f64) {
    v.fill(0.0);
    for i in 0..3 {
        s[i] = a[3 * i + i];
        v[3 * i + i] = 1.0;
    }

    let max_iter = 20;
    let abs_tol = tol * max_off_diag_symm(a, 3);
    if abs_tol != 0.0 {
        let mut num_iter = 0;
        loop {
            num_iter += 1;
            let mut z = [0.0; 3];
            let mut changed;
            changed = rotation3(a, v, &mut z, tol, 0, 1, 2);
            changed = rotation3(a, v, &mut z, tol, 0, 2, 1) || changed;
            changed = rotation3(a, v, &mut z, tol, 1, 2, 0) || changed;

            for i in 0..3 {
                s[i] += z[i];
                a[3 * i + i] = s[i];
            }

            if !changed || max_off_diag_symm(a, 3) <= abs_tol || num_iter >= max_iter {
                break;
            }
        }
    }
}

pub fn eigen_solver4(a: &mut [f64], s: &mut [f64], v: &mut [f64], tol: f64) {
    v.fill(0.0);
    for i in 0..4 {
        s[i] = a[4 * i + i];
        v[4 * i + i] = 1.0;
    }

    let max_iter = 20;
    let abs_tol = tol * max_off_diag_symm(a, 4);
    if abs_tol != 0.0 {
        let mut num_iter = 0;
        loop {
            num_iter += 1;
            let mut z = [0.0; 4];
            let mut changed;
            changed = rotation4(a, v, &mut z, tol, 0, 1, 2, 3);
            changed = rotation4(a, v, &mut z, tol, 0, 2, 1, 3) || changed;
            changed = rotation4(a, v, &mut z, tol, 0, 3, 1, 2) || changed;
            changed = rotation4(a, v, &mut z, tol, 1, 2, 0, 3) || changed;
            changed = rotation4(a, v, &mut z, tol, 1, 3, 0, 2) || changed;
            changed = rotation4(a, v, &mut z, tol, 2, 3, 0, 1) || changed;

            for i in 0..4 {
                s[i] += z[i];
                a[4 * i + i] = s[i];
            }

            if !changed || max_off_diag_symm(a, 4) <= abs_tol || num_iter >= max_iter {
                break;
            }
        }
    }
}
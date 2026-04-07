use super::KINDA_SMALL_NUMBER;

pub fn lup_factorize(a: &mut [f64], pivot: &mut [u32], size: usize, epsilon: f64) -> bool {
    for i in 0..size {
        pivot[i] = i as u32;
    }

    for i in 0..size {
        let mut max_value = a[size * i + i].abs();
        let mut max_index = i;

        for j in i + 1..size {
            let abs_value = a[size * j + i].abs();
            if abs_value > max_value {
                max_value = abs_value;
                max_index = j;
            }
        }

        if max_value < epsilon {
            return false;
        }

        if max_index != i {
            pivot.swap(i, max_index);
            for j in 0..size {
                a.swap(size * i + j, size * max_index + j);
            }
        }

        for j in i + 1..size {
            a[size * j + i] /= a[size * i + i];
            for k in i + 1..size {
                let factor = a[size * j + i];
                a[size * j + k] -= factor * a[size * i + k];
            }
        }
    }

    true
}

pub fn lup_solve(lu: &[f64], pivot: &[u32], size: usize, b: &[f64], x: &mut [f64]) {
    for i in 0..size {
        x[i] = b[pivot[i] as usize];
        for j in 0..i {
            x[i] -= lu[size * i + j] * x[j];
        }
    }

    for i in (0..size).rev() {
        for j in i + 1..size {
            x[i] -= lu[size * i + j] * x[j];
        }
        x[i] /= lu[size * i + i];
    }
}

pub fn lup_solve_iterate(a: &[f64], lu: &[f64], pivot: &[u32], size: usize, b: &[f64], x: &mut [f64]) -> bool {
    let mut residual = vec![0.0; size];
    let mut error = vec![0.0; size];

    lup_solve(lu, pivot, size, b, x);

    let mut close_enough = false;
    for _ in 0..4 {
        for i in 0..size {
            residual[i] = b[i];
            for j in 0..size {
                residual[i] -= a[size * i + j] * x[j];
            }
        }

        lup_solve(lu, pivot, size, &residual, &mut error);

        let mut mse = 0.0;
        for i in 0..size {
            x[i] += error[i];
            mse += error[i] * error[i];
        }

        if mse < KINDA_SMALL_NUMBER {
            close_enough = true;
            break;
        }
    }

    close_enough
}
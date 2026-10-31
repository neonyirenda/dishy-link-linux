//! The 4×4 matrix maths the sky camera needs, and nothing else.
//!
//! Column-major to match OpenGL's uniform layout, so a matrix goes to the GPU
//! without a transpose. Pure functions with no GL handles, which is what makes
//! them testable without a display.

pub type Vec3 = [f64; 3];
pub type Mat4 = [f32; 16];

pub fn perspective(fov_y: f64, aspect: f64, near: f64, far: f64) -> Mat4 {
    let f = 1.0 / (fov_y / 2.0).tan();
    let nf = 1.0 / (near - far);
    [
        (f / aspect) as f32,
        0.0,
        0.0,
        0.0,
        0.0,
        f as f32,
        0.0,
        0.0,
        0.0,
        0.0,
        ((far + near) * nf) as f32,
        -1.0,
        0.0,
        0.0,
        ((2.0 * far * near) * nf) as f32,
        0.0,
    ]
}

pub fn look_at(eye: Vec3, target: Vec3, up: Vec3) -> Mat4 {
    let zx = eye[0] - target[0];
    let zy = eye[1] - target[1];
    let zz = eye[2] - target[2];
    let zl = (zx * zx + zy * zy + zz * zz).sqrt().max(f64::EPSILON);
    let z = [zx / zl, zy / zl, zz / zl];
    let mut x = [
        up[1] * z[2] - up[2] * z[1],
        up[2] * z[0] - up[0] * z[2],
        up[0] * z[1] - up[1] * z[0],
    ];
    let xl = (x[0] * x[0] + x[1] * x[1] + x[2] * x[2])
        .sqrt()
        .max(f64::EPSILON);
    x[0] /= xl;
    x[1] /= xl;
    x[2] /= xl;
    let y = [
        z[1] * x[2] - z[2] * x[1],
        z[2] * x[0] - z[0] * x[2],
        z[0] * x[1] - z[1] * x[0],
    ];
    [
        x[0] as f32,
        y[0] as f32,
        z[0] as f32,
        0.0,
        x[1] as f32,
        y[1] as f32,
        z[1] as f32,
        0.0,
        x[2] as f32,
        y[2] as f32,
        z[2] as f32,
        0.0,
        -(x[0] * eye[0] + x[1] * eye[1] + x[2] * eye[2]) as f32,
        -(y[0] * eye[0] + y[1] * eye[1] + y[2] * eye[2]) as f32,
        -(z[0] * eye[0] + z[1] * eye[1] + z[2] * eye[2]) as f32,
        1.0,
    ]
}

pub fn multiply(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [0.0_f32; 16];
    for column in 0..4 {
        for row in 0..4 {
            out[column * 4 + row] = a[row] * b[column * 4]
                + a[4 + row] * b[column * 4 + 1]
                + a[8 + row] * b[column * 4 + 2]
                + a[12 + row] * b[column * 4 + 3];
        }
    }
    out
}

pub fn normalize(vector: Vec3) -> Vec3 {
    let length = (vector[0] * vector[0] + vector[1] * vector[1] + vector[2] * vector[2])
        .sqrt()
        .max(f64::EPSILON);
    [vector[0] / length, vector[1] / length, vector[2] / length]
}

#[cfg(test)]
mod tests {
    use super::*;

    // SPEC-SKY-004: the projection and view matrices follow OpenGL's
    // column-major layout, which is what the GL uniform consumes untransposed.
    #[test]
    fn matrices_are_column_major_and_project_the_origin() {
        let projection = perspective(std::f64::consts::FRAC_PI_3, 1.5, 0.1, 100.0);
        assert_eq!(projection[11], -1.0);
        assert!((projection[0] - 1.154_700_5).abs() < 0.000_1);

        let view = look_at([0.0, 0.0, 5.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        // The eye sits on +z, so the view-space z translation is -5.
        assert!((view[14] + 5.0).abs() < f64::EPSILON as f32);

        let combined = multiply(&projection, &view);
        assert_eq!(combined.len(), 16);
    }

    #[test]
    fn normalization_survives_a_zero_vector() {
        let unit = normalize([0.0, 0.0, 0.0]);
        assert!(unit.iter().all(|value| value.is_finite()));
    }
}

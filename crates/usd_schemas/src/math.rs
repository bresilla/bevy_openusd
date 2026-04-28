//! Small math helpers used by the schema authoring layer.
//!
//! Kept in-crate so `usd_schemas` is self-contained — callers don't
//! need to bring in `nalgebra` / `glam` / their own quaternion lib just
//! to populate `Xformable` ops.

/// Convert intrinsic X-Y-Z Tait-Bryan angles (URDF's `<origin rpy="r p y">`
/// convention, `q = Rz(y) · Ry(p) · Rx(r)`) to a unit quaternion
/// `(w, x, y, z)`. USD's `Quatf` uses the same `(real, imag)` ordering.
pub fn rpy_to_quat(roll: f64, pitch: f64, yaw: f64) -> [f64; 4] {
    let (sr, cr) = (roll * 0.5).sin_cos();
    let (sp, cp) = (pitch * 0.5).sin_cos();
    let (sy, cy) = (yaw * 0.5).sin_cos();

    let w = cr * cp * cy + sr * sp * sy;
    let x = sr * cp * cy - cr * sp * sy;
    let y = cr * sp * cy + sr * cp * sy;
    let z = cr * cp * sy - sr * sp * cy;

    [w, x, y, z]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity() {
        let q = rpy_to_quat(0.0, 0.0, 0.0);
        assert!((q[0] - 1.0).abs() < 1e-12);
        assert!(q[1].abs() + q[2].abs() + q[3].abs() < 1e-12);
    }

    #[test]
    fn unit_norm() {
        let q = rpy_to_quat(0.3, -0.7, 1.1);
        let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
        assert!((n - 1.0).abs() < 1e-12);
    }
}

//! Vm60 100683f0 builds Rx(pitch) * Ry(roll) * Rz(bank) in signed Q12.
//! Each product term truncates before addition, as in the original x87 code.
pub(super) fn matrix(angles: [i32; 3]) -> [[i16; 3]; 3] {
    let [roll, pitch, bank] = angles.map(|a| f64::from(a) * 9.587379924285279e-05);
    let (sr, cr) = roll.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let (sb, cb) = bank.sin_cos();
    let q = |v: f64| (v * 4096.0).trunc() as i16;
    [
        [q(cr * cb), q(-cr * sb), q(sr)],
        [
            q(cb * sr * sp).wrapping_add(q(sb * cp)),
            q(cb * cp).wrapping_sub(q(sb * sr * sp)),
            q(-cr * sp),
        ],
        [
            q(sb * sp).wrapping_sub(q(cb * sr * cp)),
            q(cb * sp).wrapping_add(q(sb * sr * cp)),
            q(cr * cp),
        ],
    ]
}
pub(super) fn apply_q12(matrix: [[i16; 3]; 3], point: [i32; 3]) -> [i64; 3] {
    matrix.map(|row| {
        row.into_iter()
            .zip(point)
            .map(|(a, b)| i64::from(a) * i64::from(b))
            .sum()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matrix_matches_original_dll_for_each_axis_and_mixed_rotations() {
        // Numeric outputs from the installed DLL, with no game image data.
        for (angles, expected) in [
            ([0, 0, 0], [[4096, 0, 0], [0, 4096, 0], [0, 0, 4096]]),
            (
                [4096, 0, 0],
                [[3784, 0, 1567], [0, 4096, 0], [-1567, 0, 3784]],
            ),
            (
                [0, 4096, 0],
                [[4096, 0, 0], [0, 3784, -1567], [0, 1567, 3784]],
            ),
            (
                [0, 0, 4096],
                [[3784, -1567, 0], [1567, 3784, 0], [0, 0, 4096]],
            ),
            (
                [1000, 2000, 3000],
                [[3909, -1156, 392], [1211, 3834, -777], [-148, 857, 4002]],
            ),
            (
                [-2288, -336, 0],
                [[3997, 0, -891], [28, 4093, 128], [890, -131, 3995]],
            ),
            (
                [-3000, 1200, -800],
                [[3916, 300, -1161], [-443, 4046, -450], [1114, 556, 3901]],
            ),
        ] {
            assert_eq!(matrix(angles), expected, "{angles:?}");
        }
    }
}

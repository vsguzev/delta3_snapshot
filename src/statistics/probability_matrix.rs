use nalgebra::DMatrix;

pub struct ProbabilityMatrix;

impl ProbabilityMatrix {
    fn normal_pdf(x: f32, mean: f32, std_dev: f32) -> f32 {
        if std_dev < 1e-10 {
            return if (x - mean).abs() < 1e-10 { 1.0 } else { 0.0 };
        }
        let z_score = (x - mean) / std_dev;
        (-z_score * z_score / 2.0).exp() / (std_dev * (2.0 * std::f32::consts::PI).sqrt())
    }

    pub fn generate(stats: &DMatrix<f64>, deltas: usize) -> DMatrix<f32> {
        let available_periods = stats.nrows();
        let base_step = 0.0001;
        let max_delta = base_step * (deltas as f32);

        let mut all_probs = vec![vec![0.0; deltas]; available_periods];
        let mut sums = vec![0.0; available_periods];

        for row in 0..available_periods {
            let mean = stats[(row, 1)] as f32;
            let std_dev = stats[(row, 2)] as f32;

            if std_dev < 1e-10 {
                all_probs[row][0] = 1.0;
                sums[row] = 1.0;
            } else {
                for col in 0..deltas {
                    let delta = col as f32 * base_step;
                    all_probs[row][col] = Self::normal_pdf(delta, mean, std_dev);
                    sums[row] += all_probs[row][col];
                }
            }
        }

        DMatrix::from_fn(available_periods, deltas, |row, col| {
            if sums[row] > 0.0 {
                all_probs[row][col] / sums[row]
            } else if col == 0 {
                1.0
            } else {
                0.0
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    #[test]
    fn test_probability_matrix_properties() {
        let stats_data = vec![0.0, 0.001, 0.002, 1.0, 0.002, 0.003];
        let stats = DMatrix::from_row_slice(2, 3, &stats_data);
        let deltas = 5;

        let prob_matrix = ProbabilityMatrix::generate(&stats, deltas);

        for row in 0..prob_matrix.nrows() {
            for col in 0..prob_matrix.ncols() {
                let prob = prob_matrix[(row, col)];
                assert!(
                    prob >= 0.0 && prob <= 1.0,
                    "Probability at [{}, {}] = {} is not between 0 and 1",
                    row,
                    col,
                    prob
                );
            }
        }

        for row in 0..prob_matrix.nrows() {
            let row_sum: f32 = (0..prob_matrix.ncols())
                .map(|col| prob_matrix[(row, col)])
                .sum();
            assert!(
                (row_sum - 1.0).abs() < 1e-6,
                "Row {} sum = {} should be approximately 1.0",
                row,
                row_sum
            );
        }

        for row in 0..prob_matrix.nrows() {
            let mean = stats[(row, 1)] as f32;
            let std_dev = stats[(row, 2)] as f32;

            if std_dev < 1e-10 {
                continue;
            }

            let peak_idx = prob_matrix
                .row(row)
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
                .unwrap()
                .0;

            let delta_range = 0.05;
            let dx = 2.0 * delta_range / (deltas as f32 - 1.0);
            let peak_x = -delta_range + peak_idx as f32 * dx;

            assert!(
                (peak_x - mean).abs() <= std_dev,
                "Peak probability at x = {} is too far from mean = {}",
                peak_x,
                mean
            );
        }
    }

    #[test]
    fn test_zero_std_dev_case() {
        let stats_data = vec![0.0, 0.001, 0.0];
        let stats = DMatrix::from_row_slice(1, 3, &stats_data);
        let deltas = 5;

        let prob_matrix = ProbabilityMatrix::generate(&stats, deltas);

        assert_relative_eq!(prob_matrix[(0, 0)], 1.0);
        for col in 1..deltas {
            assert_relative_eq!(prob_matrix[(0, col)], 0.0);
        }

        let row_sum: f32 = (0..prob_matrix.ncols())
            .map(|col| prob_matrix[(0, col)])
            .sum();
        assert_relative_eq!(row_sum, 1.0, epsilon = 1e-6);
    }
}

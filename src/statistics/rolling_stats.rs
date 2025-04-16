use nalgebra::DMatrix;
use crate::models::Candle;

pub struct RollingStats;

impl RollingStats {
    pub fn compute(candles: &[Candle]) -> DMatrix<f64> {
        let rows = candles.len();
        let mut data: Vec<f64> = Vec::with_capacity(rows * 6); // Increased columns for new metrics

        let lambda = 0.94; // EWMA decay factor
        let mut ewma_variance = 0.0;
        let mut last_return = 0.0;

        for i in 0..rows {
            let period_prices: Vec<f64> = candles[i..].iter().map(|c| c.close).collect();
            
            let price_changes: Vec<f64> = period_prices.windows(2)
                .map(|w| (w[1] - w[0]) / w[0])  // Relative price change
                .collect();
            
            let mean = if price_changes.is_empty() {
                0.0
            } else {
                price_changes.iter().sum::<f64>() / price_changes.len() as f64
            };
            
            let std_dev = if price_changes.len() <= 1 {
                0.0
            } else {
                (price_changes.iter()
                    .map(|&x| (x - mean).powi(2))
                    .sum::<f64>()
                    / (price_changes.len() - 1) as f64)
                    .sqrt()
            };

            // EWMA volatility
            if !price_changes.is_empty() {
                let current_return = price_changes[0];
                ewma_variance = lambda * ewma_variance + (1.0 - lambda) * (current_return - last_return).powi(2);
                last_return = current_return;
            }
            let ewma_vol = ewma_variance.sqrt();

            // Skewness
            let skewness = if price_changes.len() > 2 && std_dev > 0.0 {
                let n = price_changes.len() as f64;
                let sum_cubed_deviations = price_changes.iter()
                    .map(|&x| (x - mean).powi(3))
                    .sum::<f64>();
                (sum_cubed_deviations / n) / std_dev.powi(3)
            } else {
                0.0
            };

            // Kurtosis
            let kurtosis = if price_changes.len() > 2 && std_dev > 0.0 {
                let n = price_changes.len() as f64;
                let sum_fourth_deviations = price_changes.iter()
                    .map(|&x| (x - mean).powi(4))
                    .sum::<f64>();
                (sum_fourth_deviations / n) / std_dev.powi(4) - 3.0 // Excess kurtosis
            } else {
                0.0
            };

            data.push(candles[i].timestamp as f64);
            data.push(mean);
            data.push(std_dev);
            data.push(ewma_vol);
            data.push(skewness);
            data.push(kurtosis);
        }

        DMatrix::from_row_slice(rows, 6, &data)
    }
} 
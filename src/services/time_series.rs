use std::collections::VecDeque;
use log::info;
use nalgebra::DMatrix;
use crate::models::{Tick, Candle};
use std::collections::HashMap;

pub struct TimeSeries {
    ticks: VecDeque<Tick>,
    candles: Vec<Candle>,
}

impl TimeSeries {
    pub fn new() -> Self {
        Self {
            ticks: VecDeque::new(),
            candles: Vec::new(),
        }
    }

    pub fn add_tick(&mut self, timestamp: u64, price: f64) {
        self.ticks.push_back(Tick { timestamp, price });
    }

    pub fn remove_old_ticks(&mut self, min_timestamp: u64) -> usize {
        // Remove old ticks and return the number of removed ticks
        let mut removed = 0;
        while let Some(tick) = self.ticks.front() {
            if tick.timestamp < min_timestamp {
                self.ticks.pop_front();
                removed += 1;
            } else {
                break;
            }
        }
        // Remove old candles
        self.candles.retain(|candle| candle.timestamp >= min_timestamp);
        removed
    }

    pub fn generate_candles(&mut self) {
        if self.ticks.is_empty() {
            return;
        }
        
        let mut working_ticks = self.ticks.clone();
        
        let mut current_second = working_ticks.front().unwrap().timestamp;
        let mut second_ticks: Vec<f64> = Vec::new();

        // Group ticks by timestamp
        let mut grouped_ticks: HashMap<u64, Vec<f64>> = HashMap::new();
        
        while let Some(tick) = working_ticks.pop_front() {
            grouped_ticks
                .entry(tick.timestamp)
                .or_insert_with(Vec::new)
                .push(tick.price);
        }
        
        self.candles.clear();
        
        let mut timestamps: Vec<u64> = grouped_ticks.keys().cloned().collect();
        timestamps.sort();
        
        for timestamp in timestamps {
            let prices = &grouped_ticks[&timestamp];
            if !prices.is_empty() {
                let candle = Candle {
                    timestamp,
                    open: prices[0],
                    high: prices.iter().fold(f64::NEG_INFINITY, |a, &b| a.max(b)),
                    low: prices.iter().fold(f64::INFINITY, |a, &b| a.min(b)),
                    close: *prices.last().unwrap(),
                };
                self.candles.push(candle);
            }
        }
    }

    pub fn get_candles(&self) -> &[Candle] {
        &self.candles
    }
} 
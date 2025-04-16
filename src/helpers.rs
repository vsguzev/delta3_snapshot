use log::info;

pub fn round_to_step(value: f64, step: f64) -> f64 {
  info!("{value}");
  info!("{step}");
  (value / step).floor() * step
}

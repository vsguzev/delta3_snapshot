use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OrderStatus {
    New,
    PartiallyFilled,
    Filled,
    Cancelled,
    Rejected,
}

#[derive(Debug, Clone)]
pub struct Order {
    pub id: Uuid,
    pub side: Side,
    pub price: f64,
    pub size: f64,
    pub filled_size: f64,
    pub status: OrderStatus,
    pub timestamp: u64,
}

impl Order {
    pub fn new(side: Side, price: f64, size: f64, timestamp: u64) -> Self {
        Self {
            id: Uuid::new_v4(),
            side,
            price,
            size,
            filled_size: 0.0,
            status: OrderStatus::New,
            timestamp,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Trade {
    pub id: Uuid,
    pub maker_order_id: Uuid,
    pub taker_order_id: Uuid,
    pub price: f64,
    pub size: f64,
    pub timestamp: u64,
}

#[derive(Debug, Clone)]
pub struct OrderBook {
    pub bids: BTreeMap<i64, Vec<Order>>,
    pub asks: BTreeMap<i64, Vec<Order>>,
}

impl Default for OrderBook {
    fn default() -> Self {
        Self {
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Position {
    pub size: f64,
    pub entry_price: f64,
    pub notional_value: f64,
}

impl Position {
    pub fn new() -> Self {
        Self {
            size: 0.0,
            entry_price: 0.0,
            notional_value: 0.0,
        }
    }

    pub fn update(&mut self, fill_size: f64, fill_price: f64, is_buy: bool) -> f64 {
        let old_size = self.size;
        let fill_notional = fill_size * fill_price;
        let mut realized_pnl = 0.0;

        if is_buy {
            self.size += fill_size;
            if old_size < 0.0 {
                let cover_size = if self.size > 0.0 {
                    -old_size
                } else {
                    fill_size
                };

                realized_pnl = (self.entry_price - fill_price) * cover_size;

                if self.size > 0.0 {
                    self.notional_value = (fill_size + old_size) * fill_price;
                } else {
                    self.notional_value = -self.size * self.entry_price;
                }
            } else {
                self.notional_value += fill_notional;
            }
        } else {
            self.size -= fill_size;
            if old_size > 0.0 {
                let close_size = if self.size < 0.0 { old_size } else { fill_size };

                realized_pnl = (fill_price - self.entry_price) * close_size;

                if self.size < 0.0 {
                    self.notional_value = -self.size * fill_price;
                } else {
                    self.notional_value = self.size * self.entry_price;
                }
            } else {
                self.notional_value += fill_notional;
            }
        }

        if self.size != 0.0 {
            self.entry_price = self.notional_value / self.size.abs();
        } else {
            self.entry_price = 0.0;
            self.notional_value = 0.0;
        }

        realized_pnl
    }

    pub fn calculate_pnl(&self, current_price: f64) -> (f64, f64) {
        if self.size == 0.0 {
            return (0.0, 0.0);
        }

        let current_value = self.size.abs() * current_price;
        let unrealized_pnl = if self.size > 0.0 {
            current_value - self.notional_value
        } else {
            self.notional_value - current_value
        };

        (unrealized_pnl, current_value)
    }
}

pub trait Exchange {
    fn place_order(&mut self, side: Side, price: f64, size: f64) -> Uuid;

    fn cancel_order(&mut self, order_id: Uuid) -> bool;

    fn get_order(&self, order_id: Uuid) -> Option<&Order>;

    fn get_trades(&self) -> &[Trade];

    fn get_current_price(&self) -> f64;

    fn get_position(&self) -> f64;

    fn get_position_info(&self) -> (f64, f64, f64);

    fn update_price(&mut self) -> u64;

    fn get_realized_pnl(&self) -> f64;

    fn get_unrealized_pnl(&self) -> f64;
}

pub fn price_to_level(price: f64) -> i64 {
    (price * 1_000_000.0) as i64
}

pub fn level_to_price(level: i64) -> f64 {
    level as f64 / 1_000_000.0
}

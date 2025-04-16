use super::types::{Order, OrderBook, OrderStatus, Side, Trade};
use rand::Rng;
use rand_distr::{Distribution, Normal};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{atomic::AtomicBool, Arc},
};
use uuid::Uuid;

pub trait Exchange {
    fn place_order(&mut self, side: Side, price: f64, size: f64) -> Uuid;
    fn cancel_order(&mut self, order_id: Uuid) -> Arc<AtomicBool>;
    fn get_order(&self, order_id: Uuid) -> Option<&Order>;
    fn get_trades(&self) -> &[Trade];
    fn get_current_price(&self) -> f64;
    fn get_position(&self) -> f64;
    fn get_position_info(&self) -> (f64, f64, f64);
    fn get_realized_pnl(&self) -> f64;
    fn get_unrealized_pnl(&self) -> f64;
}

#[derive(Debug, Clone)]
pub struct Position {
    size: f64,
    entry_price: f64,
    notional_value: f64,
}

impl Position {
    fn new() -> Self {
        Self {
            size: 0.0,
            entry_price: 0.0,
            notional_value: 0.0,
        }
    }

    fn update(&mut self, fill_size: f64, fill_price: f64, is_buy: bool) -> f64 {
        let old_size = self.size;
        let old_notional = self.notional_value;
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

    fn calculate_pnl(&self, current_price: f64) -> (f64, f64) {
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

pub struct ExchangeSimulator {
    order_book: OrderBook,
    orders: HashMap<Uuid, Order>,
    trades: Vec<Trade>,
    current_price: f64,
    timestamp: u64,
    normal_dist: Normal<f64>,
    position: Position,
    realized_pnl: f64,
}

impl ExchangeSimulator {
    pub fn new(initial_price: f64) -> Self {
        Self {
            order_book: OrderBook::default(),
            orders: HashMap::new(),
            trades: Vec::new(),
            current_price: initial_price,
            timestamp: 0,
            normal_dist: Normal::new(0.0, 0.01).unwrap(),
            position: Position::new(),
            realized_pnl: 0.0,
        }
    }

    pub fn place_order(&mut self, side: Side, price: f64, size: f64) -> Uuid {
        let order = Order::new(side, price, size, self.timestamp);
        let order_id = order.id;
        let price_level = price_to_level(price);

        match side {
            Side::Buy => {
                self.order_book
                    .bids
                    .entry(price_level)
                    .or_insert_with(Vec::new)
                    .push(order.clone());
            }
            Side::Sell => {
                self.order_book
                    .asks
                    .entry(price_level)
                    .or_insert_with(Vec::new)
                    .push(order.clone());
            }
        }

        self.orders.insert(order_id, order);
        self.match_orders();
        order_id
    }

    pub fn cancel_order(&mut self, order_id: Uuid) -> Arc<AtomicBool> {
        if let Some(order) = self.orders.get_mut(&order_id) {
            if order.status == OrderStatus::Cancelled {
                return Arc::new(AtomicBool::new(false));
            }

            order.status = OrderStatus::Cancelled;
            let price_level = price_to_level(order.price);

            match order.side {
                Side::Buy => {
                    if let Some(orders) = self.order_book.bids.get_mut(&price_level) {
                        orders.retain(|o| o.id != order_id);
                        if orders.is_empty() {
                            self.order_book.bids.remove(&price_level);
                        }
                    }
                }
                Side::Sell => {
                    if let Some(orders) = self.order_book.asks.get_mut(&price_level) {
                        orders.retain(|o| o.id != order_id);
                        if orders.is_empty() {
                            self.order_book.asks.remove(&price_level);
                        }
                    }
                }
            }
            Arc::new(AtomicBool::new(true))
        } else {
            Arc::new(AtomicBool::new(false))
        }
    }

    pub fn simulate_price_update(&mut self) -> u64 {
        let mut rng = rand::thread_rng();
        let change = self.normal_dist.sample(&mut rng);
        self.current_price *= 1.0 + change;
        self.timestamp += 1;
        self.match_orders();
        self.timestamp
    }

    fn match_orders(&mut self) {
        let mut matched_trades = Vec::new();

        self.match_market_orders(&mut matched_trades);

        self.match_limit_orders(&mut matched_trades);

        for trade in matched_trades {
            if let Some(order) = self.orders.get_mut(&trade.maker_order_id) {
                order.filled_size += trade.size;
                order.status = if order.filled_size >= order.size {
                    OrderStatus::Filled
                } else {
                    OrderStatus::PartiallyFilled
                };
            }

            if trade.taker_order_id != trade.maker_order_id {
                if let Some(order) = self.orders.get_mut(&trade.taker_order_id) {
                    order.filled_size += trade.size;
                    order.status = if order.filled_size >= order.size {
                        OrderStatus::Filled
                    } else {
                        OrderStatus::PartiallyFilled
                    };
                }
            }

            self.trades.push(trade);
        }
    }

    fn match_market_orders(&mut self, matched_trades: &mut Vec<Trade>) {
        let mut bid_updates = Vec::new();
        for (&bid_level, bids) in self.order_book.bids.iter_mut().rev() {
            let bid_price = level_to_price(bid_level);
            if bid_price < self.current_price {
                break;
            }

            let mut i = 0;
            while i < bids.len() {
                let bid = &bids[i];
                if bid.status == OrderStatus::New || bid.status == OrderStatus::PartiallyFilled {
                    let remaining_size = bid.size - bid.filled_size;
                    bid_updates.push((bid_level, i, bid.clone(), remaining_size));
                }
                i += 1;
            }
        }

        for (level, index, order, size) in bid_updates {
            let pnl = self.position.update(size, self.current_price, true);
            self.realized_pnl += pnl;

            matched_trades.push(Trade {
                id: Uuid::new_v4(),
                maker_order_id: order.id,
                taker_order_id: order.id,
                price: self.current_price,
                size,
                timestamp: self.timestamp,
            });

            if let Some(orders) = self.order_book.bids.get_mut(&level) {
                if index < orders.len() {
                    orders.remove(index);
                    if orders.is_empty() {
                        self.order_book.bids.remove(&level);
                    }
                }
            }
        }

        let mut ask_updates = Vec::new();
        for (&ask_level, asks) in self.order_book.asks.iter_mut() {
            let ask_price = level_to_price(ask_level);
            if ask_price > self.current_price {
                break;
            }

            let mut i = 0;
            while i < asks.len() {
                let ask = &asks[i];
                if ask.status == OrderStatus::New || ask.status == OrderStatus::PartiallyFilled {
                    let remaining_size = ask.size - ask.filled_size;
                    ask_updates.push((ask_level, i, ask.clone(), remaining_size));
                }
                i += 1;
            }
        }

        for (level, index, order, size) in ask_updates {
            let pnl = self.position.update(size, self.current_price, false);
            self.realized_pnl += pnl;

            matched_trades.push(Trade {
                id: Uuid::new_v4(),
                maker_order_id: order.id,
                taker_order_id: order.id,
                price: self.current_price,
                size,
                timestamp: self.timestamp,
            });

            if let Some(orders) = self.order_book.asks.get_mut(&level) {
                if index < orders.len() {
                    orders.remove(index);
                    if orders.is_empty() {
                        self.order_book.asks.remove(&level);
                    }
                }
            }
        }
    }

    fn match_limit_orders(&mut self, matched_trades: &mut Vec<Trade>) {
        loop {
            let best_bid_level = self
                .order_book
                .bids
                .iter()
                .next_back()
                .map(|(&level, _)| level);
            let best_ask_level = self.order_book.asks.iter().next().map(|(&level, _)| level);

            let (bid_level, ask_level) = match (best_bid_level, best_ask_level) {
                (Some(bl), Some(al)) => {
                    let bid_price = level_to_price(bl);
                    let ask_price = level_to_price(al);
                    if bid_price < ask_price {
                        break;
                    }
                    (bl, al)
                }
                _ => break,
            };

            let bid_price = level_to_price(bid_level);
            let ask_price = level_to_price(ask_level);
            let trade_price = (bid_price + ask_price) / 2.0;

            let mut remove_bid_level = false;
            let mut remove_ask_level = false;

            if let (Some(bids), Some(asks)) = (
                self.order_book.bids.get_mut(&bid_level),
                self.order_book.asks.get_mut(&ask_level),
            ) {
                while !bids.is_empty() && !asks.is_empty() {
                    let bid = &bids[0];
                    let ask = &asks[0];

                    if bid.status != OrderStatus::New && bid.status != OrderStatus::PartiallyFilled
                    {
                        bids.remove(0);
                        if bids.is_empty() {
                            remove_bid_level = true;
                            break;
                        }
                        continue;
                    }
                    if ask.status != OrderStatus::New && ask.status != OrderStatus::PartiallyFilled
                    {
                        asks.remove(0);
                        if asks.is_empty() {
                            remove_ask_level = true;
                            break;
                        }
                        continue;
                    }

                    let bid_remaining = bid.size - bid.filled_size;
                    let ask_remaining = ask.size - ask.filled_size;
                    let trade_size = bid_remaining.min(ask_remaining);

                    let pnl = self.position.update(trade_size, trade_price, true);
                    self.realized_pnl += pnl;

                    matched_trades.push(Trade {
                        id: Uuid::new_v4(),
                        maker_order_id: ask.id,
                        taker_order_id: bid.id,
                        price: trade_price,
                        size: trade_size,
                        timestamp: self.timestamp,
                    });

                    {
                        let bid = &mut bids[0];
                        bid.filled_size += trade_size;
                        bid.status = if bid.filled_size >= bid.size {
                            OrderStatus::Filled
                        } else {
                            OrderStatus::PartiallyFilled
                        };
                    }

                    {
                        let ask = &mut asks[0];
                        ask.filled_size += trade_size;
                        ask.status = if ask.filled_size >= ask.size {
                            OrderStatus::Filled
                        } else {
                            OrderStatus::PartiallyFilled
                        };
                    }

                    if bids[0].status == OrderStatus::Filled {
                        bids.remove(0);
                        if bids.is_empty() {
                            remove_bid_level = true;
                            break;
                        }
                    }
                    if asks[0].status == OrderStatus::Filled {
                        asks.remove(0);
                        if asks.is_empty() {
                            remove_ask_level = true;
                            break;
                        }
                    }
                }
            }

            if remove_bid_level {
                self.order_book.bids.remove(&bid_level);
            }
            if remove_ask_level {
                self.order_book.asks.remove(&ask_level);
            }
        }
    }
}

impl Exchange for ExchangeSimulator {
    fn place_order(&mut self, side: Side, price: f64, size: f64) -> Uuid {
        ExchangeSimulator::place_order(self, side, price, size)
    }

    fn cancel_order(&mut self, order_id: Uuid) -> Arc<AtomicBool> {
        ExchangeSimulator::cancel_order(self, order_id)
    }

    fn get_order(&self, order_id: Uuid) -> Option<&Order> {
        self.orders.get(&order_id)
    }

    fn get_trades(&self) -> &[Trade] {
        &self.trades
    }

    fn get_current_price(&self) -> f64 {
        self.current_price
    }

    fn get_position(&self) -> f64 {
        self.position.size
    }

    fn get_position_info(&self) -> (f64, f64, f64) {
        (
            self.position.size,
            self.position.entry_price,
            self.position.notional_value,
        )
    }

    fn get_realized_pnl(&self) -> f64 {
        self.realized_pnl
    }

    fn get_unrealized_pnl(&self) -> f64 {
        self.position.calculate_pnl(self.current_price).0
    }
}

fn price_to_level(price: f64) -> i64 {
    (price * 1_000_000.0) as i64
}

fn level_to_price(level: i64) -> f64 {
    level as f64 / 1_000_000.0
}

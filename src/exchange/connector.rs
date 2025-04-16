use super::types::{Order, OrderStatus, Side, Trade};
use super::Exchange;
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use uuid::Uuid;

pub struct ExchangeConnector {
    api_key: String,
    api_secret: String,
    base_url: String,
    orders: HashMap<Uuid, Order>,
    trades: Vec<Trade>,
    current_price: f64,
    position_size: f64,
    position_entry: f64,
    position_notional: f64,
    realized_pnl: f64,
}

impl ExchangeConnector {
    pub fn new(api_key: &str, api_secret: &str, base_url: &str, initial_price: f64) -> Self {
        Self {
            api_key: api_key.to_string(),
            api_secret: api_secret.to_string(),
            base_url: base_url.to_string(),
            orders: HashMap::new(),
            trades: Vec::new(),
            current_price: initial_price,
            position_size: 0.0,
            position_entry: 0.0,
            position_notional: 0.0,
            realized_pnl: 0.0,
        }
    }

    fn send_api_request(
        &self,
        endpoint: &str,
        params: HashMap<String, String>,
    ) -> Result<String, String> {
        Ok("stub response".to_string())
    }
}

impl Exchange for ExchangeConnector {
    fn place_order(&mut self, side: Side, price: f64, size: f64) -> Uuid {
        let timestamp = chrono::Utc::now().timestamp() as u64;
        let order = Order::new(side, price, size, timestamp);
        let order_id = order.id;

        self.orders.insert(order_id, order);

        order_id
    }

    fn cancel_order(&mut self, order_id: Uuid) -> Arc<AtomicBool> {
        if let Some(order) = self.orders.get_mut(&order_id) {
            order.status = OrderStatus::Cancelled;
            Arc::new(AtomicBool::new(true))
        } else {
            Arc::new(AtomicBool::new(false))
        }
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
        self.position_size
    }

    fn get_position_info(&self) -> (f64, f64, f64) {
        (
            self.position_size,
            self.position_entry,
            self.position_notional,
        )
    }

    fn get_realized_pnl(&self) -> f64 {
        self.realized_pnl
    }

    fn get_unrealized_pnl(&self) -> f64 {
        let unrealized = if self.position_size > 0.0 {
            (self.current_price - self.position_entry) * self.position_size
        } else if self.position_size < 0.0 {
            (self.position_entry - self.current_price) * self.position_size.abs()
        } else {
            0.0
        };

        unrealized
    }
}

use std::sync::Arc;

use crate::exchange::simulator::ExchangeSimulator;
use crate::exchange::types::{OrderStatus, Side};
use crate::hyperliquid::types::{ClientLimit, ClientOrder, ClientOrderRequest};
use crate::hyperliquid::{truncate_float, ExchangeClient, OrderUpdate, Trade, TradeInfo};
use crate::services::TimeSeries;
use crate::statistics::{ProbabilityMatrix, RollingStats};
use chrono::Utc;
use log::{debug, error, info, warn};
use nalgebra::DMatrix;
use tokio::sync::{mpsc, Mutex};
use uuid::Uuid;

use crate::manager::*;

#[derive(Debug, Clone)]
pub struct PairConfig {
    asset: String,
    position: f64,
    entry_price: f64,
    max_leverage: u32,
    account_value: f64,
    px_decimals: u32,
    sz_decimals: u32,
}

impl PairConfig {
    pub fn new(
        asset: String,
        position: f64,
        entry_price: f64,
        max_leverage: u32,
        account_value: f64,
        px_decimals: u32,
        sz_decimals: u32,
    ) -> Self {
        Self {
            asset,
            position,
            entry_price,
            max_leverage,
            account_value,
            px_decimals,
            sz_decimals,
        }
    }

    pub fn asset(&self) -> &str {
        &self.asset
    }

    pub fn position(&self) -> f64 {
        self.position
    }

    pub fn entry_price(&self) -> f64 {
        self.entry_price
    }

    pub fn max_leverage(&self) -> u32 {
        self.max_leverage
    }

    pub fn account_value(&self) -> f64 {
        self.account_value
    }

    pub fn px_decimals(&self) -> u32 {
        self.px_decimals
    }

    pub fn sz_decimals(&self) -> u32 {
        self.sz_decimals
    }
}

#[derive(Debug, Clone)]
pub enum SubscriptionData {
    Price(f64),
    OrderUpdate(OrderUpdate),
    TradeInfo(TradeInfo),
    Trade(Trade),
}

#[derive(Debug, Clone, PartialEq)]
pub enum StrategyState {
    Idle,
    CalculatingDelta,
    PlacingOrders,
    Monitoring,
    CancellingOtherOrder,
}

#[derive(Debug, Clone)]
pub enum StrategyInput {
    ReceivedPrice,
    OrderFilled,
    DeltaCalculated,
    OrdersPlaced,
    OrderCancelled,
}

pub struct Strategy {
    config: PairConfig,
    manager: Arc<Mutex<TaskManager>>,
    rx: mpsc::Receiver<SubscriptionData>,
    capital: f64,
    active_orders: (Option<Uuid>, Option<Uuid>),

    ts: TimeSeries,
    time_window: u64,

    risk: f64,
    state: StrategyState,
    current_price: f64,
    last_trade_price: f64,
    last_trade_size: f64,
    position: f64,
    entry_price: f64,
    realized_pnl: f64,
    best_delta: f64,
    effective_capital: f64,
}

impl Strategy {
    pub async fn new(
        config: PairConfig,
        manager: Arc<Mutex<TaskManager>>,
        rx: mpsc::Receiver<SubscriptionData>,
        time_window: u64,
        risk: f64,
    ) -> Self {
        let capital = config.account_value();
        let position = config.position();
        let entry_price = config.entry_price();

        info!(
            "[{}] Initializing strategy with account value: ${:.2}, time window: {}s, risk: {}",
            config.asset(),
            capital,
            time_window,
            risk
        );
        info!(
            "[{}] Initial position: {} @ ${} (max leverage: {}x)",
            config.asset(),
            config.position(),
            config.entry_price(),
            config.max_leverage()
        );

        let strategy = Self {
            config,
            manager,
            rx,
            capital,
            active_orders: (None, None),
            ts: TimeSeries::new(),
            time_window,
            risk,
            state: StrategyState::Idle,
            current_price: 0.0,
            last_trade_price: 0.0,
            last_trade_size: 0.0,
            position,
            entry_price,
            realized_pnl: 0.0,
            best_delta: 0.0,
            effective_capital: 0.0,
        };

        strategy.cancel_all_orders_on_start().await;

        strategy
    }

    async fn cancel_all_orders_on_start(&self) {
        info!(
            "[{}] Cancelling all existing orders on strategy start",
            self.config.asset()
        );

        let manager = self.manager.clone();
        let asset = self.config.asset().to_string();

        let manager_lock = manager.lock().await;

        let result = manager_lock.cancel_all_orders(asset.clone(), true).await;

        match result {
            TaskResult::Success => info!(
                "[{}] Successfully cancelled all existing orders on strategy start",
                self.config.asset()
            ),
            TaskResult::Failure(e) => warn!(
                "[{}] Failed to cancel all orders on strategy start: {}",
                self.config.asset(),
                e
            ),
        }
    }

    pub fn process_input(&mut self, input: StrategyInput) {
        let old_state = self.state.clone();

        debug!(
            "[{}] Processing state machine input: {:?} in state {:?}",
            self.config.asset(),
            input,
            old_state
        );

        self.state = match (&self.state, input.clone()) {
            (StrategyState::Idle, StrategyInput::ReceivedPrice) => StrategyState::CalculatingDelta,
            (StrategyState::Idle, StrategyInput::OrderFilled) => {
                StrategyState::CancellingOtherOrder
            }
            (StrategyState::CalculatingDelta, StrategyInput::ReceivedPrice) => {
                StrategyState::CalculatingDelta
            }
            (StrategyState::PlacingOrders, StrategyInput::OrdersPlaced) => {
                StrategyState::Monitoring
            }
            (StrategyState::Monitoring, StrategyInput::ReceivedPrice) => {
                StrategyState::CalculatingDelta
            }
            (StrategyState::Monitoring, StrategyInput::OrderFilled) => {
                StrategyState::CancellingOtherOrder
            }
            (StrategyState::CancellingOtherOrder, StrategyInput::OrderCancelled) => {
                StrategyState::CalculatingDelta
            }
            (StrategyState::CalculatingDelta, StrategyInput::DeltaCalculated) => {
                StrategyState::PlacingOrders
            }
            _ => {
                debug!(
                    "[{}] No state transition for {:?} in state {:?}",
                    self.config.asset(),
                    input,
                    old_state
                );
                self.state.clone()
            }
        };

        if old_state != self.state {
            info!(
                "[{}] State transition: {:?} -> {:?}",
                self.config.asset(),
                old_state,
                self.state
            );
        }
    }

    pub fn calculate_best_delta(&self, probability_matrix: &DMatrix<f32>, min_delta: f64) -> f64 {
        debug!(
            "[{}] Calculating best delta with min_delta = {}",
            self.config.asset(),
            min_delta
        );

        let stats = RollingStats::compute(self.ts.get_candles());
        if stats.nrows() == 0 {
            warn!(
                "[{}] No statistics available, returning delta = 0.0",
                self.config.asset()
            );
            return 0.0;
        }

        let latest_std = stats[(0, 2)];
        let latest_ewma_vol = stats[(0, 3)];
        let latest_skewness = stats[(0, 4)];
        let latest_kurtosis = stats[(0, 5)];

        debug!(
            "[{}] Market conditions - std: {}, ewma_vol: {}, skew: {}, kurt: {}",
            self.config.asset(),
            latest_std,
            latest_ewma_vol,
            latest_skewness,
            latest_kurtosis
        );

        let mut best_delta = 0.0;
        let mut best_expected_value = f64::NEG_INFINITY;
        let base_step = 0.0001;

        debug!(
            "[{}] Searching for best delta across {} columns",
            self.config.asset(),
            probability_matrix.ncols()
        );

        for j in 0..probability_matrix.ncols() {
            let delta = j as f64 * base_step;

            if delta < min_delta {
                continue;
            }

            let mut total_prob = 0.0;
            for i in 0..probability_matrix.nrows() {
                let prob = probability_matrix[(i, j)] as f64;
                total_prob += prob;
            }

            let avg_prob = total_prob / probability_matrix.nrows() as f64;

            let size_penalty = (-delta.powi(2) * 1000.0).exp();
            let expected_value = avg_prob * delta * size_penalty;

            if expected_value > best_expected_value {
                best_expected_value = expected_value;
                best_delta = delta;
                debug!(
                    "[{}] New best delta found: {} with expected value: {}",
                    self.config.asset(),
                    best_delta,
                    best_expected_value
                );
            }
        }

        let adjusted_delta = best_delta;

        let final_delta = adjusted_delta.max(min_delta).min(0.05);

        info!(
            "[{}] Delta calculation: raw = {}, adjusted = {}, final = {}",
            self.config.asset(),
            best_delta,
            adjusted_delta,
            final_delta
        );

        final_delta
    }

    async fn handle_price_update(&mut self, price: f64) {
        debug!(
            "[{}] Received price update: ${}",
            self.config.asset(),
            price
        );

        self.current_price = price;
        let current_time = Utc::now().timestamp() as u64;

        let min_time = current_time.saturating_sub(self.time_window * 2);
        let removed = self.ts.remove_old_ticks(min_time);
        debug!(
            "[{}] Removed {} old ticks before timestamp {}",
            self.config.asset(),
            removed,
            min_time
        );

        self.ts.add_tick(current_time, price);
        debug!(
            "[{}] Added new tick at timestamp {} with price ${}",
            self.config.asset(),
            current_time,
            price
        );

        info!(
            "[{}] Triggering price-based state transition, current position: {}, P&L: ${:.2}",
            self.config.asset(),
            self.position,
            self.realized_pnl
        );
        self.process_input(StrategyInput::ReceivedPrice);
    }

    async fn handle_trade(&mut self, trade: Trade) {
        info!(
            "[{}] Received trade: {} @ ${}",
            self.config.asset(),
            trade.sz,
            trade.px
        );
        self.handle_price_update(trade.px.parse::<f64>().unwrap_or(self.current_price))
            .await;
    }

    fn normalize_order_id(id: &str) -> String {
        let id = id.strip_prefix("0x").unwrap_or(id);

        id.replace('-', "").to_lowercase()
    }

    async fn handle_order_update(&mut self, update: OrderUpdate) {
        debug!(
            "[{}] Received order update: status={}, size={}, price={}",
            self.config.asset(),
            update.status,
            update.order.sz,
            update.order.limit_px
        );

        match update.status.as_str() {
            "filled" => {
                if let Some(cloid) = &update.order.cloid {
                    let uuid_result = Uuid::parse_str(cloid);

                    let normalized_cloid = Self::normalize_order_id(cloid);
                    let normalized_buy_id = self
                        .active_orders
                        .0
                        .map(|id| Self::normalize_order_id(&id.to_string()))
                        .unwrap_or_default();
                    let normalized_sell_id = self
                        .active_orders
                        .1
                        .map(|id| Self::normalize_order_id(&id.to_string()))
                        .unwrap_or_default();

                    if normalized_cloid == normalized_buy_id {
                        let size = update.order.orig_sz.parse::<f64>().unwrap_or(0.0);
                        let price = update.order.limit_px.parse::<f64>().unwrap_or(0.0);
                        self.position += size;

                        info!(
                            "[{}] Buy order filled: +{} @ ${} (position now: {})",
                            self.config.asset(),
                            size,
                            price,
                            self.position
                        );

                        self.process_input(StrategyInput::OrderFilled);
                    } else if normalized_cloid == normalized_sell_id {
                        let size = update.order.orig_sz.parse::<f64>().unwrap_or(0.0);
                        let price = update.order.limit_px.parse::<f64>().unwrap_or(0.0);
                        self.position -= size;

                        info!(
                            "[{}] Sell order filled: -{} @ ${} (position now: {})",
                            self.config.asset(),
                            size,
                            price,
                            self.position
                        );

                        self.process_input(StrategyInput::OrderFilled);
                    } else {
                        warn!(
                            "[{}] Filled order with CLOID {} doesn't match our active orders",
                            self.config.asset(),
                            cloid
                        );
                    }
                }
            }
            "cancelled" => {
                if let Some(cloid) = &update.order.cloid {
                    let uuid = Uuid::parse_str(cloid).unwrap_or_default();

                    if Some(uuid) == self.active_orders.0 {
                        info!("[{}] Buy order cancelled", self.config.asset());
                        self.active_orders.0 = None;
                    } else if Some(uuid) == self.active_orders.1 {
                        info!("[{}] Sell order cancelled", self.config.asset());
                        self.active_orders.1 = None;
                    } else {
                        warn!(
                            "[{}] Cancelled order with CLOID {} doesn't match our active orders",
                            self.config.asset(),
                            uuid
                        );
                    }
                }

                debug!("[{}] Processing order cancellation", self.config.asset());
                self.process_input(StrategyInput::OrderCancelled);
            }
            status => {
                debug!(
                    "[{}] Received order update with status: {}",
                    self.config.asset(),
                    status
                );
            }
        }
    }

    async fn handle_trade_info(&mut self, info: TradeInfo) {
        let price = info.px.parse::<f64>().unwrap_or(0.0);
        let size = info.sz.parse::<f64>().unwrap_or(0.0);
        let pnl = info.closed_pnl.parse::<f64>().unwrap_or(0.0);

        info!(
            "[{}] Received trade info: {} {} @ ${} (P&L: ${:.2})",
            self.config.asset(),
            if info.side == "B" { "bought" } else { "sold" },
            size,
            price,
            pnl
        );

        self.last_trade_price = price;
        self.last_trade_size = size;
        self.entry_price = info.start_position.parse::<f64>().unwrap_or(0.0);
        self.realized_pnl = pnl;

        debug!(
            "[{}] Updated trade metrics - last price: ${}, entry price: ${}, P&L: ${:.2}",
            self.config.asset(),
            self.last_trade_price,
            self.entry_price,
            self.realized_pnl
        );
    }

    async fn handle_idle_state(&mut self) {
        debug!(
            "[{}] In IDLE state, waiting for events",
            self.config.asset()
        );
    }

    async fn handle_calculating_delta_state(&mut self) {
        info!(
            "[{}] CALCULATING DELTA - analyzing market data",
            self.config.asset()
        );

        let candles_before = self.ts.get_candles().len();
        self.ts.generate_candles();
        let candles_after = self.ts.get_candles().len();

        debug!(
            "[{}] Generated {} candles from time series data (total: {})",
            self.config.asset(),
            candles_after - candles_before,
            candles_after
        );

        let data_span = self.ts.get_candles().len() as u64;

        if data_span < self.time_window {
            let percentage_filled = if self.time_window > 0 {
                (data_span as f64 / self.time_window as f64 * 100.0).round()
            } else {
                0.0
            };

            info!(
                "[{}] Insufficient historical data: {}/{} seconds filled ({}%)",
                self.config.asset(),
                data_span,
                self.time_window,
                percentage_filled
            );
            info!(
                "[{}] Waiting for more data before placing orders",
                self.config.asset()
            );

            self.state = StrategyState::Monitoring;
            return;
        }

        let stats = RollingStats::compute(self.ts.get_candles());
        debug!(
            "[{}] Computed rolling statistics with {} rows",
            self.config.asset(),
            stats.nrows()
        );

        let deltas = 500;

        debug!(
            "[{}] Generating probability matrix with {} delta steps",
            self.config.asset(),
            deltas
        );
        let probability_matrix = ProbabilityMatrix::generate(&stats, deltas);

        let effective_capital = self.capital + self.realized_pnl;
        let min_delta = 10.0 / (self.risk * effective_capital);

        info!(
            "[{}] Capital calculation: base=${:.2}, P&L=${:.2}, effective=${:.2}",
            self.config.asset(),
            self.capital,
            self.realized_pnl,
            effective_capital
        );

        debug!(
            "[{}] Calculating best delta with min_delta={:.6}",
            self.config.asset(),
            min_delta
        );
        let best_delta = self.calculate_best_delta(&probability_matrix, min_delta);

        self.best_delta = best_delta;
        self.effective_capital = effective_capital;

        if best_delta <= 0.0 {
            info!(
                "[{}] Calculated delta is too small or zero ({}), remaining in current state",
                self.config.asset(),
                best_delta
            );

            self.state = StrategyState::Monitoring;
        } else if self.active_orders.0.is_some() || self.active_orders.1.is_some() {
            info!(
                "[{}] Active orders found, remaining in current state",
                self.config.asset()
            );
            self.state = StrategyState::Monitoring;
        } else {
            info!(
                "[{}] Calculated optimal delta: {} with effective capital: ${:.2}",
                self.config.asset(),
                best_delta,
                effective_capital
            );

            self.process_input(StrategyInput::DeltaCalculated);
        }
    }

    async fn handle_placing_orders_state(&mut self) {
        info!(
            "[{}] PLACING ORDERS - executing market orders",
            self.config.asset()
        );

        if self.best_delta <= 0.0 {
            warn!(
                "[{}] Delta too small ({}), skipping order placement",
                self.config.asset(),
                self.best_delta
            );
            self.process_input(StrategyInput::OrdersPlaced);
            return;
        }

        let central_price = if self.last_trade_price > 0.0 {
            self.last_trade_price
        } else {
            self.current_price
        };

        debug!(
            "[{}] Using central price: ${} (from {})",
            self.config.asset(),
            central_price,
            if self.last_trade_price > 0.0 {
                "last trade"
            } else {
                "current price"
            }
        );

        let position_value = self.position * self.current_price;
        let mut buy_delta = self.best_delta;
        let mut sell_delta = self.best_delta;

        debug!(
            "[{}] Position metrics - size: {}, value: ${:.2}, current price: ${}",
            self.config.asset(),
            self.position,
            position_value,
            self.current_price
        );

        if position_value > 0.0 {
            let adjustment = 1.0 / (1.0 - position_value / self.effective_capital);
            buy_delta *= adjustment;
            debug!(
                "[{}] Long position, increasing buy delta by factor {:.4}",
                self.config.asset(),
                adjustment
            );
        } else if position_value < 0.0 {
            let adjustment = 1.0 / (1.0 + position_value / self.effective_capital);
            sell_delta *= adjustment;
            debug!(
                "[{}] Short position, increasing sell delta by factor {:.4}",
                self.config.asset(),
                adjustment
            );
        }

        let order_size = self.effective_capital
            * self.risk
            * (self.best_delta * (1.0 + buy_delta.max(sell_delta)))
            / central_price;

        let buy_price = central_price * (1.0 / (1.0 + buy_delta));
        let sell_price = central_price * (1.0 + sell_delta);

        info!("[{}] Calculated order parameters:", self.config.asset());
        info!(
            "[{}]   - Buy:  {} @ ${:.2} (delta: {:.6})",
            self.config.asset(),
            order_size,
            buy_price,
            buy_delta
        );
        info!(
            "[{}]   - Sell: {} @ ${:.2} (delta: {:.6})",
            self.config.asset(),
            order_size,
            sell_price,
            sell_delta
        );

        let buy_id = Uuid::new_v4();
        let sell_id = Uuid::new_v4();

        let buy_request = create_order_request(
            &self.config.asset,
            true,
            truncate_float(buy_price, self.config.px_decimals(), true),
            truncate_float(order_size, self.config.sz_decimals(), true),
        );

        let sell_request = create_order_request(
            &self.config.asset,
            false,
            truncate_float(sell_price, self.config.px_decimals(), false),
            truncate_float(order_size, self.config.sz_decimals(), true),
        );

        let mut buy_request = buy_request;
        let mut sell_request = sell_request;
        buy_request.cloid = Some(buy_id);
        sell_request.cloid = Some(sell_id);

        debug!(
            "[{}] Generated order IDs - buy: {}, sell: {}",
            self.config.asset(),
            buy_id,
            sell_id
        );

        self.active_orders = (Some(buy_id), Some(sell_id));

        info!("[{}] Submitting orders to exchange", self.config.asset());
        let manager = self.manager.clone();

        tokio::spawn(async move {
            let manager_lock = manager.lock().await;
            let _ = manager_lock
                .bulk_place_orders(vec![buy_request.clone(), sell_request.clone()], true)
                .await;
        });

        info!(
            "[{}] Orders placed, transitioning to monitoring state",
            self.config.asset()
        );
        self.process_input(StrategyInput::OrdersPlaced);
    }

    async fn handle_monitoring_state(&mut self) {
        debug!(
            "[{}] MONITORING - watching market and active orders",
            self.config.asset()
        );

        let both_orders_active = self.active_orders.0.is_some() && self.active_orders.1.is_some();
        let buy_active = self.active_orders.0.is_some();
        let sell_active = self.active_orders.1.is_some();

        debug!(
            "[{}] Order status - buy: {}, sell: {}",
            self.config.asset(),
            if buy_active { "active" } else { "inactive" },
            if sell_active { "active" } else { "inactive" }
        );

        if !both_orders_active {
            if !buy_active && !sell_active {
                info!(
                    "[{}] Both orders are no longer active, recalculating strategy",
                    self.config.asset()
                );
            } else if !buy_active {
                info!(
                    "[{}] Buy order is no longer active, recalculating strategy",
                    self.config.asset()
                );
            } else {
                info!(
                    "[{}] Sell order is no longer active, recalculating strategy",
                    self.config.asset()
                );
            }

            self.process_input(StrategyInput::ReceivedPrice);
        }
    }

    async fn handle_cancelling_other_order_state(&mut self) {
        info!(
            "[{}] CANCELLING ORDERS - one order was filled, cancelling others",
            self.config.asset()
        );

        let buy_id = self.active_orders.0;
        let sell_id = self.active_orders.1;
        let manager = self.manager.clone();
        let asset = self.config.asset.clone();

        let mut orders_to_cancel = Vec::new();

        if let Some(buy_id) = buy_id {
            info!(
                "[{}] Will cancel buy order with ID: {}",
                self.config.asset(),
                buy_id
            );
            orders_to_cancel.push(buy_id);
        }

        if let Some(sell_id) = sell_id {
            info!(
                "[{}] Will cancel sell order with ID: {}",
                self.config.asset(),
                sell_id
            );
            orders_to_cancel.push(sell_id);
        }

        if !orders_to_cancel.is_empty() {
            debug!(
                "[{}] Sending bulk cancel request for {} orders",
                self.config.asset(),
                orders_to_cancel.len()
            );

            tokio::spawn(async move {
                let manager_lock = manager.lock().await;
                let _ = manager_lock
                    .bulk_cancel_orders(asset, orders_to_cancel, true)
                    .await;
            });
        } else {
            debug!("[{}] No active orders to cancel", self.config.asset());
        }

        debug!("[{}] Resetting active orders", self.config.asset());
        self.active_orders = (None, None);

        info!(
            "[{}] Order cancellation complete, transitioning to calculating delta state",
            self.config.asset()
        );
        self.process_input(StrategyInput::OrderCancelled);
    }

    pub async fn update(&mut self) {
        debug!(
            "[{}] Starting update cycle in state: {:?}",
            self.config.asset(),
            self.state
        );

        if let Some(data) = self.rx.recv().await {
            debug!(
                "[{}] Received subscription data: {:?}",
                self.config.asset(),
                match &data {
                    SubscriptionData::Price(p) => format!("Price: ${}", p),
                    SubscriptionData::OrderUpdate(_) => "Order Update".to_string(),
                    SubscriptionData::TradeInfo(_) => "Trade Info".to_string(),
                    SubscriptionData::Trade(_) => "Trade".to_string(),
                }
            );

            match data {
                SubscriptionData::Price(price) => {
                    self.handle_price_update(price).await;
                }
                SubscriptionData::OrderUpdate(update) => {
                    self.handle_order_update(update).await;
                }
                SubscriptionData::TradeInfo(info) => {
                    self.handle_trade_info(info).await;
                }
                SubscriptionData::Trade(trade) => {
                    self.handle_trade(trade).await;
                }
            }
        }

        debug!(
            "[{}] Executing state handler for state: {:?}",
            self.config.asset(),
            self.state
        );

        match self.state {
            StrategyState::Idle => {
                self.handle_idle_state().await;
            }
            StrategyState::CalculatingDelta => {
                self.handle_calculating_delta_state().await;
            }
            StrategyState::PlacingOrders => {
                self.handle_placing_orders_state().await;
            }
            StrategyState::Monitoring => {
                self.handle_monitoring_state().await;
            }
            StrategyState::CancellingOtherOrder => {
                self.handle_cancelling_other_order_state().await;
            }
        }

        debug!(
            "[{}] Completed update cycle, ending in state: {:?}",
            self.config.asset(),
            self.state
        );
    }

    pub fn get_current_state(&self) -> &StrategyState {
        &self.state
    }

    pub fn get_position(&self) -> f64 {
        self.position
    }

    pub fn get_realized_pnl(&self) -> f64 {
        self.realized_pnl
    }

    pub fn get_current_price(&self) -> f64 {
        self.current_price
    }

    pub fn emergency_cancel_all_orders(&self) {
        info!(
            "[{}] EMERGENCY: Cancelling all orders immediately",
            self.config.asset()
        );

        let manager = self.manager.clone();
        let asset = self.config.asset().to_string();

        tokio::spawn(async move {
            let manager_lock = manager.lock().await;
            manager_lock.cancel_all_orders_non_blocking(asset);
        });
    }
}

pub fn create_order_request(
    asset: &str,
    is_buy: bool,
    price: f64,
    size: f64,
) -> ClientOrderRequest {
    let request = ClientOrderRequest {
        asset: asset.to_string(),
        is_buy,
        sz: size,
        limit_px: price,
        order_type: ClientOrder::Limit(ClientLimit {
            tif: "Gtc".to_string(),
        }),
        reduce_only: false,
        cloid: None,
    };

    debug!(
        "[{}] Created {} order request: {} @ ${}",
        asset,
        if is_buy { "buy" } else { "sell" },
        size,
        price
    );

    request
}

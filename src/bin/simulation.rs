use clap::Parser;
use delta3::{
    cli::Args,
    exchange::Exchange,
    exchange::{ExchangeSimulator, OrderStatus, Side, Trade},
    services::TimeSeries,
    statistics::{ProbabilityMatrix, RollingStats},
};
use nalgebra::DMatrix;
use rand::{rng, Rng};
use rand_distr::{num_traits::Float, Distribution, Normal};
use std::io::{self, Write};
use std::thread;
use std::time::Duration;
use uuid::Uuid;

struct Strategy {
    exchange: ExchangeSimulator,
    capital: f64,
    active_orders: (Option<Uuid>, Option<Uuid>),
    ts: TimeSeries,
    deltas: usize,
    time_limit_seconds: u64,
    performance_metrics: PerformanceMetrics,
    risk: f64,
}

struct PerformanceMetrics {
    trades: Vec<Trade>,
    initial_capital: f64,
    current_equity: f64,
    high_water_mark: f64,
    max_drawdown: f64,
    total_pnl: f64,
    winning_trades: usize,
    losing_trades: usize,
    total_profit: f64,
    total_loss: f64,
    equity_curve: Vec<(u64, f64)>,
    returns: Vec<f64>,
}

impl PerformanceMetrics {
    fn new(initial_capital: f64) -> Self {
        Self {
            trades: Vec::new(),
            initial_capital,
            current_equity: initial_capital,
            high_water_mark: initial_capital,
            max_drawdown: 0.0,
            total_pnl: 0.0,
            winning_trades: 0,
            losing_trades: 0,
            total_profit: 0.0,
            total_loss: 0.0,
            equity_curve: vec![(0, initial_capital)],
            returns: Vec::new(),
        }
    }

    fn update(&mut self, trade: &Trade, realized_pnl: f64, unrealized_pnl: f64) {
        self.trades.push(trade.clone());

        self.total_pnl = realized_pnl + unrealized_pnl;
        self.current_equity = self.initial_capital + self.total_pnl;

        if let Some((_, last_equity)) = self.equity_curve.last() {
            let return_pct = (self.current_equity - last_equity) / last_equity;
            self.returns.push(return_pct);
        }

        self.equity_curve
            .push((trade.timestamp, self.current_equity));

        if self.current_equity > self.high_water_mark {
            self.high_water_mark = self.current_equity;
        } else {
            let drawdown = (self.high_water_mark - self.current_equity) / self.high_water_mark;
            self.max_drawdown = self.max_drawdown.max(drawdown);
        }

        if let Some(prev_trade) = self
            .trades
            .len()
            .checked_sub(2)
            .and_then(|i| self.trades.get(i))
        {
            let trade_pnl = realized_pnl - self.total_pnl;

            if trade_pnl > 0.0 {
                self.winning_trades += 1;
                self.total_profit += trade_pnl;
            } else if trade_pnl < 0.0 {
                self.losing_trades += 1;
                self.total_loss += trade_pnl.abs();
            }
        }
    }

    fn calculate_metrics(&self) -> String {
        let total_trades = self.winning_trades + self.losing_trades;
        let win_rate = if total_trades > 0 {
            self.winning_trades as f64 / total_trades as f64
        } else {
            0.0
        };

        let profit_factor = if self.total_loss != 0.0 {
            self.total_profit / self.total_loss
        } else {
            f64::INFINITY
        };

        let (avg_return, std_dev) = if !self.returns.is_empty() {
            let sum_returns: f64 = self.returns.iter().sum();
            let avg = sum_returns / self.returns.len() as f64;

            let variance = self.returns.iter().map(|&r| (r - avg).powi(2)).sum::<f64>()
                / self.returns.len() as f64;

            (avg, variance.sqrt())
        } else {
            (0.0, 0.0)
        };

        let sharpe_ratio = if std_dev > 0.0 {
            let annualized_return = avg_return * 252.0;
            let annualized_std_dev = std_dev * (252.0_f64).sqrt();
            annualized_return / annualized_std_dev
        } else {
            0.0
        };

        let avg_trade_pnl = if total_trades > 0 {
            self.total_pnl / total_trades as f64
        } else {
            0.0
        };

        format!(
            "\nPerformance Metrics\n\
            Total PnL:      ${:.2}\n\
            Win Rate:       {:.1}%\n\
            Profit Factor:  {:.2}\n\
            Max Drawdown:   {:.1}%\n\
            Sharpe Ratio:   {:.2}\n\
            Avg Trade PnL:  ${:.2}\n\
            Total Trades:   {}\n\
            Winning Trades: {}\n\
            Losing Trades:  {}\n\
            Return:         {:.1}%\n",
            self.total_pnl,
            win_rate * 100.0,
            profit_factor,
            self.max_drawdown * 100.0,
            sharpe_ratio,
            avg_trade_pnl,
            total_trades,
            self.winning_trades,
            self.losing_trades,
            ((self.current_equity / self.initial_capital) - 1.0) * 100.0
        )
    }
}

impl Strategy {
    fn new(
        initial_price: f64,
        capital: f64,
        deltas: usize,
        time_limit_seconds: u64,
        risk: f64,
    ) -> Self {
        Self {
            exchange: ExchangeSimulator::new(initial_price),
            capital,
            active_orders: (None, None),
            ts: TimeSeries::new(),
            deltas,
            time_limit_seconds,
            performance_metrics: PerformanceMetrics::new(capital),
            risk,
        }
    }

    fn calculate_best_delta(&self, probability_matrix: &DMatrix<f32>, min_delta: f64) -> f64 {
        let stats = RollingStats::compute(self.ts.get_candles());
        if stats.nrows() == 0 {
            return 0.0;
        }

        let latest_std = stats[(0, 2)];
        let latest_ewma_vol = stats[(0, 3)];
        let latest_skewness = stats[(0, 4)];
        let latest_kurtosis = stats[(0, 5)];

        let mut best_delta = 0.0;
        let mut best_expected_value = f64::NEG_INFINITY;
        let base_step = 0.0001;

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
            }
        }

        let vol_factor = if latest_ewma_vol > latest_std {
            1.5
        } else {
            1.0
        };
        let skew_factor = if latest_skewness < -1.0 {
            1.2
        } else if latest_skewness > 1.0 {
            0.8
        } else {
            1.0
        };
        let kurt_factor = if latest_kurtosis > 3.0 {
            1.3
        } else if latest_kurtosis < -1.0 {
            0.9
        } else {
            1.0
        };

        let adjusted_delta = best_delta * vol_factor * skew_factor * kurt_factor;

        adjusted_delta.max(min_delta).min(0.05)
    }

    fn update(&mut self) {
        let current_price = self.exchange.get_current_price();
        let current_time = self.exchange.simulate_price_update();

        let min_time = current_time.saturating_sub(self.time_limit_seconds);
        self.ts.remove_old_ticks(min_time);

        self.ts.add_tick(current_time as u64, current_price);

        self.ts.generate_candles();
        let stats = RollingStats::compute(self.ts.get_candles());
        let probability_matrix = ProbabilityMatrix::generate(&stats, self.deltas);

        let effective_capital = self.capital + self.exchange.get_realized_pnl();
        let min_delta = 10.0 / (self.risk * effective_capital);

        let best_delta = self.calculate_best_delta(&probability_matrix, min_delta);

        let (last_trade_size, last_trade_price) = self
            .exchange
            .get_trades()
            .last()
            .map(|t| (t.size, t.price))
            .unwrap_or((0.0, current_price));

        let central_price = last_trade_price;

        let (buy_price, sell_price, order_size) = if best_delta == 0.0 {
            (central_price, central_price, 0.0)
        } else {
            let position = self.exchange.get_position();
            let position_value = position * current_price;
            let mut buy_delta = best_delta;
            let mut sell_delta = best_delta;

            if position_value > 0.0 {
                buy_delta *= 1.0 / (1.0 - position_value / effective_capital).powf(10.0);
            } else if position_value < 0.0 {
                sell_delta *= 1.0 / (1.0 + position_value / effective_capital).powf(10.0);
            }
            let order_size =
                effective_capital * self.risk * (best_delta * (1.0 + buy_delta.max(sell_delta)))
                    / central_price;

            let buy_price = central_price * (1.0 / (1.0 + buy_delta));
            let sell_price = central_price * (1.0 + sell_delta);
            (buy_price, sell_price, order_size)
        };

        let position = self.exchange.get_position();
        let position_value = position * current_price;
        let (_, entry_price, _) = self.exchange.get_position_info();

        let (buy_status, buy_price_current, sell_status, sell_price_current) =
            match (self.active_orders.0, self.active_orders.1) {
                (Some(buy_id), Some(sell_id)) => {
                    let buy_order = self.exchange.get_order(buy_id);
                    let sell_order = self.exchange.get_order(sell_id);
                    match (buy_order, sell_order) {
                        (Some(buy), Some(sell)) => (buy.status, buy.price, sell.status, sell.price),
                        _ => (OrderStatus::Cancelled, 0.0, OrderStatus::Cancelled, 0.0),
                    }
                }
                _ => (OrderStatus::Cancelled, 0.0, OrderStatus::Cancelled, 0.0),
            };

        if let Some(trade) = self.exchange.get_trades().last() {
            self.performance_metrics.update(
                trade,
                self.exchange.get_realized_pnl(),
                self.exchange.get_unrealized_pnl(),
            );
        }

        let output = format!(
            "\x1B[2J\x1B[1;1H=== Status ===\n\
            Time: {}\n\
            Current Price: {:.2}\n\
            Central Price: {:.2}\n\
            \n\
            Position\n\
            Size:        {:.2}\n\
            Value:       {:.2}\n\
            Entry Price: {:.2}\n\
            \n\
            P&L\n\
            Realized:    {:.2}\n\
            Unrealized:  {:.2}\n\
            \n\
            Capital\n\
            Initial:     {:.2}\n\
            Effective:   {:.2}\n\
            Pos/Cap:     {:.1}%\n\
            \n\
            Active Orders\n\
            Buy:         {:.2} @ {:.2} [{:?}]\n\
            Sell:        {:.2} @ {:.2} [{:?}]\n\
            \n\
            Next Orders\n\
            Buy:         {:.2} @ {:.2}\n\
            Sell:        {:.2} @ {:.2}\n\
            Delta:       {:.2}%\n\
            \n\
            Last Trade\n\
            Size:        {:.2}\n\
            Price:       {:.2}\n\
            \n{}",
            current_time,
            current_price,
            central_price,
            position,
            position_value,
            if position != 0.0 { entry_price } else { 0.0 },
            self.exchange.get_realized_pnl(),
            self.exchange.get_unrealized_pnl(),
            self.capital,
            effective_capital,
            (position_value / effective_capital) * 100.0,
            order_size,
            buy_price_current,
            buy_status,
            order_size,
            sell_price_current,
            sell_status,
            order_size,
            buy_price,
            order_size,
            sell_price,
            best_delta * 100.0,
            last_trade_size,
            last_trade_price,
            &self.performance_metrics.calculate_metrics()
        );

        print!("{}", output);
        io::stdout().flush().unwrap();

        if best_delta > 0.0 {
            match (buy_status, sell_status) {
                (OrderStatus::Filled, OrderStatus::New) => {
                    self.exchange.cancel_order(self.active_orders.1.unwrap());
                    let new_buy_id = self.exchange.place_order(Side::Buy, buy_price, order_size);
                    let new_sell_id = self
                        .exchange
                        .place_order(Side::Sell, sell_price, order_size);
                    self.active_orders = (Some(new_buy_id), Some(new_sell_id));
                }
                (OrderStatus::New, OrderStatus::Filled) => {
                    self.exchange.cancel_order(self.active_orders.0.unwrap());
                    let new_buy_id = self.exchange.place_order(Side::Buy, buy_price, order_size);
                    let new_sell_id = self
                        .exchange
                        .place_order(Side::Sell, sell_price, order_size);
                    self.active_orders = (Some(new_buy_id), Some(new_sell_id));
                }
                (OrderStatus::New, OrderStatus::New) => {}
                _ => {
                    let new_buy_id = self.exchange.place_order(Side::Buy, buy_price, order_size);
                    let new_sell_id = self
                        .exchange
                        .place_order(Side::Sell, sell_price, order_size);
                    self.active_orders = (Some(new_buy_id), Some(new_sell_id));
                }
            }
        }
    }
}

fn main() {
    let args = Args::parse();
    let time = args.time.secs;
    let deltas = 100;
    let initial_price = 100.0;
    let capital = args.funds;
    let risk = args.risk;

    let mut strategy = Strategy::new(initial_price, capital, deltas, time, risk);

    loop {
        strategy.update();
    }
}

pub mod types;
pub mod simulator;
pub mod connector;
pub use simulator::Exchange;
pub use simulator::ExchangeSimulator;
pub use connector::ExchangeConnector;
pub use types::{Order, OrderBook, OrderStatus, Side, Trade};

#[cfg(test)]
mod tests {
    use super::*;
    use types::Side;

    #[test]
    fn test_exchange_trait() {
        // Create a simulator implementation that implements the Exchange trait
        let mut exchange: Box<dyn Exchange> = Box::new(ExchangeSimulator::new(100.0));
        
        // Test basic operations through the trait interface
        let order_id = exchange.place_order(Side::Buy, 99.0, 10.0);
        assert!(exchange.get_order(order_id).is_some());
        
        // Test cancel order
        assert!(exchange.cancel_order(order_id).load(std::sync::atomic::Ordering::SeqCst));
        
        // Test position tracking
        let buy_order_id = exchange.place_order(Side::Buy, 101.0, 5.0);
        assert_eq!(exchange.get_position(), 0.0); // No matches yet at this price
        
        // Sell at the same price should match
        let sell_order_id = exchange.place_order(Side::Sell, 101.0, 2.0);
        assert_eq!(exchange.get_position(), 2.0); // Bought 2 units
        
        // Check PnL
        assert!(exchange.get_unrealized_pnl() >= 0.0);
        assert_eq!(exchange.get_realized_pnl(), 0.0);
        
        // Get position info
        let (size, entry_price, notional) = exchange.get_position_info();
        assert_eq!(size, 2.0);
        assert!(entry_price > 0.0);
        assert!(notional > 0.0);
    }

    #[test]
    fn test_exchange_connector() {
        // This test doesn't actually connect to a real exchange, just tests the interface
        let mut exchange: Box<dyn Exchange> = Box::new(ExchangeConnector::new(
            "api_key", "api_secret", "https://exchange.example.com", 100.0
        ));
        
        // Test basic operations through the trait interface
        let order_id = exchange.place_order(Side::Buy, 99.0, 10.0);
        assert!(exchange.get_order(order_id).is_some());
        
        // Test cancel order
        assert!(exchange.cancel_order(order_id).load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn test_order_placement_and_cancellation() {
        let mut exchange = ExchangeSimulator::new(100.0);
        
        // Place a buy order
        let buy_id = exchange.place_order(Side::Buy, 99.0, 1.0);
        let buy_order = exchange.get_order(buy_id).unwrap();
        assert_eq!(buy_order.side, Side::Buy);
        assert_eq!(buy_order.price, 99.0);
        assert_eq!(buy_order.status, OrderStatus::New);
        
        // Cancel the order
        assert!(exchange.cancel_order(buy_id).load(std::sync::atomic::Ordering::SeqCst));
        let cancelled_order = exchange.get_order(buy_id).unwrap();
        assert_eq!(cancelled_order.status, OrderStatus::Cancelled);
    }

    #[test]
    fn test_order_matching() {
        let mut exchange = ExchangeSimulator::new(100.0);
        
        // Place a buy order
        let buy_id = exchange.place_order(Side::Buy, 100.0, 1.0);
        
        // Place a matching sell order
        let sell_id = exchange.place_order(Side::Sell, 100.0, 1.0);
        
        // Check orders are filled
        let buy_order = exchange.get_order(buy_id).unwrap();
        let sell_order = exchange.get_order(sell_id).unwrap();
        assert_eq!(buy_order.status, OrderStatus::Filled);
        assert_eq!(sell_order.status, OrderStatus::Filled);
        
        // Check trade was created
        let trades = exchange.get_trades();
        assert_eq!(trades.len(), 1);
        assert_eq!(trades[0].size, 1.0);
        assert_eq!(trades[0].price, 100.0);
    }

    #[test]
    fn test_partial_fills() {
        let mut exchange = ExchangeSimulator::new(100.0);
        
        // Place a large buy order
        let buy_id = exchange.place_order(Side::Buy, 100.0, 2.0);
        
        // Place a smaller sell order
        let sell_id = exchange.place_order(Side::Sell, 100.0, 1.0);
        
        // Check orders status
        let buy_order = exchange.get_order(buy_id).unwrap();
        let sell_order = exchange.get_order(sell_id).unwrap();
        assert_eq!(buy_order.status, OrderStatus::PartiallyFilled);
        assert_eq!(sell_order.status, OrderStatus::Filled);
        assert_eq!(buy_order.filled_size, 1.0);
        assert_eq!(sell_order.filled_size, 1.0);
    }

    #[test]
    fn test_price_simulation() {
        let mut exchange = ExchangeSimulator::new(100.0);
        let initial_price = exchange.get_current_price();
        
        // Simulate price updates
        for _ in 0..100 {
            exchange.simulate_price_update();
            let price = exchange.get_current_price();
            assert!(price > 0.0); // Price should always be positive
        }
        
        let final_price = exchange.get_current_price();
        assert!(final_price != initial_price); // Price should have changed
    }
} 
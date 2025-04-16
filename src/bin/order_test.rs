use delta3::{
    hyperliquid::*,
    manager::{ConnectionConfig, TaskManager, TaskPolicy, TaskResult},
};
use ethers::signers::LocalWallet;
use std::str::FromStr;
use uuid::Uuid;
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    let private_key = std::env::var("PRIVATE_KEY").expect("PRIVATE_KEY must be set");
    let wallet = LocalWallet::from_str(&private_key)?;

    let config = ConnectionConfig {
        wallet,
        base_url: Some(BaseUrl::Mainnet),
        vault_address: None,
        wallet_address: None,
    };

    let info_client = InfoClient::new(None, config.base_url.clone()).unwrap();
    let exchange_client = ExchangeClient::new(
        None,
        config.wallet.clone(),
        config.base_url.clone(),
        None,
        config.vault_address.clone(),
    )
    .await?;

    let task_manager = TaskManager::new(exchange_client, info_client, config);

    let asset = "BTC".to_string();
    let base_price = 83067.0;
    let size = 0.00013;

    let buy_price = base_price * 0.9;
    let buy_cloid = Uuid::new_v4();
    let buy_order = ClientOrderRequest {
        asset: asset.clone(),
        is_buy: true,
        sz: size,
        limit_px: truncate_float(buy_price, 0, false),
        order_type: ClientOrder::Limit(ClientLimit {
            tif: "Gtc".to_string(),
        }),
        reduce_only: false,
        cloid: Some(buy_cloid),
    };

    let sell_price = base_price * 1.1;
    let sell_cloid = Uuid::new_v4();
    let sell_order = ClientOrderRequest {
        asset: asset.clone(),
        is_buy: false,
        sz: size,
        limit_px: truncate_float(sell_price, 0, true),
        order_type: ClientOrder::Limit(ClientLimit {
            tif: "Gtc".to_string(),
        }),
        reduce_only: false,
        cloid: Some(sell_cloid),
    };

    println!("Placing orders...");
    let (buy_result, sell_result) = tokio::join!(
        task_manager.place_order(buy_order, true),
        task_manager.place_order(sell_order, true)
    );

    match buy_result {
        TaskResult::Success => println!("Buy order placed successfully at {}", buy_price),
        TaskResult::Failure(e) => println!("Failed to place buy order: {}", e),
    }
    match sell_result {
        TaskResult::Success => println!("Sell order placed successfully at {}", sell_price),
        TaskResult::Failure(e) => println!("Failed to place sell order: {}", e),
    }

    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

    println!("Cancelling orders...");
    let (cancel_buy_result, cancel_sell_result) = tokio::join!(
        task_manager.cancel_order_by_cloid(asset.clone(), buy_cloid, true),
        task_manager.cancel_order_by_cloid(asset.clone(), sell_cloid, true)
    );

    match cancel_buy_result {
        TaskResult::Success => println!("Buy order cancelled successfully"),
        TaskResult::Failure(e) => println!("Failed to cancel buy order: {}", e),
    }
    match cancel_sell_result {
        TaskResult::Success => println!("Sell order cancelled successfully"),
        TaskResult::Failure(e) => println!("Failed to cancel sell order: {}", e),
    }

    Ok(())
}

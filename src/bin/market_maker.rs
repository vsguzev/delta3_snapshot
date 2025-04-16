use delta3::{
    hyperliquid::*,
    manager::{ConnectionConfig, TaskManager, TaskPolicy, TaskResult},
    services::TimeSeries,
    statistics::{ProbabilityMatrix, RollingStats},
};
use ethers::{signers::{LocalWallet, Wallet}, types::H160};
use nalgebra::DMatrix;
use std::{
    collections::HashMap,
    str::FromStr,
    sync::Arc,
};
use tokio::{
    sync::{mpsc, Mutex},
    time::Duration,
};
use uuid::Uuid;
use futures_util::StreamExt;
use anyhow;
use log::{info, warn, error, debug, trace};
use std::pin::Pin;
use futures::Stream;

use delta3::strategy::*;


async fn handle_subscription(
    subscription: Subscription,
    task_manager: Arc<Mutex<TaskManager>>,
    sender_channels: Arc<HashMap<String, mpsc::Sender<SubscriptionData>>>,
) -> Result<(), anyhow::Error> {
    let mut subscription_stream = match subscription {
        Subscription::AllMids => Box::pin(task_manager.lock().await.subscribe_all_mids().await?) as Pin<Box<dyn Stream<Item = Message> + Send>>,
        Subscription::Trades { coin } => Box::pin(task_manager.lock().await.subscribe_trades(coin).await?) as Pin<Box<dyn Stream<Item = Message> + Send>>,
        Subscription::OrderUpdates { user } => Box::pin(task_manager.lock().await.subscribe_order_updates(user).await?) as Pin<Box<dyn Stream<Item = Message> + Send>>,
        Subscription::UserFills { user } => Box::pin(task_manager.lock().await.subscribe_user_fills(user).await?) as Pin<Box<dyn Stream<Item = Message> + Send>>,
        _ => {
            return Err(anyhow::anyhow!("Unsupported subscription type"));
        }
    };

    while let Some(message) = subscription_stream.next().await {
        match message {
            Message::AllMids(mids) => {
                for (asset, price) in mids.data.mids {
                    let price = f64::from_str(&price).unwrap();
                    if let Some(tx) = sender_channels.get(&asset) {
                        if let Err(e) = tx.send(SubscriptionData::Price(price)).await {
                            error!("Failed to send price update for {}: {}", asset, e);
                        }
                    }
                }
            }
            Message::Trades(trades) => {
                for trade in trades.data {
                    let asset = trade.coin.clone();
                    if let Some(tx) = sender_channels.get(&asset) {
                        if let Err(e) = tx.send(SubscriptionData::Trade(trade)).await {
                            error!("Failed to send trade info for {}: {}", asset, e);
                        }
                    }
                }
            }
            Message::UserFills(fills) => {
                
                if fills.data.is_snapshot.unwrap_or(false) {
                    
                    return Ok(());
                }
                info!("Received user fills: {:?}", fills);
                for fill in fills.data.fills {
                    let asset = fill.coin.clone();
                    if let Some(tx) = sender_channels.get(&asset) {
                        if let Err(e) = tx.send(SubscriptionData::TradeInfo(fill)).await {
                            error!("Failed to send trade info for {}: {}", asset, e);
                        }
                    }
                }
            }
            Message::OrderUpdates(updates) => {
                
                info!("Received order updates: {:?}", updates);
                for update in updates.data {
                    let asset = update.order.coin.clone();
                    info!("Received order update for {}: {:?}", asset, update);
                    if let Some(tx) = sender_channels.get(&asset) {
                        info!("Sending order update for {}: {:?}", asset, update);
                        if let Err(e) = tx.send(SubscriptionData::OrderUpdate(update)).await {
                            error!("Failed to send price update for {}: {}", asset, e);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    
    env_logger::init();
    info!("Starting market maker application");

    
    let private_key = "aeabf3a2e1f1fc29654e50689c39ace3bde54e7d348f6a7123b2bab57e19b22a";
    let wallet = LocalWallet::from_str(&private_key)?;
    let user = H160::from_str("0xA3ED069f18E64f3747AFDDE3b550a0A3CC26a747").unwrap();
    info!("Successfully loaded wallet");

    
    let config = ConnectionConfig {
        wallet: wallet.clone(),
        base_url: Some(BaseUrl::Mainnet),
        vault_address: None,
        wallet_address: Some(user),
    };
    info!("Created connection config with mainnet base URL");

    
    info!("Initializing info client...");
    let info_client = InfoClient::with_reconnect(None, config.base_url.clone()).unwrap();
    info!("Successfully initialized info client");

    info!("Initializing exchange client...");
    let exchange_client = ExchangeClient::new(
        None,
        config.wallet.clone(),
        config.base_url.clone(),
        None,
        config.vault_address.clone(),
    ).await?;
    info!("Successfully initialized exchange client");

    
    info!("Fetching current positions from exchange...");
    let user_positions = info_client.user_state(user).await?;
    
    
    let account_value = user_positions.cross_margin_summary.account_value
        .parse::<f64>()
        .unwrap_or_else(|_| {
            warn!("Failed to parse account value, using default value");
            10.0 
        });
    
    info!("Account value: ${:.2}", account_value);
    
    let positions_map: HashMap<String, (f64, f64, u32)> = user_positions.asset_positions
        .into_iter()
        .map(|pos| {
            let asset = pos.position.coin.clone();
            let position = pos.position.szi.parse::<f64>().unwrap_or(0.0);
            let entry_price = pos.position.entry_px.unwrap_or_else(|| "0".to_string()).parse::<f64>().unwrap_or(0.0);
            let max_leverage = pos.position.max_leverage;
            (asset, (position, entry_price, max_leverage))
        })
        .collect();
    info!("Retrieved positions for {} assets", positions_map.len());

    
    let pair_configs = vec![
        ("BTC", 0, 5),  
        
        
    ];
    let capital_allocation = 1.0 / pair_configs.len() as f64;
    
    let pairs = pair_configs.into_iter().map(|(symbol, px_decimals, sz_decimals)| {
        let (position, entry_price, max_leverage) = positions_map
            .get(symbol)
            .cloned()
            .unwrap_or((0.0, 0.0, 10)); 
        
        let pair_account_value = account_value * capital_allocation;
        
        info!("Asset: {}, Position: {}, Entry Price: ${}, Max Leverage: {}x, Account Value: ${:.2}, PX Decimals: {}, SZ Decimals: {}", 
              symbol, position, entry_price, max_leverage, pair_account_value, px_decimals, sz_decimals);
        
        PairConfig::new(
            symbol.to_string(), 
            position,
            entry_price,
            max_leverage,
            pair_account_value,
            px_decimals, 
            sz_decimals, 
        )
    }).collect::<Vec<_>>();
    
    info!("Configured trading pairs with position data and account values");

    
    let task_manager = Arc::new(Mutex::new(TaskManager::new(exchange_client, info_client, config)));
    info!("Created task manager");

    
    let mut senders = HashMap::new();
    let mut receivers = HashMap::new();
    
    
    for pair in &pairs {
        let (tx, rx) = mpsc::channel(100);
        senders.insert(pair.asset().to_string(), tx);
        receivers.insert(pair.asset().to_string(), rx);
    }
    
    info!("Created communication channels for all pairs");

    
    let shared_senders = Arc::new(senders);
    
    let pairs_clone = pairs.clone();
    let task_manager_for_subscription = task_manager.clone();
    let subscription_handle = tokio::spawn(async move {
        info!("Starting price subscription task");
        let mut futures = vec![
            handle_subscription(Subscription::AllMids, task_manager_for_subscription.clone(), shared_senders.clone()),
            handle_subscription(Subscription::OrderUpdates { user }, task_manager_for_subscription.clone(), shared_senders.clone()),
            handle_subscription(Subscription::UserFills { user }, task_manager_for_subscription.clone(), shared_senders.clone()),
        ];
        for pair in &pairs_clone {
            futures.push(handle_subscription(Subscription::Trades { coin: pair.asset().to_string() }, task_manager_for_subscription.clone(), shared_senders.clone()));
        }
        futures::future::join_all(futures).await;
    });

    
    let task_manager_for_pairs = task_manager.clone();
    
    info!("Spawning tasks for each trading pair");
    let mut handles = Vec::new();
    for pair in pairs {
        let task_manager = task_manager_for_pairs.clone();
        let rx = receivers.remove(pair.asset()).unwrap();
        
        let handle = tokio::spawn(process_pair(pair, task_manager, rx));
        handles.push(handle);
    }
    info!("All pair tasks spawned");

    
    info!("Waiting for all tasks to complete");
    subscription_handle.await.expect("Subscription task failed");
    for handle in handles {
        if let Err(e) = handle.await {
            error!("Task failed with error: {}", e);
        }
    }
    info!("All tasks completed");

    Ok(())
}


async fn process_pair(
    pair_config: PairConfig,
    task_manager: Arc<Mutex<TaskManager>>,
    rx: mpsc::Receiver<SubscriptionData>,
) -> Result<(), anyhow::Error> {
    info!("Starting processing for pair: {}", pair_config.asset());
    
    
    let mut strategy = Strategy::new(
        pair_config.clone(),
        task_manager.clone(),
        rx,
        60,    
        10.0,     
    ).await;
    
    info!("Strategy initialized for {}", pair_config.asset());
    
    
    loop {
        
        
        
        
        strategy.update().await;
        
        
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    
    #[allow(unreachable_code)]
    Ok(())
} 

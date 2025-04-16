use ethers::types::H160;
use futures_util::Stream;
use futures_util::StreamExt;
use log::debug;
use log::{error, info, warn};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, Mutex};
use uuid::Uuid;

use crate::hyperliquid::ExchangeResponseStatus;
use crate::hyperliquid::*;

#[derive(Debug, Clone)]
pub enum Task {
    Cancel(ClientCancelRequest),
    CancelCloid(ClientCancelRequestCloid),
    Modify(ClientModifyRequest),
    Order(ClientOrderRequest),
    BulkOrder(Vec<ClientOrderRequest>),
    BulkCancel(Vec<ClientCancelRequest>),
}

#[derive(Debug, Clone)]
pub struct TaskPolicy {
    pub is_strict: bool,
    pub max_retries: u32,
    pub retry_delay_ms: u64,
}

impl Default for TaskPolicy {
    fn default() -> Self {
        Self {
            is_strict: true,
            max_retries: 3,
            retry_delay_ms: 100,
        }
    }
}

#[derive(Debug, Clone)]
pub enum TaskResult {
    Success,
    Failure(String),
}

pub struct ConnectionConfig {
    pub wallet: ethers::signers::LocalWallet,
    pub base_url: Option<BaseUrl>,
    pub vault_address: Option<ethers::types::H160>,
    pub wallet_address: Option<ethers::types::H160>,
}

pub struct TaskManager {
    tx: mpsc::Sender<(Task, TaskPolicy, oneshot::Sender<TaskResult>)>,
    exchange_client: Arc<Mutex<ExchangeClient>>,
    info_client: Arc<Mutex<InfoClient>>,
    connection_config: Arc<ConnectionConfig>,
}

impl TaskManager {
    pub fn new(
        exchange_client: ExchangeClient,
        info_client: InfoClient,
        connection_config: ConnectionConfig,
    ) -> Self {
        let (tx, mut rx) = mpsc::channel(100);
        let exchange_client = Arc::new(Mutex::new(exchange_client));
        let info_client = Arc::new(Mutex::new(info_client));
        let connection_config = Arc::new(connection_config);

        let manager = Self {
            tx,
            exchange_client: exchange_client.clone(),
            info_client: info_client.clone(),
            connection_config: connection_config.clone(),
        };

        tokio::spawn(async move {
            while let Some((task, policy, result_tx)) = rx.recv().await {
                let task_exchange_client = exchange_client.clone();
                let task_info_client = info_client.clone();
                let task_connection_config = connection_config.clone();
                debug!("Received task: {:?}", task);

                tokio::spawn(async move {
                    let mut retries = 0;
                    let mut success = false;

                    while !success && (policy.is_strict || retries < policy.max_retries) {
                        let task_result = {
                            let delegate = TaskDelegate::new(&task, task_exchange_client.clone());

                            delegate.process().await
                        };
                        debug!("Task result: {:?}", task_result);

                        match task_result {
                            Ok(_) => {
                                success = true;

                                let _ = result_tx.send(TaskResult::Success);
                                break;
                            }
                            Err(e) => {
                                retries += 1;

                                let error_string = e.to_string();
                                let connection_error = error_string.contains("connection")
                                    || error_string.contains("timeout")
                                    || error_string.contains("network");

                                if connection_error {
                                    warn!(
                                        "Connection error detected: {}. Attempting to reconnect...",
                                        error_string
                                    );

                                    match reconnect(
                                        &task_exchange_client,
                                        &task_info_client,
                                        &task_connection_config,
                                    )
                                    .await
                                    {
                                        Ok(_) => info!("Successfully reconnected"),
                                        Err(e) => error!("Failed to reconnect: {}", e),
                                    }
                                }

                                if !policy.is_strict || retries >= policy.max_retries {
                                    let _ = result_tx.send(TaskResult::Failure(error_string));
                                    break;
                                } else {
                                    tokio::time::sleep(tokio::time::Duration::from_millis(
                                        policy.retry_delay_ms,
                                    ))
                                    .await;
                                }
                            }
                        }
                    }
                });
            }
        });

        manager
    }

    pub async fn submit_task(&self, task: Task, policy: TaskPolicy) -> TaskResult {
        let (result_tx, result_rx) = oneshot::channel();
        match self.tx.send((task, policy, result_tx)).await {
            Ok(_) => result_rx.await.unwrap_or(TaskResult::Failure(
                "Failed to receive task result".to_string(),
            )),
            Err(_) => TaskResult::Failure("Failed to submit task".to_string()),
        }
    }

    pub async fn cancel_order(&self, asset: String, oid: u64, is_strict: bool) -> TaskResult {
        let task = Task::Cancel(ClientCancelRequest { asset, oid });
        let policy = TaskPolicy {
            is_strict,
            ..Default::default()
        };
        self.submit_task(task, policy).await
    }

    pub async fn cancel_order_by_cloid(
        &self,
        asset: String,
        cloid: Uuid,
        is_strict: bool,
    ) -> TaskResult {
        let task = Task::CancelCloid(ClientCancelRequestCloid { asset, cloid });
        let policy = TaskPolicy {
            is_strict,
            ..Default::default()
        };
        self.submit_task(task, policy).await
    }

    pub async fn modify_order(&self, request: ClientModifyRequest, is_strict: bool) -> TaskResult {
        let task = Task::Modify(request);
        let policy = TaskPolicy {
            is_strict,
            ..Default::default()
        };
        self.submit_task(task, policy).await
    }

    pub async fn place_order(&self, request: ClientOrderRequest, is_strict: bool) -> TaskResult {
        let task = Task::Order(request);
        let policy = TaskPolicy {
            is_strict,
            ..Default::default()
        };
        self.submit_task(task, policy).await
    }

    pub async fn subscribe_all_mids(
        &self,
    ) -> std::result::Result<impl Stream<Item = Message>, anyhow::Error> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut info_client = self.info_client.lock().await;
        info_client.subscribe(Subscription::AllMids, tx).await?;

        Ok(futures_util::stream::unfold(rx, |mut rx| {
            Box::pin(async move {
                match rx.recv().await {
                    Some(msg @ Message::AllMids(_)) => Some((msg, rx)),
                    Some(_) => None,
                    None => None,
                }
            })
        }))
    }

    pub async fn subscribe_notification(
        &self,
        user: H160,
    ) -> std::result::Result<impl Stream<Item = Message>, anyhow::Error> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut info_client = self.info_client.lock().await;
        info_client
            .subscribe(Subscription::Notification { user }, tx)
            .await?;

        Ok(futures_util::stream::unfold(rx, |mut rx| {
            Box::pin(async move {
                match rx.recv().await {
                    Some(msg @ Message::Notification(_)) => Some((msg, rx)),
                    Some(_) => None,
                    None => None,
                }
            })
        }))
    }

    pub async fn subscribe_web_data2(
        &self,
        user: H160,
    ) -> std::result::Result<impl Stream<Item = Message>, anyhow::Error> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let mut info_client = self.info_client.lock().await;
        info_client
            .subscribe(Subscription::WebData2 { user }, tx)
            .await?;

        Ok(futures_util::stream::unfold(rx, |mut rx| {
            Box::pin(async move {
                match rx.recv().await {
                    Some(msg @ Message::WebData2(_)) => Some((msg, rx)),
                    Some(_) => None,
                    None => None,
                }
            })
        }))
    }

    pub async fn subscribe_candle(
        &self,
        coin: String,
        interval: String,
    ) -> std::result::Result<impl Stream<Item = Message>, anyhow::Error> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut info_client = self.info_client.lock().await;
        info_client
            .subscribe(Subscription::Candle { coin, interval }, tx)
            .await?;

        Ok(futures_util::stream::unfold(rx, |mut rx| {
            Box::pin(async move {
                match rx.recv().await {
                    Some(msg @ Message::Candle(_)) => Some((msg, rx)),
                    Some(_) => None,
                    None => None,
                }
            })
        }))
    }

    pub async fn subscribe_l2_book(
        &self,
        coin: String,
    ) -> std::result::Result<impl Stream<Item = Message>, anyhow::Error> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut info_client = self.info_client.lock().await;
        info_client
            .subscribe(Subscription::L2Book { coin }, tx)
            .await?;

        Ok(futures_util::stream::unfold(rx, |mut rx| {
            Box::pin(async move {
                match rx.recv().await {
                    Some(msg @ Message::L2Book(_)) => Some((msg, rx)),
                    Some(_) => None,
                    None => None,
                }
            })
        }))
    }

    pub async fn subscribe_trades(
        &self,
        coin: String,
    ) -> std::result::Result<impl Stream<Item = Message>, anyhow::Error> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut info_client = self.info_client.lock().await;
        info_client
            .subscribe(Subscription::Trades { coin }, tx)
            .await?;

        Ok(futures_util::stream::unfold(rx, |mut rx| {
            Box::pin(async move {
                match rx.recv().await {
                    Some(msg @ Message::Trades(_)) => Some((msg, rx)),
                    Some(_) => None,
                    None => None,
                }
            })
        }))
    }

    pub async fn subscribe_order_updates(
        &self,
        user: H160,
    ) -> std::result::Result<impl Stream<Item = Message>, anyhow::Error> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut info_client = self.info_client.lock().await;
        info_client
            .subscribe(Subscription::OrderUpdates { user }, tx)
            .await?;

        Ok(futures_util::stream::unfold(rx, |mut rx| {
            Box::pin(async move {
                match rx.recv().await {
                    Some(msg @ Message::OrderUpdates(_)) => Some((msg, rx)),
                    Some(_) => None,
                    None => None,
                }
            })
        }))
    }

    pub async fn subscribe_user_events(
        &self,
        user: H160,
    ) -> std::result::Result<impl Stream<Item = Message>, anyhow::Error> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut info_client = self.info_client.lock().await;
        info_client
            .subscribe(Subscription::UserEvents { user }, tx)
            .await?;

        Ok(futures_util::stream::unfold(rx, |mut rx| {
            Box::pin(async move {
                match rx.recv().await {
                    Some(msg @ Message::User(_)) => Some((msg, rx)),
                    Some(_) => None,
                    None => None,
                }
            })
        }))
    }

    pub async fn subscribe_user_fills(
        &self,
        user: H160,
    ) -> std::result::Result<impl Stream<Item = Message>, anyhow::Error> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut info_client = self.info_client.lock().await;
        info_client
            .subscribe(Subscription::UserFills { user }, tx)
            .await?;

        Ok(futures_util::stream::unfold(rx, |mut rx| {
            Box::pin(async move {
                match rx.recv().await {
                    Some(msg @ Message::UserFills(_)) => Some((msg, rx)),
                    Some(_) => None,
                    None => None,
                }
            })
        }))
    }

    pub async fn subscribe_user_fundings(
        &self,
        user: H160,
    ) -> std::result::Result<impl Stream<Item = Message>, anyhow::Error> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut info_client = self.info_client.lock().await;
        info_client
            .subscribe(Subscription::UserFundings { user }, tx)
            .await?;

        Ok(futures_util::stream::unfold(rx, |mut rx| {
            Box::pin(async move {
                match rx.recv().await {
                    Some(msg @ Message::UserFundings(_)) => Some((msg, rx)),
                    Some(_) => None,
                    None => None,
                }
            })
        }))
    }

    pub async fn subscribe_user_non_funding_ledger_updates(
        &self,
        user: H160,
    ) -> std::result::Result<impl Stream<Item = Message>, anyhow::Error> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut info_client = self.info_client.lock().await;
        info_client
            .subscribe(Subscription::UserNonFundingLedgerUpdates { user }, tx)
            .await?;

        Ok(futures_util::stream::unfold(rx, |mut rx| {
            Box::pin(async move {
                match rx.recv().await {
                    Some(msg @ Message::UserNonFundingLedgerUpdates(_)) => Some((msg, rx)),
                    Some(_) => None,
                    None => None,
                }
            })
        }))
    }

    pub async fn subscribe_active_asset_ctx(
        &self,
        coin: String,
    ) -> std::result::Result<impl Stream<Item = Message>, anyhow::Error> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut info_client = self.info_client.lock().await;
        info_client
            .subscribe(Subscription::ActiveAssetCtx { coin }, tx)
            .await?;

        Ok(futures_util::stream::unfold(rx, |mut rx| {
            Box::pin(async move {
                match rx.recv().await {
                    Some(msg @ Message::ActiveAssetCtx(_)) => Some((msg, rx)),
                    Some(_) => None,
                    None => None,
                }
            })
        }))
    }

    pub async fn cancel_all_orders(&self, asset: String, is_strict: bool) -> TaskResult {
        info!("Cancelling all open orders for asset: {}", asset);

        let wallet_address = self
            .connection_config
            .wallet_address
            .expect("Wallet address not found");

        let open_orders = {
            let info_client = self.info_client.lock().await;
            match info_client.open_orders(wallet_address).await {
                Ok(orders) => orders,
                Err(e) => {
                    error!("Failed to fetch open orders: {}", e);
                    return TaskResult::Failure(format!("Failed to fetch open orders: {}", e));
                }
            }
        };

        debug!("Found {} open orders in total", open_orders.len());

        let asset_orders: Vec<_> = open_orders
            .into_iter()
            .filter(|order| order.coin == asset)
            .collect();

        debug!(
            "Found {} open orders for asset {}",
            asset_orders.len(),
            asset
        );

        if asset_orders.is_empty() {
            info!("No open orders found for asset: {}", asset);
            return TaskResult::Success;
        }

        let exchange_client = self.exchange_client.lock().await;

        let cancel_requests: Vec<ClientCancelRequest> = asset_orders
            .into_iter()
            .map(|order| ClientCancelRequest {
                asset: asset.clone(),
                oid: order.oid,
            })
            .collect();

        debug!(
            "Sending bulk cancel request for {} orders",
            cancel_requests.len()
        );

        match exchange_client.bulk_cancel(cancel_requests, None).await {
            Ok(_) => {
                info!("Successfully cancelled all orders for asset {}", asset);
                TaskResult::Success
            }
            Err(e) => {
                let error_msg = format!("Failed to cancel orders for asset {}: {}", asset, e);
                error!("{}", error_msg);
                TaskResult::Failure(error_msg)
            }
        }
    }

    pub fn cancel_all_orders_non_blocking(&self, asset: String) {
        let exchange_client = self.exchange_client.clone();
        let info_client = self.info_client.clone();
        let wallet_address = self.connection_config.wallet_address;
        let asset = asset.clone();

        tokio::spawn(async move {
            let wallet_address = match wallet_address {
                Some(addr) => addr,
                None => {
                    error!("Wallet address not found for non-blocking cancel");
                    return;
                }
            };

            let open_orders = {
                let info_client = info_client.lock().await;
                match info_client.open_orders(wallet_address).await {
                    Ok(orders) => orders,
                    Err(e) => {
                        error!("Failed to fetch open orders for non-blocking cancel: {}", e);
                        return;
                    }
                }
            };

            let asset_orders: Vec<_> = open_orders
                .into_iter()
                .filter(|order| order.coin == asset)
                .collect();

            if asset_orders.is_empty() {
                return;
            }

            let cancel_requests: Vec<ClientCancelRequest> = asset_orders
                .into_iter()
                .map(|order| ClientCancelRequest {
                    asset: asset.clone(),
                    oid: order.oid,
                })
                .collect();

            if let Ok(client) = exchange_client.try_lock() {
                let _ = client.bulk_cancel(cancel_requests, None).await;
            }
        });
    }

    pub async fn bulk_place_orders(
        &self,
        requests: Vec<ClientOrderRequest>,
        is_strict: bool,
    ) -> TaskResult {
        if requests.is_empty() {
            return TaskResult::Success;
        }

        let task = Task::BulkOrder(requests);
        let policy = TaskPolicy {
            is_strict,
            ..Default::default()
        };
        self.submit_task(task, policy).await
    }

    pub async fn bulk_cancel_orders(
        &self,
        asset: String,
        cloids: Vec<Uuid>,
        is_strict: bool,
    ) -> TaskResult {
        if cloids.is_empty() {
            return TaskResult::Success;
        }

        info!(
            "Attempting to cancel orders by client IDs for asset: {}",
            asset
        );

        let cancel_result = self.cancel_all_orders(asset, is_strict).await;

        cancel_result
    }
}

async fn reconnect(
    exchange_client: &Arc<Mutex<ExchangeClient>>,
    info_client: &Arc<Mutex<InfoClient>>,
    config: &ConnectionConfig,
) -> Result<()> {
    let new_info_client = InfoClient::new(None, config.base_url.clone())?;

    let meta = new_info_client.meta().await?;

    let new_exchange_client = ExchangeClient::new(
        None,
        config.wallet.clone(),
        config.base_url.clone(),
        Some(meta),
        config.vault_address.clone(),
    )
    .await?;

    {
        let mut info_client_lock = info_client.lock().await;
        *info_client_lock = new_info_client;
    }

    {
        let mut exchange_client_lock = exchange_client.lock().await;
        *exchange_client_lock = new_exchange_client;
    }

    Ok(())
}

struct TaskDelegate<'a> {
    task: &'a Task,
    exchange_client: Arc<Mutex<ExchangeClient>>,
}

impl<'a> TaskDelegate<'a> {
    fn new(task: &'a Task, exchange_client: Arc<Mutex<ExchangeClient>>) -> Self {
        Self {
            task,
            exchange_client,
        }
    }

    async fn process(&self) -> Result<ExchangeResponseStatus> {
        match self.task {
            Task::Cancel(req) => {
                let client = self.exchange_client.lock().await;
                client.cancel(req.clone(), None).await
            }
            Task::CancelCloid(req) => {
                let client = self.exchange_client.lock().await;
                client.cancel_by_cloid(req.clone(), None).await
            }
            Task::Modify(req) => {
                let client = self.exchange_client.lock().await;
                client.modify(req.clone(), None).await
            }
            Task::Order(req) => {
                let client = self.exchange_client.lock().await;
                client.order(req.clone(), None).await
            }
            Task::BulkOrder(reqs) => {
                let client = self.exchange_client.lock().await;
                client.bulk_order(reqs.to_vec(), None).await
            }
            Task::BulkCancel(reqs) => {
                let client = self.exchange_client.lock().await;
                client.bulk_cancel(reqs.to_vec(), None).await
            }
        }
    }
}

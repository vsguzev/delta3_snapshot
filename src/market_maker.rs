use std::{future::Future, pin::Pin, time::Duration};

use ethers::{
    abi::Address,
    signers::{LocalWallet, Signer},
    types::H160,
};
use log::{error, info};
use uuid::Uuid;

use tokio::{sync::mpsc::unbounded_channel, time::timeout};

use hyperliquid_rust_sdk::{
    bps_diff, truncate_float, AssetPosition, BaseUrl, BulkCancelCloid, ClientCancelRequest,
    ClientCancelRequestCloid, ClientLimit, ClientOrder, ClientOrderRequest, ExchangeClient,
    ExchangeDataStatus, ExchangeResponseStatus, InfoClient, Message, Subscription, UserData,
    EPSILON,
};
#[derive(Debug)]
pub struct MarketMakerRestingOrder {
    pub oid: u64,
    pub position: f64,
    pub price: f64,
}

#[derive(Clone, Debug)]
pub struct MarketMakerInput {
    pub asset: String,
    pub target_liquidity: f64,

    pub half_spread: u16,

    pub max_absolute_position_size: f64,

    pub decimals: u32,

    pub wallet: LocalWallet,
}

pub struct MarketMaker {
    pub asset: String,
    pub target_liquidity: f64,
    pub half_spread: u16,
    pub max_absolute_position_size: f64,
    pub decimals: u32,
    pub lower_resting: MarketMakerRestingOrder,
    pub upper_resting: MarketMakerRestingOrder,
    pub cur_position: f64,
    pub latest_mid_price: f64,
    pub info_client: InfoClient,
    pub exchange_client: ExchangeClient,
    pub user_address: H160,
}

impl MarketMaker {
    pub async fn new(input: MarketMakerInput, user_address: Address) -> MarketMaker {
        let info_client = InfoClient::with_reconnect(None, Some(BaseUrl::Mainnet))
            .await
            .unwrap();
        let exchange_client =
            ExchangeClient::new(None, input.wallet, Some(BaseUrl::Mainnet), None, None)
                .await
                .unwrap();

        let cur_positions = info_client
            .user_state(user_address)
            .await
            .unwrap()
            .asset_positions
            .into_iter()
            .filter(|entry| entry.position.coin == input.asset)
            .collect::<Vec<AssetPosition>>();
        let cur_position = if cur_positions.len() > 0 {
            cur_positions[0]
                .position
                .szi
                .clone()
                .parse::<f64>()
                .unwrap()
        } else {
            0.0
        };

        MarketMaker {
            asset: input.asset,
            target_liquidity: input.target_liquidity,
            half_spread: input.half_spread,
            max_absolute_position_size: input.max_absolute_position_size,
            decimals: input.decimals,
            lower_resting: MarketMakerRestingOrder {
                oid: 0,
                position: 0.0,
                price: -1.0,
            },
            upper_resting: MarketMakerRestingOrder {
                oid: 0,
                position: 0.0,
                price: -1.0,
            },
            cur_position,
            latest_mid_price: -1.0,
            info_client,
            exchange_client,
            user_address,
        }
    }

    pub async fn start(&mut self) {
        let (sender, mut receiver) = unbounded_channel();

        println!("Trying to cancel previous orders...");
        let result = self
            .exchange_client
            .bulk_cancel_by_cloid(
                vec![
                    ClientCancelRequestCloid {
                        asset: self.asset.clone(),
                        cloid: Uuid::from_u128(1),
                    },
                    ClientCancelRequestCloid {
                        asset: self.asset.clone(),
                        cloid: Uuid::from_u128(2),
                    },
                ],
                None,
            )
            .await;
        println!("{result:?}");

        let user_events = self
            .info_client
            .subscribe(
                Subscription::UserEvents {
                    user: self.user_address,
                },
                sender.clone(),
            )
            .await
            .unwrap();

        let all_mids = self
            .info_client
            .subscribe(Subscription::AllMids, sender)
            .await
            .unwrap();

        loop {
            tokio::select! {
                maybe_message = receiver.recv() => {
                    match maybe_message {
                        Some(message) => {


                            match message {
                                Message::AllMids(all_mids) => {
                                    let all_mids = all_mids.data.mids;
                                    if let Some(mid) = all_mids.get(&self.asset) {
                                        let mid: f64 = mid.parse().unwrap();
                                        self.latest_mid_price = mid;


                                        self.potentially_update().await;
                                    } else {
                                        error!("could not get mid for asset {}: {all_mids:?}", self.asset.clone());
                                    }
                                }
                                Message::User(user_events) => {
                                    info!("{user_events:?}");


                                    if self.latest_mid_price < 0.0 {
                                        continue;
                                    }
                                    let user_events = user_events.data;
                                    if let UserData::Fills(fills) = user_events {
                                        let mut last_ts: u64 = 0;
                                        for fill in fills {
                                            let amount: f64 = fill.sz.parse().unwrap();
                                            if last_ts < fill.time {
                                                last_ts = fill.time;
                                                self.latest_mid_price = fill.px.parse().unwrap();
                                            }


                                            if fill.side.eq("B") {
                                                self.cur_position += amount;
                                                self.lower_resting.position -= amount;
                                                info!("Fill: bought {amount} {}", self.asset.clone());
                                            } else {
                                                self.cur_position -= amount;
                                                self.upper_resting.position -= amount;
                                                info!("Fill: sold {amount} {}", self.asset.clone());
                                            }
                                        }
                                    }


                                    self.potentially_update().await;
                                }
                                message => {
                                    info!("Unsupported message type: {message:?}");
                                }
                            }
                        }
                        None => {


                            println!("Channel closed.");
                            break;
                        }
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(10)) => {


                    println!("No message received in 10 seconds, exiting loop.");
                    break;
                }
            }
        }
        self.info_client.unsubscribe(user_events).await;
        self.info_client.unsubscribe(all_mids).await;
    }

    async fn attempt_cancel(&self, asset: String, oid: u64) -> bool {
        let cancel = self
            .exchange_client
            .cancel(ClientCancelRequest { asset, oid }, None)
            .await;

        match cancel {
            Ok(cancel) => match cancel {
                ExchangeResponseStatus::Ok(cancel) => {
                    if let Some(cancel) = cancel.data {
                        if !cancel.statuses.is_empty() {
                            match cancel.statuses[0].clone() {
                                ExchangeDataStatus::Success => {
                                    return true;
                                }
                                ExchangeDataStatus::Error(e) => {
                                    error!("Error with cancelling: {e}")
                                }
                                _ => unreachable!(),
                            }
                        } else {
                            error!("Exchange data statuses is empty when cancelling: {cancel:?}")
                        }
                    } else {
                        error!("Exchange response data is empty when cancelling: {cancel:?}")
                    }
                }
                ExchangeResponseStatus::Err(e) => error!("Error with cancelling: {e}"),
            },
            Err(e) => error!("Error with cancelling: {e}"),
        }
        false
    }

    async fn place_order(
        &self,
        asset: String,
        amount: f64,
        price: f64,
        is_buy: bool,
    ) -> (f64, u64) {
        let order = self
            .exchange_client
            .order(
                ClientOrderRequest {
                    asset,
                    is_buy,
                    reduce_only: false,
                    limit_px: price,
                    sz: amount,
                    cloid: Some(Uuid::from_u128(if is_buy { 1 } else { 2 })),
                    order_type: ClientOrder::Limit(ClientLimit {
                        tif: "Gtc".to_string(),
                    }),
                },
                None,
            )
            .await;
        match order {
            Ok(order) => match order {
                ExchangeResponseStatus::Ok(order) => {
                    if let Some(order) = order.data {
                        if !order.statuses.is_empty() {
                            match order.statuses[0].clone() {
                                ExchangeDataStatus::Filled(order) => {
                                    return (amount, order.oid);
                                }
                                ExchangeDataStatus::Resting(order) => {
                                    return (amount, order.oid);
                                }
                                ExchangeDataStatus::Error(e) => {
                                    error!("Error with placing order: {e}")
                                }
                                _ => unreachable!(),
                            }
                        } else {
                            error!("Exchange data statuses is empty when placing order: {order:?}")
                        }
                    } else {
                        error!("Exchange response data is empty when placing order: {order:?}")
                    }
                }
                ExchangeResponseStatus::Err(e) => {
                    error!("Error with placing order: {e}")
                }
            },
            Err(e) => error!("Error with placing order: {e}"),
        }
        (0.0, 0)
    }

    async fn potentially_update(&mut self) {
        let buy_spread = self.latest_mid_price
            * (1.
                - 1. / (1.
                    + ((self.half_spread as f64) / 10000.0
                        * (1.
                            / (1.
                                - self.cur_position
                                    / (self.max_absolute_position_size
                                        / self.latest_mid_price
                                        * 1.)))
                            .max(1.))));
        let sell_spread = self.latest_mid_price
            * ((self.half_spread as f64) / 10000.0
                * (1.
                    / (1.
                        + self.cur_position
                            / (self.max_absolute_position_size / self.latest_mid_price * 1.)))
                    .max(1.));

        let (lower_price, upper_price) = (
            self.latest_mid_price - buy_spread,
            self.latest_mid_price + sell_spread,
        );
        let (mut lower_price, mut upper_price) = (
            truncate_float(lower_price, self.decimals, true),
            truncate_float(upper_price, self.decimals, false),
        );

        if (lower_price - upper_price).abs() < EPSILON {
            lower_price = truncate_float(lower_price, self.decimals, false);
            upper_price = truncate_float(upper_price, self.decimals, true);
        }

        let lower_order_amount = truncate_float(
            (self.max_absolute_position_size / lower_price - self.cur_position)
                .min(self.target_liquidity / lower_price)
                .max(0.0),
            2,
            true,
        );

        let upper_order_amount = truncate_float(
            (self.max_absolute_position_size / lower_price + self.cur_position)
                .min(self.target_liquidity / lower_price)
                .max(0.0),
            2,
            true,
        );

        let lower_change = self.lower_resting.position.abs() < 0.009;
        let upper_change = self.upper_resting.position.abs() < 0.009;

        let change = lower_change || upper_change;

        use futures::future::ready;
        use std::future::Future;
        use std::pin::Pin;

        let cancel_lower_future: Pin<Box<dyn Future<Output = bool> + Send>> =
            if self.lower_resting.oid != 0 && upper_change {
                Box::pin(self.attempt_cancel(self.asset.clone(), self.lower_resting.oid))
            } else {
                Box::pin(ready(false))
            };

        let cancel_upper_future: Pin<Box<dyn Future<Output = bool> + Send>> =
            if self.upper_resting.oid != 0 && lower_change {
                Box::pin(self.attempt_cancel(self.asset.clone(), self.upper_resting.oid))
            } else {
                Box::pin(ready(false))
            };

        let lower_order_future: Pin<Box<dyn Future<Output = (f64, u64)> + Send>> =
            if lower_order_amount > EPSILON && change {
                Box::pin(self.place_order(
                    self.asset.clone(),
                    lower_order_amount,
                    lower_price,
                    true,
                ))
            } else {
                Box::pin(ready((0.0, 0)))
            };

        let upper_order_future: Pin<Box<dyn Future<Output = (f64, u64)> + Send>> =
            if upper_order_amount > EPSILON && change {
                Box::pin(self.place_order(
                    self.asset.clone(),
                    upper_order_amount,
                    upper_price,
                    false,
                ))
            } else {
                Box::pin(ready((0.0, 0)))
            };

        let (cancel_lower_result, cancel_upper_result, lower_order_result, upper_order_result) = tokio::join!(
            cancel_lower_future,
            cancel_upper_future,
            lower_order_future,
            upper_order_future
        );

        if lower_order_amount > EPSILON && change {
            let (amount_resting, oid) = lower_order_result;
            self.lower_resting.oid = oid;
            self.lower_resting.position = amount_resting;
            self.lower_resting.price = lower_price;

            info!("Current position: {:.2}", self.cur_position);
            if amount_resting > EPSILON {
                info!(
                    "Buy for {amount_resting} {} resting at {lower_price} with spread {buy_spread:.3}",
                    self.asset.clone()
                );
            }
        }

        if upper_order_amount > EPSILON && change {
            let (amount_resting, oid) = upper_order_result;
            self.upper_resting.oid = oid;
            self.upper_resting.position = amount_resting;
            self.upper_resting.price = upper_price;

            info!("Current position: {:.2}", self.cur_position);
            if amount_resting > EPSILON {
                info!(
                    "Sell for {amount_resting} {} resting at {upper_price} with spread {sell_spread:.3}",
                    self.asset.clone()
                );
            }
        }

        if self.lower_resting.oid != 0 && upper_change {
            info!("Cancelled buy order: {:?}", self.lower_resting);
        }
        if self.upper_resting.oid != 0 && lower_change {
            info!("Cancelled sell order: {:?}", self.upper_resting);
        }
    }
}

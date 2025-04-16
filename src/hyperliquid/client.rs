use crate::hyperliquid::{
    errors::*, helpers::*, req::HttpClient, sign::*, types::*, Message, Subscription, WsManager,
};
use ethers::{
    abi::AbiEncode,
    signers::{LocalWallet, Signer},
    types::{Signature, H160, H256},
};
use log::debug;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::mpsc::UnboundedSender;





#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CandleSnapshotRequest {
    coin: String,
    interval: String,
    start_time: u64,
    end_time: u64,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(tag = "type")]
#[serde(rename_all = "camelCase")]
pub enum InfoRequest {
    #[serde(rename = "clearinghouseState")]
    UserState {
        user: H160,
    },
    #[serde(rename = "batchClearinghouseStates")]
    UserStates {
        users: Vec<H160>,
    },
    #[serde(rename = "spotClearinghouseState")]
    UserTokenBalances {
        user: H160,
    },
    UserFees {
        user: H160,
    },
    OpenOrders {
        user: H160,
    },
    OrderStatus {
        user: H160,
        oid: u64,
    },
    Meta,
    SpotMeta,
    SpotMetaAndAssetCtxs,
    AllMids,
    UserFills {
        user: H160,
    },
    #[serde(rename_all = "camelCase")]
    FundingHistory {
        coin: String,
        start_time: u64,
        end_time: Option<u64>,
    },
    #[serde(rename_all = "camelCase")]
    UserFunding {
        user: H160,
        start_time: u64,
        end_time: Option<u64>,
    },
    L2Book {
        coin: String,
    },
    RecentTrades {
        coin: String,
    },
    #[serde(rename_all = "camelCase")]
    CandleSnapshot {
        req: CandleSnapshotRequest,
    },
    Referral {
        user: H160,
    },
    HistoricalOrders {
        user: H160,
    },
}

#[derive(Debug)]
pub struct InfoClient {
    pub http_client: HttpClient,
    pub(crate) ws_manager: Option<WsManager>,
    reconnect: bool,
}

impl InfoClient {
    pub fn new(client: Option<Client>, base_url: Option<BaseUrl>) -> Result<InfoClient> {
        Self::new_internal(client, base_url, false)
    }

    pub fn with_reconnect(client: Option<Client>, base_url: Option<BaseUrl>) -> Result<InfoClient> {
        Self::new_internal(client, base_url, true)
    }

    fn new_internal(
        client: Option<Client>,
        base_url: Option<BaseUrl>,
        reconnect: bool,
    ) -> Result<InfoClient> {
        let client = client.unwrap_or_default();
        let base_url = base_url.unwrap_or(BaseUrl::Mainnet).get_url();

        Ok(InfoClient {
            http_client: HttpClient { client, base_url },
            ws_manager: None,
            reconnect,
        })
    }

    pub async fn subscribe(
        &mut self,
        subscription: Subscription,
        sender_channel: UnboundedSender<Message>,
    ) -> Result<u32> {
        if self.ws_manager.is_none() {
            let ws_manager = WsManager::new(
                format!("ws{}/ws", &self.http_client.base_url[4..]),
                self.reconnect,
            )
            .await?;
            self.ws_manager = Some(ws_manager);
        }

        let identifier =
            serde_json::to_string(&subscription).map_err(|e| Error::JsonParse(e.to_string()))?;

        self.ws_manager
            .as_mut()
            .ok_or(Error::WsManagerNotFound)?
            .add_subscription(identifier, sender_channel)
            .await
    }

    pub async fn unsubscribe(&mut self, subscription_id: u32) -> Result<()> {
        if self.ws_manager.is_none() {
            let ws_manager = WsManager::new(
                format!("ws{}/ws", &self.http_client.base_url[4..]),
                self.reconnect,
            )
            .await?;
            self.ws_manager = Some(ws_manager);
        }

        self.ws_manager
            .as_mut()
            .ok_or(Error::WsManagerNotFound)?
            .remove_subscription(subscription_id)
            .await
    }

    async fn send_info_request<T: for<'a> Deserialize<'a>>(
        &self,
        info_request: InfoRequest,
    ) -> Result<T> {
        let data =
            serde_json::to_string(&info_request).map_err(|e| Error::JsonParse(e.to_string()))?;

        let return_data = self.http_client.post("/info", data).await?;
        serde_json::from_str(&return_data).map_err(|e| Error::JsonParse(e.to_string()))
    }

    pub async fn open_orders(&self, address: H160) -> Result<Vec<OpenOrdersResponse>> {
        let input = InfoRequest::OpenOrders { user: address };
        self.send_info_request(input).await
    }

    pub async fn user_state(&self, address: H160) -> Result<UserStateResponse> {
        let input = InfoRequest::UserState { user: address };
        self.send_info_request(input).await
    }

    pub async fn user_states(&self, addresses: Vec<H160>) -> Result<Vec<UserStateResponse>> {
        let input = InfoRequest::UserStates { users: addresses };
        self.send_info_request(input).await
    }

    pub async fn user_token_balances(&self, address: H160) -> Result<UserTokenBalanceResponse> {
        let input = InfoRequest::UserTokenBalances { user: address };
        self.send_info_request(input).await
    }

    pub async fn user_fees(&self, address: H160) -> Result<UserFeesResponse> {
        let input = InfoRequest::UserFees { user: address };
        self.send_info_request(input).await
    }

    pub async fn meta(&self) -> Result<Meta> {
        let input = InfoRequest::Meta;
        self.send_info_request(input).await
    }

    pub async fn spot_meta(&self) -> Result<SpotMeta> {
        let input = InfoRequest::SpotMeta;
        self.send_info_request(input).await
    }

    pub async fn spot_meta_and_asset_contexts(&self) -> Result<Vec<SpotMetaAndAssetCtxs>> {
        let input = InfoRequest::SpotMetaAndAssetCtxs;
        self.send_info_request(input).await
    }

    pub async fn all_mids(&self) -> Result<HashMap<String, String>> {
        let input = InfoRequest::AllMids;
        self.send_info_request(input).await
    }

    pub async fn user_fills(&self, address: H160) -> Result<Vec<UserFillsResponse>> {
        let input = InfoRequest::UserFills { user: address };
        self.send_info_request(input).await
    }

    pub async fn funding_history(
        &self,
        coin: String,
        start_time: u64,
        end_time: Option<u64>,
    ) -> Result<Vec<FundingHistoryResponse>> {
        let input = InfoRequest::FundingHistory {
            coin,
            start_time,
            end_time,
        };
        self.send_info_request(input).await
    }

    pub async fn user_funding_history(
        &self,
        user: H160,
        start_time: u64,
        end_time: Option<u64>,
    ) -> Result<Vec<UserFundingResponse>> {
        let input = InfoRequest::UserFunding {
            user,
            start_time,
            end_time,
        };
        self.send_info_request(input).await
    }

    pub async fn recent_trades(&self, coin: String) -> Result<Vec<RecentTradesResponse>> {
        let input = InfoRequest::RecentTrades { coin };
        self.send_info_request(input).await
    }

    pub async fn l2_snapshot(&self, coin: String) -> Result<L2SnapshotResponse> {
        let input = InfoRequest::L2Book { coin };
        self.send_info_request(input).await
    }

    pub async fn candles_snapshot(
        &self,
        coin: String,
        interval: String,
        start_time: u64,
        end_time: u64,
    ) -> Result<Vec<CandlesSnapshotResponse>> {
        let input = InfoRequest::CandleSnapshot {
            req: CandleSnapshotRequest {
                coin,
                interval,
                start_time,
                end_time,
            },
        };
        self.send_info_request(input).await
    }

    pub async fn query_order_by_oid(&self, address: H160, oid: u64) -> Result<OrderStatusResponse> {
        let input = InfoRequest::OrderStatus { user: address, oid };
        self.send_info_request(input).await
    }

    pub async fn query_referral_state(&self, address: H160) -> Result<ReferralResponse> {
        let input = InfoRequest::Referral { user: address };
        self.send_info_request(input).await
    }

    pub async fn historical_orders(&self, address: H160) -> Result<Vec<OrderInfo>> {
        let input = InfoRequest::HistoricalOrders { user: address };
        self.send_info_request(input).await
    }
}





#[derive(Debug)]
pub struct ExchangeClient {
    pub http_client: HttpClient,
    pub wallet: LocalWallet,
    pub meta: Meta,
    pub vault_address: Option<H160>,
    pub coin_to_asset: HashMap<String, u32>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExchangePayload {
    action: serde_json::Value,
    signature: Signature,
    nonce: u64,
    vault_address: Option<H160>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
#[serde(rename_all = "camelCase")]
pub enum Actions {
    UsdSend(UsdSend),
    UpdateLeverage(UpdateLeverage),
    UpdateIsolatedMargin(UpdateIsolatedMargin),
    Order(BulkOrder),
    Cancel(BulkCancel),
    CancelByCloid(BulkCancelCloid),
    BatchModify(BulkModify),
    ApproveAgent(ApproveAgent),
    Withdraw3(Withdraw3),
    SpotUser(SpotUser),
    VaultTransfer(VaultTransfer),
    SpotSend(SpotSend),
    SetReferrer(SetReferrer),
    ApproveBuilderFee(ApproveBuilderFee),
}

impl Actions {
    fn hash(&self, timestamp: u64, vault_address: Option<H160>) -> Result<H256> {
        let mut bytes =
            rmp_serde::to_vec_named(self).map_err(|e| Error::RmpParse(e.to_string()))?;
        bytes.extend(timestamp.to_be_bytes());
        if let Some(vault_address) = vault_address {
            bytes.push(1);
            bytes.extend(vault_address.to_fixed_bytes());
        } else {
            bytes.push(0);
        }
        Ok(H256(ethers::utils::keccak256(bytes)))
    }
}

impl ExchangeClient {
    pub async fn new(
        client: Option<Client>,
        wallet: LocalWallet,
        base_url: Option<BaseUrl>,
        meta: Option<Meta>,
        vault_address: Option<H160>,
    ) -> Result<ExchangeClient> {
        let client = client.unwrap_or_default();
        let base_url = base_url.unwrap_or(BaseUrl::Mainnet);

        let info = InfoClient::new(None, Some(base_url)).unwrap();
        let meta = if let Some(meta) = meta {
            meta
        } else {
            info.meta().await?
        };

        let mut coin_to_asset = HashMap::new();
        for (asset_ind, asset) in meta.universe.iter().enumerate() {
            coin_to_asset.insert(asset.name.clone(), asset_ind as u32);
        }

        coin_to_asset = info
            .spot_meta()
            .await?
            .add_pair_and_name_to_index_map(coin_to_asset);

        Ok(ExchangeClient {
            wallet,
            meta,
            vault_address,
            http_client: HttpClient {
                client,
                base_url: base_url.get_url(),
            },
            coin_to_asset,
        })
    }

    async fn post(
        &self,
        action: serde_json::Value,
        signature: Signature,
        nonce: u64,
    ) -> Result<ExchangeResponseStatus> {
        let exchange_payload = ExchangePayload {
            action,
            signature,
            nonce,
            vault_address: self.vault_address,
        };
        let res = serde_json::to_string(&exchange_payload)
            .map_err(|e| Error::JsonParse(e.to_string()))?;
        debug!("Sending request {res:?}");

        let output = &self
            .http_client
            .post("/exchange", res)
            .await
            .map_err(|e| Error::JsonParse(e.to_string()))?;
        serde_json::from_str(output).map_err(|e| Error::JsonParse(e.to_string()))
    }

    pub async fn usdc_transfer(
        &self,
        amount: &str,
        destination: &str,
        wallet: Option<&LocalWallet>,
    ) -> Result<ExchangeResponseStatus> {
        let wallet = wallet.unwrap_or(&self.wallet);
        let hyperliquid_chain = if self.http_client.is_mainnet() {
            "Mainnet".to_string()
        } else {
            "Testnet".to_string()
        };

        let timestamp = next_nonce();
        let usd_send = UsdSend {
            signature_chain_id: 421614.into(),
            hyperliquid_chain,
            destination: destination.to_string(),
            amount: amount.to_string(),
            time: timestamp,
        };
        let signature = sign_typed_data(&usd_send, wallet)?;
        let action = serde_json::to_value(Actions::UsdSend(usd_send))
            .map_err(|e| Error::JsonParse(e.to_string()))?;

        self.post(action, signature, timestamp).await
    }

    pub async fn class_transfer(
        &self,
        usdc: f64,
        to_perp: bool,
        wallet: Option<&LocalWallet>,
    ) -> Result<ExchangeResponseStatus> {
        
        let usdc = (usdc * 1e6).round() as u64;
        let wallet = wallet.unwrap_or(&self.wallet);

        let timestamp = next_nonce();

        let action = Actions::SpotUser(SpotUser {
            class_transfer: ClassTransfer { usdc, to_perp },
        });
        let connection_id = action.hash(timestamp, self.vault_address)?;
        let action = serde_json::to_value(&action).map_err(|e| Error::JsonParse(e.to_string()))?;
        let is_mainnet = self.http_client.is_mainnet();
        let signature = sign_l1_action(wallet, connection_id, is_mainnet)?;

        self.post(action, signature, timestamp).await
    }

    pub async fn vault_transfer(
        &self,
        is_deposit: bool,
        usd: String,
        vault_address: Option<H160>,
        wallet: Option<&LocalWallet>,
    ) -> Result<ExchangeResponseStatus> {
        let vault_address = self
            .vault_address
            .or(vault_address)
            .ok_or(Error::VaultAddressNotFound)?;
        let wallet = wallet.unwrap_or(&self.wallet);

        let timestamp = next_nonce();

        let action = Actions::VaultTransfer(VaultTransfer {
            vault_address,
            is_deposit,
            usd,
        });
        let connection_id = action.hash(timestamp, self.vault_address)?;
        let action = serde_json::to_value(&action).map_err(|e| Error::JsonParse(e.to_string()))?;
        let is_mainnet = self.http_client.is_mainnet();
        let signature = sign_l1_action(wallet, connection_id, is_mainnet)?;

        self.post(action, signature, timestamp).await
    }

    pub async fn market_open(
        &self,
        params: MarketOrderParams<'_>,
    ) -> Result<ExchangeResponseStatus> {
        let slippage = params.slippage.unwrap_or(0.05); 
        let (px, sz_decimals) = self
            .calculate_slippage_price(params.asset, params.is_buy, slippage, params.px)
            .await?;

        let order = ClientOrderRequest {
            asset: params.asset.to_string(),
            is_buy: params.is_buy,
            reduce_only: false,
            limit_px: px,
            sz: round_to_decimals(params.sz, sz_decimals),
            cloid: params.cloid,
            order_type: ClientOrder::Limit(ClientLimit {
                tif: "Ioc".to_string(),
            }),
        };

        self.order(order, params.wallet).await
    }

    pub async fn market_open_with_builder(
        &self,
        params: MarketOrderParams<'_>,
        builder: BuilderInfo,
    ) -> Result<ExchangeResponseStatus> {
        let slippage = params.slippage.unwrap_or(0.05); 
        let (px, sz_decimals) = self
            .calculate_slippage_price(params.asset, params.is_buy, slippage, params.px)
            .await?;

        let order = ClientOrderRequest {
            asset: params.asset.to_string(),
            is_buy: params.is_buy,
            reduce_only: false,
            limit_px: px,
            sz: round_to_decimals(params.sz, sz_decimals),
            cloid: params.cloid,
            order_type: ClientOrder::Limit(ClientLimit {
                tif: "Ioc".to_string(),
            }),
        };

        self.order_with_builder(order, params.wallet, builder).await
    }

    pub async fn market_close(
        &self,
        params: MarketCloseParams<'_>,
    ) -> Result<ExchangeResponseStatus> {
        let slippage = params.slippage.unwrap_or(0.05); 
        let wallet = params.wallet.unwrap_or(&self.wallet);

        let base_url = match self.http_client.base_url.as_str() {
            "https://api.hyperliquid.xyz" => BaseUrl::Mainnet,
            "https://api.hyperliquid-testnet.xyz" => BaseUrl::Testnet,
            _ => return Err(Error::GenericRequest("Invalid base URL".to_string())),
        };
        let info_client = InfoClient::new(None, Some(base_url)).unwrap();
        let user_state = info_client.user_state(wallet.address()).await?;

        let position = user_state
            .asset_positions
            .iter()
            .find(|p| p.position.coin == params.asset)
            .ok_or(Error::AssetNotFound)?;

        let szi = position
            .position
            .szi
            .parse::<f64>()
            .map_err(|_| Error::FloatStringParse)?;

        let (px, sz_decimals) = self
            .calculate_slippage_price(params.asset, szi < 0.0, slippage, params.px)
            .await?;

        let sz = round_to_decimals(params.sz.unwrap_or_else(|| szi.abs()), sz_decimals);

        let order = ClientOrderRequest {
            asset: params.asset.to_string(),
            is_buy: szi < 0.0,
            reduce_only: true,
            limit_px: px,
            sz,
            cloid: params.cloid,
            order_type: ClientOrder::Limit(ClientLimit {
                tif: "Ioc".to_string(),
            }),
        };

        self.order(order, Some(wallet)).await
    }

    async fn calculate_slippage_price(
        &self,
        asset: &str,
        is_buy: bool,
        slippage: f64,
        px: Option<f64>,
    ) -> Result<(f64, u32)> {
        let base_url = match self.http_client.base_url.as_str() {
            "https://api.hyperliquid.xyz" => BaseUrl::Mainnet,
            "https://api.hyperliquid-testnet.xyz" => BaseUrl::Testnet,
            _ => return Err(Error::GenericRequest("Invalid base URL".to_string())),
        };
        let info_client = InfoClient::new(None, Some(base_url)).unwrap();
        let meta = info_client.meta().await?;

        let asset_meta = meta
            .universe
            .iter()
            .find(|a| a.name == asset)
            .ok_or(Error::AssetNotFound)?;

        let sz_decimals = asset_meta.sz_decimals;
        let max_decimals: u32 = if self.coin_to_asset[asset] < 10000 {
            6
        } else {
            8
        };
        let price_decimals = max_decimals.saturating_sub(sz_decimals);

        let px = if let Some(px) = px {
            px
        } else {
            let all_mids = info_client.all_mids().await?;
            all_mids
                .get(asset)
                .ok_or(Error::AssetNotFound)?
                .parse::<f64>()
                .map_err(|_| Error::FloatStringParse)?
        };

        debug!("px before slippage: {px:?}");
        let slippage_factor = if is_buy {
            1.0 + slippage
        } else {
            1.0 - slippage
        };
        let px = px * slippage_factor;

        
        let px = round_to_significant_and_decimal(px, 5, price_decimals);

        debug!("px after slippage: {px:?}");
        Ok((px, sz_decimals))
    }

    pub async fn order(
        &self,
        order: ClientOrderRequest,
        wallet: Option<&LocalWallet>,
    ) -> Result<ExchangeResponseStatus> {
        self.bulk_order(vec![order], wallet).await
    }

    pub async fn order_with_builder(
        &self,
        order: ClientOrderRequest,
        wallet: Option<&LocalWallet>,
        builder: BuilderInfo,
    ) -> Result<ExchangeResponseStatus> {
        self.bulk_order_with_builder(vec![order], wallet, builder)
            .await
    }

    pub async fn bulk_order(
        &self,
        orders: Vec<ClientOrderRequest>,
        wallet: Option<&LocalWallet>,
    ) -> Result<ExchangeResponseStatus> {
        let wallet = wallet.unwrap_or(&self.wallet);
        let timestamp = next_nonce();

        let mut transformed_orders = Vec::new();

        for order in orders {
            transformed_orders.push(order.convert(&self.coin_to_asset)?);
        }

        let action = Actions::Order(BulkOrder {
            orders: transformed_orders,
            grouping: "na".to_string(),
            builder: None,
        });
        let connection_id = action.hash(timestamp, self.vault_address)?;
        let action = serde_json::to_value(&action).map_err(|e| Error::JsonParse(e.to_string()))?;

        let is_mainnet = self.http_client.is_mainnet();
        let signature = sign_l1_action(wallet, connection_id, is_mainnet)?;
        self.post(action, signature, timestamp).await
    }

    pub async fn bulk_order_with_builder(
        &self,
        orders: Vec<ClientOrderRequest>,
        wallet: Option<&LocalWallet>,
        mut builder: BuilderInfo,
    ) -> Result<ExchangeResponseStatus> {
        let wallet = wallet.unwrap_or(&self.wallet);
        let timestamp = next_nonce();

        builder.builder = builder.builder.to_lowercase();

        let mut transformed_orders = Vec::new();

        for order in orders {
            transformed_orders.push(order.convert(&self.coin_to_asset)?);
        }

        let action = Actions::Order(BulkOrder {
            orders: transformed_orders,
            grouping: "na".to_string(),
            builder: Some(builder),
        });
        let connection_id = action.hash(timestamp, self.vault_address)?;
        let action = serde_json::to_value(&action).map_err(|e| Error::JsonParse(e.to_string()))?;

        let is_mainnet = self.http_client.is_mainnet();
        let signature = sign_l1_action(wallet, connection_id, is_mainnet)?;
        self.post(action, signature, timestamp).await
    }

    pub async fn cancel(
        &self,
        cancel: ClientCancelRequest,
        wallet: Option<&LocalWallet>,
    ) -> Result<ExchangeResponseStatus> {
        self.bulk_cancel(vec![cancel], wallet).await
    }

    pub async fn bulk_cancel(
        &self,
        cancels: Vec<ClientCancelRequest>,
        wallet: Option<&LocalWallet>,
    ) -> Result<ExchangeResponseStatus> {
        let wallet = wallet.unwrap_or(&self.wallet);
        let timestamp = next_nonce();

        let mut transformed_cancels = Vec::new();
        for cancel in cancels.into_iter() {
            let &asset = self
                .coin_to_asset
                .get(&cancel.asset)
                .ok_or(Error::AssetNotFound)?;
            transformed_cancels.push(CancelRequest {
                asset,
                oid: cancel.oid,
            });
        }

        let action = Actions::Cancel(BulkCancel {
            cancels: transformed_cancels,
        });
        let connection_id = action.hash(timestamp, self.vault_address)?;

        let action = serde_json::to_value(&action).map_err(|e| Error::JsonParse(e.to_string()))?;
        let is_mainnet = self.http_client.is_mainnet();
        let signature = sign_l1_action(wallet, connection_id, is_mainnet)?;

        self.post(action, signature, timestamp).await
    }

    pub async fn modify(
        &self,
        modify: ClientModifyRequest,
        wallet: Option<&LocalWallet>,
    ) -> Result<ExchangeResponseStatus> {
        self.bulk_modify(vec![modify], wallet).await
    }

    pub async fn bulk_modify(
        &self,
        modifies: Vec<ClientModifyRequest>,
        wallet: Option<&LocalWallet>,
    ) -> Result<ExchangeResponseStatus> {
        let wallet = wallet.unwrap_or(&self.wallet);
        let timestamp = next_nonce();

        let mut transformed_modifies = Vec::new();
        for modify in modifies.into_iter() {
            transformed_modifies.push(ModifyRequest {
                oid: modify.oid,
                order: modify.order.convert(&self.coin_to_asset)?,
            });
        }

        let action = Actions::BatchModify(BulkModify {
            modifies: transformed_modifies,
        });
        let connection_id = action.hash(timestamp, self.vault_address)?;

        let action = serde_json::to_value(&action).map_err(|e| Error::JsonParse(e.to_string()))?;
        let is_mainnet = self.http_client.is_mainnet();
        let signature = sign_l1_action(wallet, connection_id, is_mainnet)?;

        self.post(action, signature, timestamp).await
    }

    pub async fn cancel_by_cloid(
        &self,
        cancel: ClientCancelRequestCloid,
        wallet: Option<&LocalWallet>,
    ) -> Result<ExchangeResponseStatus> {
        self.bulk_cancel_by_cloid(vec![cancel], wallet).await
    }

    pub async fn bulk_cancel_by_cloid(
        &self,
        cancels: Vec<ClientCancelRequestCloid>,
        wallet: Option<&LocalWallet>,
    ) -> Result<ExchangeResponseStatus> {
        let wallet = wallet.unwrap_or(&self.wallet);
        let timestamp = next_nonce();

        let mut transformed_cancels: Vec<CancelRequestCloid> = Vec::new();
        for cancel in cancels.into_iter() {
            let &asset = self
                .coin_to_asset
                .get(&cancel.asset)
                .ok_or(Error::AssetNotFound)?;
            transformed_cancels.push(CancelRequestCloid {
                asset,
                cloid: uuid_to_hex_string(cancel.cloid),
            });
        }

        let action = Actions::CancelByCloid(BulkCancelCloid {
            cancels: transformed_cancels,
        });

        let connection_id = action.hash(timestamp, self.vault_address)?;
        let action = serde_json::to_value(&action).map_err(|e| Error::JsonParse(e.to_string()))?;
        let is_mainnet = self.http_client.is_mainnet();
        let signature = sign_l1_action(wallet, connection_id, is_mainnet)?;

        self.post(action, signature, timestamp).await
    }

    pub async fn update_leverage(
        &self,
        leverage: u32,
        coin: &str,
        is_cross: bool,
        wallet: Option<&LocalWallet>,
    ) -> Result<ExchangeResponseStatus> {
        let wallet = wallet.unwrap_or(&self.wallet);

        let timestamp = next_nonce();

        let &asset_index = self.coin_to_asset.get(coin).ok_or(Error::AssetNotFound)?;
        let action = Actions::UpdateLeverage(UpdateLeverage {
            asset: asset_index,
            is_cross,
            leverage,
        });
        let connection_id = action.hash(timestamp, self.vault_address)?;
        let action = serde_json::to_value(&action).map_err(|e| Error::JsonParse(e.to_string()))?;
        let is_mainnet = self.http_client.is_mainnet();
        let signature = sign_l1_action(wallet, connection_id, is_mainnet)?;

        self.post(action, signature, timestamp).await
    }

    pub async fn update_isolated_margin(
        &self,
        amount: f64,
        coin: &str,
        wallet: Option<&LocalWallet>,
    ) -> Result<ExchangeResponseStatus> {
        let wallet = wallet.unwrap_or(&self.wallet);

        let amount = (amount * 1_000_000.0).round() as i64;
        let timestamp = next_nonce();

        let &asset_index = self.coin_to_asset.get(coin).ok_or(Error::AssetNotFound)?;
        let action = Actions::UpdateIsolatedMargin(UpdateIsolatedMargin {
            asset: asset_index,
            is_buy: true,
            ntli: amount,
        });
        let connection_id = action.hash(timestamp, self.vault_address)?;
        let action = serde_json::to_value(&action).map_err(|e| Error::JsonParse(e.to_string()))?;
        let is_mainnet = self.http_client.is_mainnet();
        let signature = sign_l1_action(wallet, connection_id, is_mainnet)?;

        self.post(action, signature, timestamp).await
    }

    pub async fn approve_agent(
        &self,
        wallet: Option<&LocalWallet>,
    ) -> Result<(String, ExchangeResponseStatus)> {
        let wallet = wallet.unwrap_or(&self.wallet);
        let key = H256::from(generate_random_key()?).encode_hex()[2..].to_string();

        let address = key
            .parse::<LocalWallet>()
            .map_err(|e| Error::PrivateKeyParse(e.to_string()))?
            .address();

        let hyperliquid_chain = if self.http_client.is_mainnet() {
            "Mainnet".to_string()
        } else {
            "Testnet".to_string()
        };

        let nonce = next_nonce();
        let approve_agent = ApproveAgent {
            signature_chain_id: 421614.into(),
            hyperliquid_chain,
            agent_address: address,
            agent_name: None,
            nonce,
        };
        let signature = sign_typed_data(&approve_agent, wallet)?;
        let action = serde_json::to_value(Actions::ApproveAgent(approve_agent))
            .map_err(|e| Error::JsonParse(e.to_string()))?;
        Ok((key, self.post(action, signature, nonce).await?))
    }

    pub async fn withdraw_from_bridge(
        &self,
        amount: &str,
        destination: &str,
        wallet: Option<&LocalWallet>,
    ) -> Result<ExchangeResponseStatus> {
        let wallet = wallet.unwrap_or(&self.wallet);
        let hyperliquid_chain = if self.http_client.is_mainnet() {
            "Mainnet".to_string()
        } else {
            "Testnet".to_string()
        };

        let timestamp = next_nonce();
        let withdraw = Withdraw3 {
            signature_chain_id: 421614.into(),
            hyperliquid_chain,
            destination: destination.to_string(),
            amount: amount.to_string(),
            time: timestamp,
        };
        let signature = sign_typed_data(&withdraw, wallet)?;
        let action = serde_json::to_value(Actions::Withdraw3(withdraw))
            .map_err(|e| Error::JsonParse(e.to_string()))?;

        self.post(action, signature, timestamp).await
    }

    pub async fn spot_transfer(
        &self,
        amount: &str,
        destination: &str,
        token: &str,
        wallet: Option<&LocalWallet>,
    ) -> Result<ExchangeResponseStatus> {
        let wallet = wallet.unwrap_or(&self.wallet);
        let hyperliquid_chain = if self.http_client.is_mainnet() {
            "Mainnet".to_string()
        } else {
            "Testnet".to_string()
        };

        let timestamp = next_nonce();
        let spot_send = SpotSend {
            signature_chain_id: 421614.into(),
            hyperliquid_chain,
            destination: destination.to_string(),
            amount: amount.to_string(),
            time: timestamp,
            token: token.to_string(),
        };
        let signature = sign_typed_data(&spot_send, wallet)?;
        let action = serde_json::to_value(Actions::SpotSend(spot_send))
            .map_err(|e| Error::JsonParse(e.to_string()))?;

        self.post(action, signature, timestamp).await
    }

    pub async fn set_referrer(
        &self,
        code: String,
        wallet: Option<&LocalWallet>,
    ) -> Result<ExchangeResponseStatus> {
        let wallet = wallet.unwrap_or(&self.wallet);
        let timestamp = next_nonce();

        let action = Actions::SetReferrer(SetReferrer { code });

        let connection_id = action.hash(timestamp, self.vault_address)?;
        let action = serde_json::to_value(&action).map_err(|e| Error::JsonParse(e.to_string()))?;

        let is_mainnet = self.http_client.is_mainnet();
        let signature = sign_l1_action(wallet, connection_id, is_mainnet)?;
        self.post(action, signature, timestamp).await
    }

    pub async fn approve_builder_fee(
        &self,
        builder: String,
        max_fee_rate: String,
        wallet: Option<&LocalWallet>,
    ) -> Result<ExchangeResponseStatus> {
        let wallet = wallet.unwrap_or(&self.wallet);
        let timestamp = next_nonce();

        let hyperliquid_chain = if self.http_client.is_mainnet() {
            "Mainnet".to_string()
        } else {
            "Testnet".to_string()
        };

        let action = Actions::ApproveBuilderFee(ApproveBuilderFee {
            signature_chain_id: 421614.into(),
            hyperliquid_chain,
            builder,
            max_fee_rate,
            nonce: timestamp,
        });

        let connection_id = action.hash(timestamp, self.vault_address)?;
        let action = serde_json::to_value(&action).map_err(|e| Error::JsonParse(e.to_string()))?;

        let is_mainnet = self.http_client.is_mainnet();
        let signature = sign_l1_action(wallet, connection_id, is_mainnet)?;
        self.post(action, signature, timestamp).await
    }
}

fn round_to_decimals(value: f64, decimals: u32) -> f64 {
    let factor = 10f64.powi(decimals as i32);
    (value * factor).round() / factor
}

fn round_to_significant_and_decimal(value: f64, sig_figs: u32, max_decimals: u32) -> f64 {
    let abs_value = value.abs();
    let magnitude = abs_value.log10().floor() as i32;
    let scale = 10f64.powi(sig_figs as i32 - magnitude - 1);
    let rounded = (abs_value * scale).round() / scale;
    round_to_decimals(rounded.copysign(value), max_decimals)
}

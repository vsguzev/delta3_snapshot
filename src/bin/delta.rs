use ethers::signers::LocalWallet;

use delta3::market_maker::{MarketMaker, MarketMakerInput};

#[tokio::main]
async fn main() {
    env_logger::init();
    // log::set_max_level(LevelFilter::Off);


    let wallet: LocalWallet = "0x0000000000000000000000000000000000000000000000000000000000000000"
        .parse()
        .unwrap();
    let market_maker_input = MarketMakerInput {
        asset: "HYPE".to_string(),
        target_liquidity: 26.0,
        half_spread: 100,
        max_absolute_position_size: 780.0,
        decimals: 3,
        wallet,
    };
    loop {
        MarketMaker::new(market_maker_input.clone(), "0xA3ED069f18E64f3747AFDDE3b550a0A3CC26a747".parse().unwrap()).await.start().await
    }
}

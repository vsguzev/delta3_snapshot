use ethers::{
    contract::{Eip712, EthAbiType},
    core::k256::{elliptic_curve::FieldBytes, Secp256k1},
    signers::LocalWallet,
    types::{transaction::eip712::Eip712, Signature, H256, U256},
};

use crate::hyperliquid::{helpers::Result, proxy_digest::Sha256Proxy, Error};

pub(crate) fn sign_l1_action(
    wallet: &LocalWallet,
    connection_id: H256,
    is_mainnet: bool,
) -> Result<Signature> {
    let source = if is_mainnet { "a" } else { "b" }.to_string();
    sign_typed_data(
        &l1::Agent {
            source,
            connection_id,
        },
        wallet,
    )
}

pub(crate) fn sign_typed_data<T: Eip712>(payload: &T, wallet: &LocalWallet) -> Result<Signature> {
    let encoded = payload
        .encode_eip712()
        .map_err(|e| Error::Eip712(e.to_string()))?;

    sign_hash(H256::from(encoded), wallet)
}

fn sign_hash(hash: H256, wallet: &LocalWallet) -> Result<Signature> {
    let (sig, rec_id) = wallet
        .signer()
        .sign_digest_recoverable(Sha256Proxy::from(hash))
        .map_err(|e| Error::SignatureFailure(e.to_string()))?;

    let v = u8::from(rec_id) as u64 + 27;

    let r_bytes: FieldBytes<Secp256k1> = sig.r().into();
    let s_bytes: FieldBytes<Secp256k1> = sig.s().into();
    let r = U256::from_big_endian(r_bytes.as_slice());
    let s = U256::from_big_endian(s_bytes.as_slice());

    Ok(Signature { r, s, v })
}

pub(crate) mod l1 {
    use super::*;
    #[derive(Debug, Eip712, Clone, EthAbiType)]
    #[eip712(
        name = "Exchange",
        version = "1",
        chain_id = 1337,
        verifying_contract = "0x0000000000000000000000000000000000000000"
    )]
    pub(crate) struct Agent {
        pub(crate) source: String,
        pub(crate) connection_id: H256,
    }
}

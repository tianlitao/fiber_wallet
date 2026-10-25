//! Native CKB testnet transfers from the wallet's Fiber funding key.
//!
//! This module deliberately owns the RPC endpoint and network choice. Callers
//! cannot redirect a transaction to a caller-supplied node or mainnet.

use std::collections::HashMap;

use ckb_jsonrpc_types::OutputsValidator;
use ckb_sdk::{
    constants::SIGHASH_TYPE_HASH,
    traits::{DefaultCellDepResolver, SecpCkbRawKeySigner},
    tx_builder::{
        transfer::CapacityTransferBuilder, tx_fee_async, unlock_tx_async, CapacityBalancer,
        TxBuilder,
    },
    unlock::{ScriptUnlocker, SecpSighashUnlocker},
    Address, NetworkType, ScriptId,
};
use ckb_types::{
    bytes::Bytes,
    core::{BlockView, Capacity},
    packed::{CellOutput, Script, WitnessArgs},
    prelude::*,
    H256,
};
use fnn::ckb::config::{
    new_ckb_rpc_async_client, new_default_cell_collector, new_default_header_dep_resolver,
    new_default_transaction_dependency_provider,
};
use secp256k1::SecretKey;

use crate::types::{
    parse_onchain_tx_hash, validate_onchain_fee_limit, OnchainTransferError, OnchainTxState,
    WalletOnchainTransfer, WalletOnchainTxStatus,
};

/// The only CKB RPC this wallet is allowed to sign and broadcast for.
pub const TESTNET_CKB_RPC_URL: &str = "https://testnet.ckbapp.dev/";
const TX_DEP_CACHE_CAPACITY: usize = 10;
const DEFAULT_FEE_RATE: u64 = 1_000;

/// Parse and validate an address for the testnet used by this wallet.
///
/// `Address::from_str` alone accepts both `ckt` and `ckb` prefixes; the
/// explicit HRP and network checks are therefore both required.
pub fn parse_testnet_address(input: &str) -> Result<Address, OnchainTransferError> {
    let input = input.trim();
    let (hrp, _, _) =
        bech32::decode(input).map_err(|error| OnchainTransferError::InvalidAddress {
            detail: error.to_string(),
        })?;
    if hrp != "ckt" {
        return Err(OnchainTransferError::WrongNetwork {
            expected: "testnet (ckt)".to_owned(),
            actual: format!("{hrp} prefix"),
        });
    }
    let address = input
        .parse::<Address>()
        .map_err(|detail| OnchainTransferError::InvalidAddress { detail })?;
    if address.network() != NetworkType::Testnet {
        return Err(OnchainTransferError::WrongNetwork {
            expected: "testnet".to_owned(),
            actual: address.network().to_string(),
        });
    }
    Ok(address)
}

/// Reject a transfer amount that cannot form a valid output cell.
pub fn validate_transfer_output(
    target: &Address,
    amount_shannons: u64,
) -> Result<(), OnchainTransferError> {
    if amount_shannons == 0 {
        return Err(OnchainTransferError::InvalidAmount {
            detail: "转账金额必须大于 0".to_owned(),
        });
    }
    let output = CellOutput::new_builder()
        .capacity(Capacity::shannons(amount_shannons))
        .lock(Script::from(target))
        .build();
    let minimum = output
        .occupied_capacity(Capacity::zero())
        .map_err(|error| OnchainTransferError::Build {
            detail: format!("无法计算目标 cell 占用容量: {error}"),
        })?
        .as_u64();
    if amount_shannons < minimum {
        return Err(OnchainTransferError::AmountTooSmall {
            amount_shannons,
            minimum_shannons: minimum,
        });
    }
    Ok(())
}

/// Build, sign, verify the fee cap, and broadcast a native CKB transfer.
///
/// This function never returns "confirmed". A returned value only means that
/// the node accepted the transaction for broadcast and provided its hash.
pub async fn transfer_ckb(
    target_address: &str,
    amount_shannons: u64,
    max_fee_shannons: u64,
    funding_lock: Script,
    secret_key: SecretKey,
) -> Result<WalletOnchainTransfer, OnchainTransferError> {
    transfer_ckb_with_rpc(
        TESTNET_CKB_RPC_URL,
        target_address,
        amount_shannons,
        max_fee_shannons,
        funding_lock,
        secret_key,
    )
    .await
}

/// Internal seam used by tests to prove the endpoint cannot be overridden by
/// API callers. Production code should use [`transfer_ckb`].
async fn transfer_ckb_with_rpc(
    rpc_url: &str,
    target_address: &str,
    amount_shannons: u64,
    max_fee_shannons: u64,
    funding_lock: Script,
    secret_key: SecretKey,
) -> Result<WalletOnchainTransfer, OnchainTransferError> {
    if rpc_url != TESTNET_CKB_RPC_URL {
        return Err(OnchainTransferError::Unavailable {
            detail: "链上转账只允许使用内置的 CKB 测试网 RPC".to_owned(),
        });
    }
    validate_onchain_fee_limit(max_fee_shannons)?;
    let target = parse_testnet_address(target_address)?;
    validate_transfer_output(&target, amount_shannons)?;

    let client = new_ckb_rpc_async_client(rpc_url);
    let genesis = client
        .get_block_by_number(0.into())
        .await
        .map_err(|error| OnchainTransferError::Rpc {
            operation: "get_block_by_number(0)".to_owned(),
            detail: error.to_string(),
        })?
        .ok_or_else(|| OnchainTransferError::Rpc {
            operation: "get_block_by_number(0)".to_owned(),
            detail: "测试网没有返回 genesis block".to_owned(),
        })?;
    let genesis = BlockView::from(genesis);
    let cell_dep_resolver = DefaultCellDepResolver::from_genesis_async(&genesis)
        .await
        .map_err(|error| OnchainTransferError::Build {
            detail: format!("无法解析 CKB genesis cell deps: {error}"),
        })?;
    let header_dep_resolver = new_default_header_dep_resolver(rpc_url);
    let mut cell_collector = new_default_cell_collector(rpc_url);
    let tx_dep_provider =
        new_default_transaction_dependency_provider(rpc_url, TX_DEP_CACHE_CAPACITY).await;

    let signer = SecpCkbRawKeySigner::new_with_secret_keys(vec![secret_key]);
    let sighash_unlocker = SecpSighashUnlocker::from(Box::new(signer) as Box<_>);
    let sighash_script_id = ScriptId::new_type(SIGHASH_TYPE_HASH.clone());
    let mut unlockers: HashMap<ScriptId, Box<dyn ScriptUnlocker>> = HashMap::new();
    unlockers.insert(
        sighash_script_id,
        Box::new(sighash_unlocker) as Box<dyn ScriptUnlocker>,
    );

    let placeholder_witness = WitnessArgs::new_builder()
        .lock(Some(Bytes::from(vec![0u8; 65])).pack())
        .build();
    let mut balancer =
        CapacityBalancer::new_simple(funding_lock.clone(), placeholder_witness, DEFAULT_FEE_RATE);
    balancer.set_max_fee(Some(max_fee_shannons));

    let output = CellOutput::new_builder()
        .capacity(Capacity::shannons(amount_shannons))
        .lock(Script::from(&target))
        .build();
    let builder = CapacityTransferBuilder::new(vec![(output, Bytes::default())]);
    let balanced = builder
        .build_balanced_async(
            &mut cell_collector,
            &cell_dep_resolver,
            &header_dep_resolver,
            &tx_dep_provider,
            &balancer,
            &unlockers,
        )
        .await
        .map_err(|error| map_builder_error(error, max_fee_shannons))?;
    let (tx, still_locked) = unlock_tx_async(balanced, &tx_dep_provider, &unlockers)
        .await
        .map_err(|error| OnchainTransferError::Build {
            detail: format!("交易签名失败: {error}"),
        })?;
    if !still_locked.is_empty() {
        return Err(OnchainTransferError::Build {
            detail: format!("交易仍有 {} 个未解锁的 script group", still_locked.len()),
        });
    }

    // The balancer can only enforce its cap on the fee-burning path. Check the
    // final signed transaction as well, before any bytes leave the process.
    let actual_fee = tx_fee_async(tx.clone(), &tx_dep_provider, &header_dep_resolver)
        .await
        .map_err(|error| OnchainTransferError::Build {
            detail: format!("无法计算已签名交易手续费: {error}"),
        })?;
    if actual_fee > max_fee_shannons {
        return Err(OnchainTransferError::FeeLimitExceeded {
            fee_shannons: actual_fee,
            max_fee_shannons,
        });
    }

    let tx_hash = client
        .send_transaction(
            ckb_jsonrpc_types::TransactionView::from(tx.clone()).inner,
            Some(OutputsValidator::Passthrough),
        )
        .await
        .map_err(|error| OnchainTransferError::Broadcast {
            tx_hash: format!("{:#x}", tx.hash()),
            detail: error.to_string(),
        })?;

    Ok(WalletOnchainTransfer {
        tx_hash: format!("{tx_hash:#x}"),
        target_address: target.to_string(),
        amount_shannons,
        fee_shannons: Some(actual_fee),
    })
}

fn map_builder_error(
    error: ckb_sdk::tx_builder::TxBuilderError,
    max_fee_shannons: u64,
) -> OnchainTransferError {
    use ckb_sdk::tx_builder::{BalanceTxCapacityError, TxBuilderError};

    match error {
        TxBuilderError::BalanceCapacity(BalanceTxCapacityError::CapacityNotEnough(detail)) => {
            OnchainTransferError::InsufficientBalance { detail }
        }
        TxBuilderError::BalanceCapacity(BalanceTxCapacityError::ForceSmallChangeAsFeeFailed(
            fee,
        )) => OnchainTransferError::FeeLimitExceeded {
            fee_shannons: fee,
            max_fee_shannons,
        },
        TxBuilderError::CellCollector(error) => OnchainTransferError::Rpc {
            operation: "indexer cell collection".to_owned(),
            detail: error.to_string(),
        },
        TxBuilderError::TxDep(error) => OnchainTransferError::Rpc {
            operation: "transaction dependency".to_owned(),
            detail: error.to_string(),
        },
        TxBuilderError::BalanceCapacity(BalanceTxCapacityError::FailEstimateCycles(error)) => {
            OnchainTransferError::Rpc {
                operation: "transaction fee evaluation".to_owned(),
                detail: error.to_string(),
            }
        }
        other => OnchainTransferError::Build {
            detail: other.to_string(),
        },
    }
}

/// Query canonical transaction status. Unknown is a real state, not an error.
pub async fn transaction_status(
    tx_hash: &str,
    rpc_url: &str,
) -> Result<WalletOnchainTxStatus, OnchainTransferError> {
    if rpc_url != TESTNET_CKB_RPC_URL {
        return Err(OnchainTransferError::Unavailable {
            detail: "链上状态查询只允许使用内置的 CKB 测试网 RPC".to_owned(),
        });
    }
    let normalized = parse_onchain_tx_hash(tx_hash)?;
    let hash = parse_tx_hash(&normalized)?;
    let client = new_ckb_rpc_async_client(rpc_url);
    let response =
        client
            .get_transaction_status(hash)
            .await
            .map_err(|error| OnchainTransferError::Rpc {
                operation: "get_transaction".to_owned(),
                detail: error.to_string(),
            })?;
    transaction_status_from_rpc(&normalized, response.tx_status)
}

/// Map the RPC transaction status into the wallet-facing type. Kept separate
/// from networking so the pending/committed distinction is unit-testable.
fn transaction_status_from_rpc(
    requested_hash: &str,
    status: ckb_jsonrpc_types::TxStatus,
) -> Result<WalletOnchainTxStatus, OnchainTransferError> {
    let hash = parse_onchain_tx_hash(requested_hash)?;
    let state = match status.status {
        ckb_jsonrpc_types::Status::Pending => OnchainTxState::Pending,
        ckb_jsonrpc_types::Status::Proposed => OnchainTxState::Proposed,
        ckb_jsonrpc_types::Status::Committed => OnchainTxState::Committed,
        ckb_jsonrpc_types::Status::Rejected => OnchainTxState::Rejected,
        ckb_jsonrpc_types::Status::Unknown => OnchainTxState::Unknown,
    };
    Ok(WalletOnchainTxStatus {
        tx_hash: hash,
        state,
        block_number: status.block_number.map(|number| number.value()),
        block_hash: status.block_hash.map(|hash| format!("{hash:#x}")),
        reason: status.reason,
    })
}

/// Validate and normalize a 32-byte transaction hash.
pub fn parse_tx_hash(input: &str) -> Result<H256, OnchainTransferError> {
    let normalized = parse_onchain_tx_hash(input)?;
    let bytes =
        hex::decode(&normalized[2..]).map_err(|error| OnchainTransferError::InvalidTxHash {
            detail: error.to_string(),
        })?;
    let array: [u8; 32] = bytes
        .try_into()
        .map_err(|_| OnchainTransferError::InvalidTxHash {
            detail: "交易哈希长度不是 32 字节".to_owned(),
        })?;
    Ok(H256::from(array))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TESTNET_ADDRESS: &str =
        "ckt1qzda0cr08m85hc8jlnfp3zer7xulejywt49kt2rr0vthywaa50xwsqf7v2xsyj0p8szesqrwqapvvygpc8hzg9sku954v";
    const MAINNET_ADDRESS: &str =
        "ckb1qzda0cr08m85hc8jlnfp3zer7xulejywt49kt2rr0vthywaa50xwsqgvf0k9sc40s3azmpfvhyuudhahpsj72tsr8cx3d";

    #[test]
    fn accepts_testnet_address() {
        let address = parse_testnet_address(TESTNET_ADDRESS).expect("valid testnet");
        assert_eq!(address.network(), NetworkType::Testnet);
    }

    #[test]
    fn rejects_mainnet_address_and_wrong_hrp() {
        assert!(matches!(
            parse_testnet_address(MAINNET_ADDRESS),
            Err(OnchainTransferError::WrongNetwork { .. })
        ));
        assert!(matches!(
            parse_testnet_address("bc1qv2xsyj0p8szesqrwqapvvygpc8hzg9sku954v"),
            Err(OnchainTransferError::WrongNetwork { .. })
                | Err(OnchainTransferError::InvalidAddress { .. })
        ));
    }

    #[test]
    fn rejects_truncated_or_bad_checksum_address() {
        let truncated = &TESTNET_ADDRESS[..TESTNET_ADDRESS.len() - 1];
        assert!(parse_testnet_address(truncated).is_err());
        let bad_checksum = format!("{}x", &TESTNET_ADDRESS[..TESTNET_ADDRESS.len() - 1]);
        assert!(parse_testnet_address(&bad_checksum).is_err());
    }

    #[test]
    fn rejects_zero_amount() {
        let target = parse_testnet_address(TESTNET_ADDRESS).unwrap();
        assert!(matches!(
            validate_transfer_output(&target, 0),
            Err(OnchainTransferError::InvalidAmount { .. })
        ));
    }

    #[test]
    fn parses_tx_hash_strictly() {
        let valid = format!("0x{}", "11".repeat(32));
        assert_eq!(format!("{:#x}", parse_tx_hash(&valid).unwrap()), valid);
        assert!(parse_tx_hash("0x1234").is_err());
        assert!(parse_tx_hash(&format!("0x{}", "zz".repeat(32))).is_err());
    }

    #[test]
    fn maps_unknown_status_without_calling_the_hash_invalid() {
        let hash = format!("0x{}", "22".repeat(32));
        let status = transaction_status_from_rpc(&hash, ckb_jsonrpc_types::TxStatus::unknown())
            .expect("unknown is a valid status");
        assert_eq!(status.tx_hash, hash);
        assert_eq!(status.state, OnchainTxState::Unknown);
    }

    #[test]
    fn maps_pending_and_rejected_without_claiming_confirmation() {
        let hash = format!("0x{}", "55".repeat(32));
        let pending =
            transaction_status_from_rpc(&hash, ckb_jsonrpc_types::TxStatus::pending()).unwrap();
        assert_eq!(pending.state, OnchainTxState::Pending);
        assert_eq!(pending.block_number, None);

        let rejected = transaction_status_from_rpc(
            &hash,
            ckb_jsonrpc_types::TxStatus::rejected("invalid fee".to_owned()),
        )
        .unwrap();
        assert_eq!(rejected.state, OnchainTxState::Rejected);
        assert_eq!(rejected.reason.as_deref(), Some("invalid fee"));
    }

    #[test]
    fn maps_committed_status_and_block_metadata() {
        let hash = format!("0x{}", "33".repeat(32));
        let status = transaction_status_from_rpc(
            &hash,
            ckb_jsonrpc_types::TxStatus::committed(123.into(), H256::from([0x44; 32]), 0.into()),
        )
        .expect("committed status");
        assert_eq!(status.state, OnchainTxState::Committed);
        assert_eq!(status.block_number, Some(123));
        assert_eq!(
            status.block_hash,
            Some(format!("{:#x}", H256::from([0x44; 32])))
        );
    }

    #[test]
    fn address_uses_the_testnet_bech32_variant() {
        let (hrp, _, _) = bech32::decode(TESTNET_ADDRESS).unwrap();
        assert_eq!(hrp, "ckt");
    }

    #[tokio::test]
    async fn refuses_non_builtin_rpc_before_any_network_call() {
        let key = SecretKey::from_slice(&[1u8; 32]).unwrap();
        let target = parse_testnet_address(TESTNET_ADDRESS).unwrap();
        let funding_lock = Script::from(&target);
        let result = transfer_ckb_with_rpc(
            "https://mainnet.example.invalid/",
            TESTNET_ADDRESS,
            100_000_000,
            1_000_000,
            funding_lock,
            key,
        )
        .await;
        assert!(matches!(
            result,
            Err(OnchainTransferError::Unavailable { .. })
        ));
    }
}

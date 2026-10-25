use std::sync::Mutex;
use std::time::Duration;

use crate::types::{
    ChannelCloseRequest, OnchainTransferError, WalletChannel, WalletChannelCloseResult,
    WalletInvoice, WalletNodeInfo, WalletOnchainTransfer, WalletOnchainTxStatus,
    WalletOpenedChannel, WalletPayment, WalletPaymentStatus, WalletPeer, WalletSendPaymentRequest,
    WalletSendPaymentResult,
};
use crate::types::{NodeNetwork, NodePhase, NodeSnapshot};

const START_DELAY: Duration = Duration::from_millis(300);
const STOP_DELAY: Duration = Duration::from_millis(180);

/// Simulated node lifecycle used by Web and unit tests.
pub struct NodeMachine {
    state: Mutex<NodeSnapshot>,
    operation: tokio::sync::Mutex<()>,
}

impl NodeMachine {
    pub fn new(network: NodeNetwork, _data_dir: String, _secret_password: String) -> Self {
        Self {
            state: Mutex::new(NodeSnapshot {
                phase: NodePhase::Stopped,
                network,
                detail: "Local node is stopped".to_owned(),
                node_id: None,
                backend: "simulated (M0)".to_owned(),
            }),
            operation: tokio::sync::Mutex::new(()),
        }
    }

    pub fn snapshot(&self) -> NodeSnapshot {
        self.state.lock().expect("node state poisoned").clone()
    }

    pub async fn start(&self) -> Result<NodeSnapshot, String> {
        let _operation = self.operation.lock().await;
        {
            let mut state = self.state.lock().expect("node state poisoned");
            if matches!(state.phase, NodePhase::Starting | NodePhase::Running) {
                return Ok(state.clone());
            }
            state.phase = NodePhase::Starting;
            state.detail = "Starting simulated local node".to_owned();
            state.node_id = None;
        }

        tokio::time::sleep(START_DELAY).await;

        let mut state = self.state.lock().expect("node state poisoned");
        state.phase = NodePhase::Running;
        state.detail = "Simulated local node is running while the app is active".to_owned();
        state.node_id = Some("02simulated-local-node".to_owned());
        Ok(state.clone())
    }

    pub async fn stop(&self) -> Result<NodeSnapshot, String> {
        let _operation = self.operation.lock().await;
        {
            let mut state = self.state.lock().expect("node state poisoned");
            if matches!(state.phase, NodePhase::Stopping | NodePhase::Stopped) {
                return Ok(state.clone());
            }
            state.phase = NodePhase::Stopping;
            state.detail = "Stopping simulated local node".to_owned();
        }

        tokio::time::sleep(STOP_DELAY).await;

        let mut state = self.state.lock().expect("node state poisoned");
        state.phase = NodePhase::Stopped;
        state.detail = "Local node is offline".to_owned();
        state.node_id = None;
        Ok(state.clone())
    }

    pub fn mark_failed(&self, message: String) -> NodeSnapshot {
        let mut state = self.state.lock().expect("node state poisoned");
        state.phase = NodePhase::Failed;
        state.detail = message;
        state.node_id = None;
        state.clone()
    }
}

// Wallet operations are only implemented by the native backend; the simulated
// backend exists for web and unit tests, where there is no Fiber node.
impl NodeMachine {
    fn wallet_unavailable<T>() -> Result<T, String> {
        Err("wallet operations require the native node backend".to_owned())
    }

    pub async fn wallet_node_info(&self) -> Result<WalletNodeInfo, String> {
        Self::wallet_unavailable()
    }

    pub async fn wallet_list_channels(&self) -> Result<Vec<WalletChannel>, String> {
        Self::wallet_unavailable()
    }

    pub async fn wallet_list_peers(&self) -> Result<Vec<WalletPeer>, String> {
        Self::wallet_unavailable()
    }

    pub async fn wallet_connect_peer(&self, _address: String) -> Result<(), String> {
        Self::wallet_unavailable()
    }

    pub async fn wallet_open_channel(
        &self,
        _peer_pubkey: String,
        _funding_amount_shannons: u64,
        _public_channel: bool,
    ) -> Result<WalletOpenedChannel, String> {
        Self::wallet_unavailable()
    }

    pub async fn wallet_close_channel(
        &self,
        _request: ChannelCloseRequest,
    ) -> Result<WalletChannelCloseResult, String> {
        Self::wallet_unavailable()
    }

    pub async fn wallet_new_invoice(
        &self,
        _amount_shannons: u64,
        _description: String,
    ) -> Result<WalletInvoice, String> {
        Self::wallet_unavailable()
    }

    pub async fn wallet_list_payments(&self, _limit: u32) -> Result<Vec<WalletPayment>, String> {
        Self::wallet_unavailable()
    }

    pub async fn wallet_get_payment(&self, _payment_hash: String) -> Result<WalletPayment, String> {
        Self::wallet_unavailable()
    }

    pub async fn wallet_send_payment(&self, _invoice: String) -> Result<WalletPayment, String> {
        Self::wallet_unavailable()
    }

    pub async fn wallet_send_payment_request(
        &self,
        request: WalletSendPaymentRequest,
    ) -> Result<WalletSendPaymentResult, String> {
        request.validate()?;
        Self::wallet_unavailable()
    }

    pub async fn wallet_poll_payment_terminal(
        &self,
        _payment_hash: String,
        expected_status: Option<WalletPaymentStatus>,
        timeout: Duration,
    ) -> Result<WalletPayment, String> {
        if expected_status.is_some_and(WalletPaymentStatus::is_terminal) {
            return Err(
                "expected_status must be Created or Inflight; terminal states are returned by polling"
                    .to_owned(),
            );
        }
        if timeout.is_zero() {
            return Err("payment polling timeout must be greater than 0".to_owned());
        }
        Self::wallet_unavailable()
    }

    pub async fn wallet_onchain_transfer(
        &self,
        _target_address: String,
        _amount_shannons: u64,
        _max_fee_shannons: u64,
    ) -> Result<WalletOnchainTransfer, OnchainTransferError> {
        Err(OnchainTransferError::Unavailable {
            detail: "模拟后端不广播链上交易".to_owned(),
        })
    }

    pub async fn wallet_onchain_transfer_status(
        &self,
        _tx_hash: String,
    ) -> Result<WalletOnchainTxStatus, OnchainTransferError> {
        Err(OnchainTransferError::Unavailable {
            detail: "模拟后端不查询链上交易状态".to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn starts_and_stops() {
        let node = NodeMachine::new(
            NodeNetwork::Testnet,
            "unused".to_owned(),
            "unused".to_owned(),
        );

        assert_eq!(node.snapshot().phase, NodePhase::Stopped);
        let running = node.start().await.expect("start");
        assert_eq!(running.phase, NodePhase::Running);
        assert!(running.node_id.is_some());

        let stopped = node.stop().await.expect("stop");
        assert_eq!(stopped.phase, NodePhase::Stopped);
        assert!(stopped.node_id.is_none());
    }

    #[test]
    fn transitions_to_failed() {
        let node = NodeMachine::new(
            NodeNetwork::Testnet,
            "unused".to_owned(),
            "unused".to_owned(),
        );
        let failed = node.mark_failed("database lock failed".to_owned());

        assert_eq!(failed.phase, NodePhase::Failed);
        assert_eq!(failed.detail, "database lock failed");
    }
}

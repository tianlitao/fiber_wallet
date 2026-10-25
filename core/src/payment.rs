//! Shared payment polling semantics.

use std::future::Future;
use std::time::Duration;

use crate::types::WalletPayment;

pub(crate) const PAYMENT_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Polls a submitted payment until Fiber reports `Success` or `Failed`.
///
/// A local timeout is not proof that the payment failed: an HTLC can still
/// settle after the UI stops waiting. The error text therefore reports the
/// last observed state and tells the caller to keep checking the record.
pub(crate) async fn poll_payment_until_terminal<F, Fut>(
    payment_hash: &str,
    timeout: Duration,
    mut fetch: F,
) -> Result<WalletPayment, String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<WalletPayment, String>>,
{
    if timeout.is_zero() {
        return Err("payment polling timeout must be greater than 0".to_owned());
    }

    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let payment = fetch().await?;
        if payment.status.is_terminal() {
            return Ok(payment);
        }

        let now = tokio::time::Instant::now();
        if now >= deadline {
            return Err(format!(
                "payment {payment_hash} did not reach Success/Failed within {} seconds; \
                 last status was {}; this is not proof of failure, keep checking the record",
                timeout.as_secs(),
                payment.status
            ));
        }

        tokio::time::sleep(PAYMENT_POLL_INTERVAL.min(deadline - now)).await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::*;
    use crate::types::WalletPaymentStatus;

    fn payment(status: WalletPaymentStatus) -> WalletPayment {
        WalletPayment {
            payment_hash: format!("0x{}", "01".repeat(32)),
            status,
            failed_error: (status == WalletPaymentStatus::Failed)
                .then(|| "route exhausted".to_owned()),
            fee_shannons: "0".to_owned(),
            created_at_ms: 1,
            last_updated_at_ms: 2,
        }
    }

    #[tokio::test]
    async fn waits_through_created_and_inflight() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_for_fetch = calls.clone();
        let result = poll_payment_until_terminal("payment", Duration::from_secs(1), move || {
            let call = calls_for_fetch.fetch_add(1, Ordering::SeqCst);
            async move {
                Ok(match call {
                    0 => payment(WalletPaymentStatus::Created),
                    1 => payment(WalletPaymentStatus::Inflight),
                    _ => payment(WalletPaymentStatus::Success),
                })
            }
        })
        .await
        .expect("eventual success");

        assert_eq!(result.status, WalletPaymentStatus::Success);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn returns_failed_as_a_terminal_outcome() {
        let result = poll_payment_until_terminal("payment", Duration::from_secs(1), || async {
            Ok(payment(WalletPaymentStatus::Failed))
        })
        .await
        .expect("failed is terminal");

        assert_eq!(result.status, WalletPaymentStatus::Failed);
        assert_eq!(result.failed_error.as_deref(), Some("route exhausted"));
    }

    #[tokio::test]
    async fn timeout_reports_the_last_nonterminal_state() {
        let error = poll_payment_until_terminal("payment", Duration::from_secs(1), || async {
            Ok(payment(WalletPaymentStatus::Inflight))
        })
        .await
        .expect_err("inflight must not be reported as success");

        assert!(error.contains("within 1 seconds"));
        assert!(error.contains("Inflight"));
        assert!(error.contains("not proof of failure"));
    }
}

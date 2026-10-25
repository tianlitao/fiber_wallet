//! UI-side rules for closing a channel.
//!
//! These helpers stay independent of the widget tree so parameter parsing and
//! status wording can be tested without starting a Fiber node.

use fiber_wallet_core::api::wallet::{ChannelCloseMode, ChannelCloseStatus};

/// Which close modes the channel row may offer in its current FNN state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseActionKind {
    /// Normal ready channel: the user chooses cooperative or force.
    ChooseMode,
    /// Not `ChannelReady`, but FNN still permits a force close.
    ForceOnly,
}

/// Returns whether a row should offer closing, and which choices to show.
pub fn close_action_kind(channel_state: &str, state_flags: Option<u32>) -> Option<CloseActionKind> {
    if channel_state.trim() == "ChannelReady" {
        return Some(CloseActionKind::ChooseMode);
    }
    if fiber_wallet_core::api::wallet::can_force_close(Some(channel_state), state_flags) {
        return Some(CloseActionKind::ForceOnly);
    }
    None
}

/// Parses the optional fee-rate field (shannons per 1000 weight units).
///
/// An empty cooperative value means FNN's channel commitment fee rate. Force
/// closes reject a fee because FNN ignores it and rejects ambiguous requests.
pub fn parse_fee_rate(mode: ChannelCloseMode, value: &str) -> Result<Option<u64>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if mode == ChannelCloseMode::Force {
        return Err("强制关闭不支持自定义费率".to_owned());
    }
    let fee_rate = value
        .parse::<u64>()
        .map_err(|_| "关闭费率必须是正整数（shannons/KW）".to_owned())?;
    if fee_rate == 0 {
        return Err("关闭费率必须大于 0".to_owned());
    }
    Ok(Some(fee_rate))
}

/// Canonical key for comparing channel ids coming from FNN and the UI.
pub fn canonical_channel_key(channel_id: &str) -> String {
    channel_id
        .trim()
        .strip_prefix("0x")
        .or_else(|| channel_id.trim().strip_prefix("0X"))
        .unwrap_or(channel_id.trim())
        .to_ascii_lowercase()
}

/// Short status wording for a channel-close attempt.
pub fn status_label(status: ChannelCloseStatus) -> &'static str {
    match status {
        ChannelCloseStatus::Submitted => "关闭请求已提交",
        ChannelCloseStatus::InProgress => "关闭进行中",
        ChannelCloseStatus::WaitingOnchainSettlement => "等待链上结算",
        ChannelCloseStatus::Closed => "关闭成功",
        ChannelCloseStatus::OpeningFailed => "开通道失败/资金已中止",
        ChannelCloseStatus::Failed => "关闭失败",
        ChannelCloseStatus::FailedWithDetail => "开/关通道失败",
        ChannelCloseStatus::Unknown => "关闭状态未知",
    }
}

/// Human-readable explanation for the selected close method.
pub fn mode_description(mode: ChannelCloseMode) -> &'static str {
    match mode {
        ChannelCloseMode::Cooperative => {
            "需要对方在线并同意。关闭交易手续费从你的通道余额扣除，完成后余额回到默认 funding 地址。"
        }
        ChannelCloseMode::Force => {
            "不等待对方同意，广播最新 commitment 交易。资金需等待链上结算，且可能承担更高风险和延迟。"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_optional_cooperative_fee() {
        assert_eq!(parse_fee_rate(ChannelCloseMode::Cooperative, ""), Ok(None));
        assert_eq!(
            parse_fee_rate(ChannelCloseMode::Cooperative, " 1000 "),
            Ok(Some(1_000))
        );
        assert!(parse_fee_rate(ChannelCloseMode::Cooperative, "0").is_err());
        assert!(parse_fee_rate(ChannelCloseMode::Cooperative, "-1").is_err());
        assert!(parse_fee_rate(ChannelCloseMode::Cooperative, "1.5").is_err());
        assert!(parse_fee_rate(ChannelCloseMode::Force, "1000").is_err());
        assert_eq!(parse_fee_rate(ChannelCloseMode::Force, ""), Ok(None));
    }

    #[test]
    fn only_offers_closing_when_fnn_can_accept_it() {
        assert_eq!(
            close_action_kind("ChannelReady", None),
            Some(CloseActionKind::ChooseMode)
        );
        assert_eq!(
            close_action_kind("ShuttingDown", None),
            Some(CloseActionKind::ForceOnly)
        );
        assert_eq!(
            close_action_kind("AwaitingChannelReady", Some(1)),
            Some(CloseActionKind::ForceOnly)
        );
        assert_eq!(close_action_kind("AwaitingChannelReady", Some(0)), None);
        assert_eq!(close_action_kind("AwaitingChannelReady", None), None);
        assert_eq!(close_action_kind("Closed", Some(0)), None);
        assert_eq!(close_action_kind("", None), None);
    }

    #[test]
    fn normalizes_ids_and_labels_statuses() {
        assert_eq!(canonical_channel_key(" 0xABcd "), "abcd");
        assert_eq!(canonical_channel_key("ABcd"), "abcd");
        assert_eq!(status_label(ChannelCloseStatus::Closed), "关闭成功");
        assert_eq!(
            status_label(ChannelCloseStatus::WaitingOnchainSettlement),
            "等待链上结算"
        );
        assert_eq!(
            status_label(ChannelCloseStatus::OpeningFailed),
            "开通道失败/资金已中止"
        );
        assert_eq!(status_label(ChannelCloseStatus::Failed), "关闭失败");
        assert_eq!(
            status_label(ChannelCloseStatus::FailedWithDetail),
            "开/关通道失败"
        );
    }
}

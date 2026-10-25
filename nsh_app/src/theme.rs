//! The app's own reactive theme: Nervos/Fiber-flavored palettes in light and
//! dark, five accent choices, and a Longbridge-like type scale.
//!
//! NSH's global token set is not mutable from app code, and the app draws
//! every surface itself, so the theme lives here: a [`Palette`] resolved from
//! the config signal plus the system appearance, re-read wherever a style
//! closure needs a color (both reads are tracked, so switches apply live).

use nsh_core::ui::{ambient, prelude::*};

use crate::config::{Accent, AppConfig, ThemeMode};

/// Longbridge-like type scale, in points.
pub mod type_scale {
    pub const XS: f32 = 11.0;
    pub const SM: f32 = 12.0;
    pub const BASE: f32 = 13.0;
    pub const MD: f32 = 14.0;
    pub const LG: f32 = 16.0;
    pub const XL: f32 = 18.0;
    pub const TITLE: f32 = 20.0;
    pub const HERO: f32 = 28.0;
}

/// One resolved appearance: every color the UI draws with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub bg: Hsla,
    /// Card surfaces.
    pub card: Hsla,
    /// Elevated/inner surfaces (inputs, code, hover).
    pub card_alt: Hsla,
    pub fg: Hsla,
    pub fg_muted: Hsla,
    pub border: Hsla,
    pub accent: Hsla,
    /// Accent at its background-tint strength (selected nav rows, badges).
    pub accent_soft: Hsla,
    /// Text on the accent color.
    pub accent_fg: Hsla,
    /// 红涨.
    pub up: Hsla,
    /// 绿跌.
    pub down: Hsla,
    pub ok: Hsla,
    pub warn: Hsla,
    pub danger: Hsla,
    /// Hero-card gradient (accent-family).
    pub gradient_from: Hsla,
    pub gradient_to: Hsla,
}

pub fn is_dark(config: &AppConfig) -> bool {
    match config.theme_mode {
        ThemeMode::System => ambient::dark(),
        ThemeMode::Light => false,
        ThemeMode::Dark => true,
    }
}

/// Resolves the palette for `config` (tracked: re-runs on config or
/// appearance change when read inside a reactive closure).
pub fn palette(config: &AppConfig) -> Palette {
    let dark = is_dark(config);
    base(dark).merge(accent(config.accent, dark))
}

fn base(dark: bool) -> Palette {
    if dark {
        // Nervos navy-black with layered cards.
        Palette {
            bg: rgb(0x0b0f1a).into(),
            card: rgb(0x121826).into(),
            card_alt: rgb(0x1a2233).into(),
            fg: rgb(0xe8ecf4).into(),
            fg_muted: rgb(0x8b93a7).into(),
            border: rgb(0x212a3d).into(),
            up: rgb(0xff5d5d).into(),
            down: rgb(0x3dd68c).into(),
            ok: rgb(0x3dd68c).into(),
            warn: rgb(0xf5a623).into(),
            danger: rgb(0xff5d5d).into(),
            // filled by accent():
            accent: Hsla::default(),
            accent_soft: Hsla::default(),
            accent_fg: rgb(0x07121c).into(),
            gradient_from: Hsla::default(),
            gradient_to: Hsla::default(),
        }
    } else {
        Palette {
            bg: rgb(0xf4f6fa).into(),
            card: rgb(0xffffff).into(),
            card_alt: rgb(0xf0f3f8).into(),
            fg: rgb(0x161c2c).into(),
            fg_muted: rgb(0x69748c).into(),
            border: rgb(0xe3e8f0).into(),
            up: rgb(0xe5484d).into(),
            down: rgb(0x18a86b).into(),
            ok: rgb(0x18a86b).into(),
            warn: rgb(0xd97a06).into(),
            danger: rgb(0xe5484d).into(),
            accent: Hsla::default(),
            accent_soft: Hsla::default(),
            accent_fg: rgb(0xffffff).into(),
            gradient_from: Hsla::default(),
            gradient_to: Hsla::default(),
        }
    }
}

/// (accent, soft tint, gradient from, gradient to) per accent id.
fn accent(accent: Accent, dark: bool) -> [Hsla; 4] {
    match accent {
        // CKB mint: the Nervos ecosystem's signature green-cyan.
        Accent::CkbMint => [
            rgb(if dark { 0x2ee6a8 } else { 0x0aa376 }).into(),
            rgb(if dark { 0x12301f } else { 0xd8f5e9 }).into(),
            rgb(0x2ee6a8).into(),
            rgb(0x38bdf8).into(),
        ],
        // Fiber teal — also the teal Longbridge tokens use today.
        Accent::FiberTeal => [
            rgb(if dark { 0x22d3ce } else { 0x0e8f93 }).into(),
            rgb(if dark { 0x0f2e30 } else { 0xd6f3f4 }).into(),
            rgb(0x22d3ce).into(),
            rgb(0x60a5fa).into(),
        ],
        Accent::Aurora => [
            rgb(if dark { 0x60a5fa } else { 0x2b6fd8 }).into(),
            rgb(if dark { 0x14263f } else { 0xdce9fb }).into(),
            rgb(0x60a5fa).into(),
            rgb(0x2ee6a8).into(),
        ],
        Accent::Violet => [
            rgb(if dark { 0x8b7cf6 } else { 0x6d4fe0 }).into(),
            rgb(if dark { 0x231d40 } else { 0xe7e2fb }).into(),
            rgb(0x8b7cf6).into(),
            rgb(0x22d3ce).into(),
        ],
        Accent::Amber => [
            rgb(if dark { 0xf5a623 } else { 0xc77800 }).into(),
            rgb(if dark { 0x33270f } else { 0xfcf0d8 }).into(),
            rgb(0xf5a623).into(),
            rgb(0xff5d5d).into(),
        ],
    }
}

/// `base` with the accent fields filled from `accent`.
trait MergePalette {
    fn merge(self, accent: [Hsla; 4]) -> Palette;
}

impl MergePalette for Palette {
    fn merge(mut self, accent: [Hsla; 4]) -> Palette {
        self.accent = accent[0];
        self.accent_soft = accent[1];
        self.gradient_from = accent[2];
        self.gradient_to = accent[3];
        self
    }
}

pub fn mono_family() -> SharedString {
    SharedString::from("Menlo")
}

/// Formats a shannons balance for display: thousands-separated CKB.
/// CKB for display, thousands-grouped. The fraction is capped at 4 digits
/// (truncated, never rounded up — the shown amount must not exceed the real
/// one); full precision stays available in the payment/hash details.
pub fn ckb_display(shannons: &str) -> String {
    let value: u128 = shannons.parse().unwrap_or(0);
    let whole = value / 100_000_000;
    let fraction = format!("{:08}", value % 100_000_000);
    let fraction = &fraction[..4];
    let fraction = fraction.trim_end_matches('0');
    let grouped = group_thousands(&whole.to_string());
    if fraction.is_empty() {
        grouped
    } else {
        format!("{grouped}.{fraction}")
    }
}

pub fn group_thousands(digits: &str) -> String {
    let bytes = digits.as_bytes();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, byte) in bytes.iter().enumerate() {
        if index > 0 && (bytes.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*byte as char);
    }
    out
}

/// Shortens a long id for display: `abcd1234…wxyz9876`.
pub fn short_id(id: &str, keep: usize) -> String {
    if id.len() <= keep * 2 + 1 {
        return id.to_owned();
    }
    let head: String = id.chars().take(keep).collect();
    let tail: String = id.chars().skip(id.chars().count() - keep).collect();
    format!("{head}…{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ckb_display_caps_fraction_at_four_digits() {
        assert_eq!(ckb_display("0"), "0");
        assert_eq!(ckb_display("100000000"), "1");
        // 1.5 CKB
        assert_eq!(ckb_display("150000000"), "1.5");
        // Truncates, never rounds up: 9,699.99999999536 CKB shows 9,699.9999.
        assert_eq!(ckb_display("969999990536"), "9,699.9999");
        // 0.00009 CKB truncates to 0 — the fraction vanishes.
        assert_eq!(ckb_display("9000"), "0");
        assert_eq!(ckb_display("123456789012345678"), "1,234,567,890.1234");
    }
}

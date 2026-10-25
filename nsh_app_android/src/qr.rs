//! QR decoding over a camera frame's luma plane.
//!
//! Kept platform-free so the decoder runs in host tests: `scanner.rs` feeds
//! the copied Y plane here, and the tests synthesize frames with fast_qr —
//! the same generator the shared shell's QR canvases use — including the
//! padded row stride real camera buffers carry.

/// Decodes the first QR code in a `width x height` luma frame. `stride` is
/// the byte distance between consecutive rows (camera buffers pad rows);
/// `luma` must hold at least `stride * (height - 1) + width` bytes.
pub fn decode_luma(width: usize, height: usize, stride: usize, luma: &[u8]) -> Option<String> {
    if width == 0 || height == 0 || luma.len() < stride * (height - 1) + width {
        return None;
    }
    let mut prepared =
        rqrr::PreparedImage::prepare_from_greyscale(width, height, |x, y| luma[y * stride + x]);
    prepared
        .detect_grids()
        .iter()
        .find_map(|grid| grid.decode().ok().map(|(_, text)| text))
}

#[cfg(test)]
mod tests {
    use super::decode_luma;
    use fast_qr::QRBuilder;

    /// Builds a `width x height` luma frame around a QR code, the way a
    /// camera frame would look: light background, dark modules, centered
    /// with a quiet zone, and rows padded to `stride` bytes.
    fn frame(qr: &fast_qr::QRCode, width: usize, height: usize, stride: usize) -> Vec<u8> {
        let modules = qr.size as usize;
        let quiet = 4;
        let cell = (width.min(height) / (modules + quiet * 2)).max(1);
        let qr_span = modules * cell;
        let left = (width - qr_span) / 2;
        let top = (height - qr_span) / 2;
        let mut luma = vec![255_u8; stride * (height - 1) + width];
        for y in 0..height {
            for x in 0..width {
                if x >= left && x < left + qr_span && y >= top && y < top + qr_span {
                    let module = qr[(y - top) / cell][(x - left) / cell].value();
                    luma[y * stride + x] = if module { 0 } else { 255 };
                }
            }
        }
        luma
    }

    #[test]
    fn decodes_a_padded_camera_frame() {
        let text = "ckb1qzda0cr08mwdhhk3pxh65gqu07hqrl4vvklctfqytx0h5em4n3vwcqgvcdl8n";
        let qr = QRBuilder::new(text).ecl(fast_qr::ECL::Q).build().expect("qr builds");
        // Camera-shaped: rows padded well past the width.
        let luma = frame(&qr, 640, 480, 704);
        assert_eq!(
            decode_luma(640, 480, 704, &luma).as_deref(),
            Some(text),
            "the padded synthetic frame must decode"
        );
    }

    #[test]
    fn decodes_invoice_sized_payload() {
        let text = "fiber:invoice?h=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef&s=100000000000";
        let qr = QRBuilder::new(text).ecl(fast_qr::ECL::M).build().expect("qr builds");
        let luma = frame(&qr, 1280, 720, 1280);
        assert_eq!(decode_luma(1280, 720, 1280, &luma).as_deref(), Some(text));
    }

    #[test]
    fn rejects_garbage_and_short_frames() {
        assert_eq!(decode_luma(8, 8, 8, &vec![128_u8; 64]), None);
        // A frame truncated against its stride reports no code rather than
        // panicking on the out-of-frame rows.
        assert_eq!(decode_luma(640, 480, 704, &vec![255_u8; 10]), None);
    }
}

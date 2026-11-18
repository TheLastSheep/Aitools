//! Pure-Rust monochrome icon analyzer matching ZTools `imageAnalysis.ts` (`SPEC-016-R2`).

use std::collections::HashMap;
use std::path::Path;

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};

const TARGET_SAMPLES: usize = 1600;
const ALPHA_THRESHOLD: u8 = 20;
const COLOR_DISTANCE_THRESHOLD: f64 = 30.0;
const MIN_SIMILARITY_RATIO: f64 = 0.85;
const MIN_TRANSPARENCY_RATIO: f64 = 0.10;
const MIN_OPAQUE_PIXELS: usize = 20;
const MAX_ICON_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// Result of ZTools-parity monochrome icon color analysis (`SPEC-016-R2`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct IconAdaptation {
    /// Whether the icon is a pure-color silhouette over a transparent background.
    pub is_simple_icon: bool,
    /// Dominant hex RGB color (`"#rrggbb"`) when [`Self::is_simple_icon`] is `true`.
    pub main_color: Option<String>,
    /// Whether the dominant color has W3C perceived luminance `< 0.5`.
    pub is_dark: bool,
    /// Whether the UI should invert this icon when rendered on a matching dark/light background.
    pub needs_adaptation: bool,
}

impl IconAdaptation {
    /// Returns `Some(self.is_dark)` only when [`Self::needs_adaptation`] is `true`; otherwise `None`.
    #[must_use]
    pub const fn icon_is_dark_hint(&self) -> Option<bool> {
        if self.needs_adaptation {
            Some(self.is_dark)
        } else {
            None
        }
    }
}

/// Analyzes raw RGBA (`[r, g, b, a]`) pixels using ZTools's 1,600-point sampling heuristic.
#[must_use]
pub fn analyze_rgba_pixels(width: u32, height: u32, rgba: &[u8]) -> IconAdaptation {
    let total_pixels = (width as usize).saturating_mul(height as usize);
    let expected_bytes = total_pixels.saturating_mul(4);
    let usable_bytes = rgba.len().min(expected_bytes);
    if total_pixels == 0 || usable_bytes < 4 {
        return IconAdaptation::default();
    }

    let pixels = &rgba[..usable_bytes];
    let step = (total_pixels / TARGET_SAMPLES).max(1);
    let byte_stride = 4usize.saturating_mul(step);

    let mut color_counts: HashMap<(u8, u8, u8), usize> = HashMap::new();
    let mut opaque_pixels = 0usize;
    let mut total_sampled = 0usize;
    let mut main_rgb = (0u8, 0u8, 0u8);
    let mut max_count = 0usize;

    let mut idx = 0usize;
    while let Some(px) = pixels.get(idx..idx + 4) {
        total_sampled += 1;
        let a = px[3];
        if a > ALPHA_THRESHOLD {
            opaque_pixels += 1;
            let rgb = (px[0], px[1], px[2]);
            let count = color_counts.entry(rgb).or_insert(0);
            *count += 1;
            if *count > max_count {
                max_count = *count;
                main_rgb = rgb;
            }
        }
        idx = idx.saturating_add(byte_stride);
    }

    if opaque_pixels == 0 || total_sampled == 0 {
        return IconAdaptation::default();
    }

    let (main_r, main_g, main_b) = main_rgb;
    let mut similar_pixels = 0usize;
    let mut idx = 0usize;
    while let Some(px) = pixels.get(idx..idx + 4) {
        if px[3] > ALPHA_THRESHOLD {
            let dr = f64::from(px[0]) - f64::from(main_r);
            let dg = f64::from(px[1]) - f64::from(main_g);
            let db = f64::from(px[2]) - f64::from(main_b);
            let distance = (dr * dr + dg * dg + db * db).sqrt();
            if distance < COLOR_DISTANCE_THRESHOLD {
                similar_pixels += 1;
            }
        }
        idx = idx.saturating_add(byte_stride);
    }

    let transparency_ratio = (total_sampled - opaque_pixels) as f64 / total_sampled as f64;
    let similarity_ratio = similar_pixels as f64 / opaque_pixels as f64;
    let is_pure_color_icon = similarity_ratio > MIN_SIMILARITY_RATIO
        && transparency_ratio > MIN_TRANSPARENCY_RATIO
        && opaque_pixels > MIN_OPAQUE_PIXELS;

    if !is_pure_color_icon {
        return IconAdaptation::default();
    }

    let luminance =
        (0.299 * f64::from(main_r) + 0.587 * f64::from(main_g) + 0.114 * f64::from(main_b)) / 255.0;
    let is_dark = luminance < 0.5;
    let main_color = format!("#{main_r:02x}{main_g:02x}{main_b:02x}");

    IconAdaptation {
        is_simple_icon: true,
        main_color: Some(main_color),
        is_dark,
        needs_adaptation: true,
    }
}

/// Decodes encoded raster icon bytes (`png`, `jpeg`, `tiff`) and runs [`analyze_rgba_pixels`].
///
/// Corrupt, empty, or unsupported payloads safely return [`IconAdaptation::default`].
#[must_use]
pub fn analyze_icon_bytes(bytes: &[u8]) -> IconAdaptation {
    if bytes.is_empty() {
        return IconAdaptation::default();
    }
    let Ok(img) = image::load_from_memory(bytes) else {
        return IconAdaptation::default();
    };
    let rgba = img.to_rgba8();
    analyze_rgba_pixels(rgba.width(), rgba.height(), rgba.as_raw())
}

/// Reads an icon file from `path` and runs [`analyze_icon_bytes`].
///
/// Missing, unreadable, oversized (`> 16 MiB`), or non-raster files safely return
/// [`IconAdaptation::default`].
#[must_use]
pub fn analyze_icon_path(path: &Path) -> IconAdaptation {
    let Ok(meta) = std::fs::metadata(path) else {
        return IconAdaptation::default();
    };
    if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_ICON_FILE_BYTES {
        return IconAdaptation::default();
    }
    let Ok(bytes) = std::fs::read(path) else {
        return IconAdaptation::default();
    };
    analyze_icon_bytes(&bytes)
}

/// Decodes a `data:image/...;base64,...` icon URI and runs [`analyze_icon_bytes`].
///
/// Non-base64, non-image, SVG, or malformed data URIs safely return [`IconAdaptation::default`].
#[must_use]
pub fn analyze_icon_data_url(data_url: &str) -> IconAdaptation {
    let trimmed = data_url.trim();
    if !trimmed.starts_with("data:image/") {
        return IconAdaptation::default();
    }
    let Some((header, payload)) = trimmed.split_once(',') else {
        return IconAdaptation::default();
    };
    if !header.to_ascii_lowercase().contains(";base64") {
        return IconAdaptation::default();
    }
    let decoded = match BASE64_STANDARD.decode(payload.trim()) {
        Ok(bytes) => bytes,
        Err(_) => {
            let compact: String = payload
                .chars()
                .filter(|c| !c.is_ascii_whitespace())
                .collect();
            let Ok(bytes) = BASE64_STANDARD.decode(compact.as_bytes()) else {
                return IconAdaptation::default();
            };
            bytes
        }
    };
    analyze_icon_bytes(&decoded)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
    use tempfile::TempDir;

    fn encode_png(img: &RgbaImage) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(img.clone())
            .write_to(&mut buf, ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    fn silhouette_png(size: u32, fg: [u8; 4]) -> Vec<u8> {
        let mut img = RgbaImage::from_pixel(size, size, Rgba([0, 0, 0, 0]));
        let lo = size / 4;
        let hi = (size * 3) / 4;
        for y in lo..hi {
            for x in lo..hi {
                img.put_pixel(x, y, Rgba(fg));
            }
        }
        encode_png(&img)
    }

    #[test]
    fn dark_monochrome_png_needs_adaptation_and_is_dark() {
        let bytes = silhouette_png(32, [18, 22, 26, 255]);
        let res = analyze_icon_bytes(&bytes);
        assert_eq!(
            res,
            IconAdaptation {
                is_simple_icon: true,
                main_color: Some("#12161a".to_string()),
                is_dark: true,
                needs_adaptation: true,
            }
        );
        assert_eq!(res.icon_is_dark_hint(), Some(true));
    }

    #[test]
    fn light_monochrome_png_needs_adaptation_and_is_not_dark() {
        let bytes = silhouette_png(32, [242, 245, 248, 255]);
        let res = analyze_icon_bytes(&bytes);
        assert_eq!(
            res,
            IconAdaptation {
                is_simple_icon: true,
                main_color: Some("#f2f5f8".to_string()),
                is_dark: false,
                needs_adaptation: true,
            }
        );
        assert_eq!(res.icon_is_dark_hint(), Some(false));
    }

    #[test]
    fn gradient_icon_does_not_need_adaptation() {
        let mut img = RgbaImage::from_pixel(32, 32, Rgba([0, 0, 0, 0]));
        for y in 8..24 {
            for x in 8..24 {
                let r = ((x - 8) * 16).min(255) as u8;
                let b = ((y - 8) * 16).min(255) as u8;
                img.put_pixel(x, y, Rgba([r, 80, b, 255]));
            }
        }
        let res = analyze_icon_bytes(&encode_png(&img));
        assert_eq!(res, IconAdaptation::default());
        assert_eq!(res.icon_is_dark_hint(), None);
    }

    #[test]
    fn full_bleed_opaque_square_does_not_need_adaptation() {
        let img = RgbaImage::from_pixel(32, 32, Rgba([12, 12, 12, 255]));
        let res = analyze_icon_bytes(&encode_png(&img));
        assert_eq!(res, IconAdaptation::default());
        assert_eq!(res.icon_is_dark_hint(), None);
    }

    #[test]
    fn corrupt_bytes_and_empty_inputs_return_default_without_panic() {
        assert_eq!(analyze_icon_bytes(&[]), IconAdaptation::default());
        assert_eq!(
            analyze_icon_bytes(b"not-a-valid-png"),
            IconAdaptation::default()
        );
        assert_eq!(analyze_rgba_pixels(0, 0, &[]), IconAdaptation::default());
        assert_eq!(
            analyze_rgba_pixels(16, 16, &[0, 0, 0]),
            IconAdaptation::default()
        );
    }

    #[test]
    fn sampling_step_path_and_data_url_work_consistently() {
        let bytes = silhouette_png(128, [30, 30, 30, 255]);
        let from_bytes = analyze_icon_bytes(&bytes);
        assert!(from_bytes.needs_adaptation);
        assert!(from_bytes.is_dark);

        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("mono.png");
        std::fs::write(&file_path, &bytes).unwrap();
        assert_eq!(analyze_icon_path(&file_path), from_bytes);
        assert_eq!(
            analyze_icon_path(&dir.path().join("missing.png")),
            IconAdaptation::default()
        );

        let b64 = BASE64_STANDARD.encode(&bytes);
        let data_url = format!("data:image/png;base64,{b64}");
        assert_eq!(analyze_icon_data_url(&data_url), from_bytes);
        assert_eq!(
            analyze_icon_data_url("data:image/svg+xml;base64,PHN2Zz48L3N2Zz4="),
            IconAdaptation::default()
        );
    }
}

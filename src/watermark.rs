//! Image watermarking: stamp a short text label (a brand or site) onto an image
//! before it is uploaded. Each platform picks its own text (see the adapter),
//! e.g. a bare brand for stricter platforms and a full URL for lenient ones.

use ab_glyph::{FontVec, PxScale};
use anyhow::{anyhow, Context, Result};
use image::{Rgba, RgbaImage};
use imageproc::drawing::{draw_text_mut, text_size};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// System fonts to try, in order. The watermark text is Latin/ASCII, so a basic
/// sans-serif (bold preferred for legibility) is enough.
const FONT_CANDIDATES: &[&str] = &[
    "C:/Windows/Fonts/arialbd.ttf",
    "C:/Windows/Fonts/seguisb.ttf",
    "C:/Windows/Fonts/segoeui.ttf",
    "C:/Windows/Fonts/arial.ttf",
];

fn load_font() -> Result<FontVec> {
    for path in FONT_CANDIDATES {
        if let Ok(bytes) = std::fs::read(path) {
            if let Ok(font) = FontVec::try_from_vec(bytes) {
                return Ok(font);
            }
        }
    }
    Err(anyhow!("找不到可用的系统字体用于水印"))
}

/// Draw `text` as a bottom-right watermark on the image at `src`, writing the
/// result into `out_dir` and returning the new file's path. White text with a
/// dark halo so it stays readable on light or dark backgrounds. JPEG sources
/// stay JPEG; anything else is written as PNG (the `image` encoders we ship).
pub fn apply(src: &Path, out_dir: &Path, prefix: &str, text: &str) -> Result<PathBuf> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(src.to_path_buf());
    }
    let font = load_font()?;
    let dynimg = image::open(src).with_context(|| format!("打开图片 {}", src.display()))?;
    let mut img = dynimg.to_rgba8();
    let (w, h) = (img.width() as i32, img.height() as i32);

    // Scale the text to the image so it's visible but not overpowering.
    let scale_px = (img.height() as f32 * 0.032).clamp(18.0, 64.0);
    let scale = PxScale::from(scale_px);
    let (tw, th) = text_size(scale, &font, text);
    let (tw, th) = (tw as i32, th as i32);
    let pad = (scale_px * 0.6).round() as i32;
    let x = (w - tw - pad).max(pad);
    let y = (h - th - pad).max(pad);

    // Dark halo (offsets) under white text for contrast on any background.
    let halo = Rgba([0u8, 0, 0, 150]);
    for (dx, dy) in [
        (-1, 0),
        (1, 0),
        (0, -1),
        (0, 1),
        (1, 1),
        (-1, -1),
        (1, -1),
        (-1, 1),
    ] {
        draw_text_mut(&mut img, halo, x + dx, y + dy, scale, &font, text);
    }
    draw_text_mut(&mut img, Rgba([255u8, 255, 255, 235]), x, y, scale, &font, text);

    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("创建水印输出目录 {}", out_dir.display()))?;
    let stem = src
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or("image");
    let is_jpeg = matches!(
        src.extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("jpg") | Some("jpeg")
    );
    let dest = out_dir.join(format!("{prefix}_{stem}.{}", if is_jpeg { "jpg" } else { "png" }));
    save(&img, &dest, is_jpeg)?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_watermark_and_writes_jpeg() {
        let dir = std::env::temp_dir().join("auto_media_watermark_test");
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src.jpg");
        // JPEG has no alpha, so build the source as RGB (mirrors a real photo).
        image::RgbImage::from_pixel(640, 480, image::Rgb([40, 90, 160]))
            .save(&src)
            .unwrap();

        let out = apply(&src, &dir, "twitter_0", "https://blog.xiedeacc.com").unwrap();
        assert!(out.exists());
        assert_eq!(out.extension().and_then(|e| e.to_str()), Some("jpg"));
        // A real re-encode produces a non-empty file distinct from the source path.
        assert_ne!(out, src);
        assert!(std::fs::metadata(&out).unwrap().len() > 0);
    }

    #[test]
    fn empty_text_returns_source_unchanged() {
        let dir = std::env::temp_dir().join("auto_media_watermark_test");
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("src2.png");
        image::RgbaImage::from_pixel(10, 10, Rgba([0, 0, 0, 255]))
            .save(&src)
            .unwrap();
        assert_eq!(apply(&src, &dir, "x_0", "   ").unwrap(), src);
    }
}

fn save(img: &RgbaImage, dest: &Path, as_jpeg: bool) -> Result<()> {
    if as_jpeg {
        // JPEG has no alpha channel — flatten to RGB first.
        let rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
        rgb.save(dest)
            .with_context(|| format!("写入水印图片 {}", dest.display()))
    } else {
        img.save(dest)
            .with_context(|| format!("写入水印图片 {}", dest.display()))
    }
}

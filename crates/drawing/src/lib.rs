//! Scanned drawings as underlays.
//!
//! Archive drawings arrive as PDFs that wrap one scanned image per page. This crate finds that
//! image (lopdf parses the PDF objects; nothing is rendered), decodes it to greyscale and lets
//! callers rotate, crop and sample it. Vector PDFs are not handled yet.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::path::Path;

use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;

/// Largest PDF read.
pub const MAX_FILE_BYTES: u64 = 1 << 30;
/// Largest scan decoded, pixels.
pub const MAX_PIXELS: u64 = 400_000_000;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum DrawingError {
    #[error("{0}")]
    Io(String),
    #[error("not a readable PDF: {0}")]
    Pdf(String),
    #[error("not supported yet: {0}")]
    Unsupported(String),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, DrawingError>;

/// An 8-bit greyscale image, row-major.
#[derive(Debug, Clone, PartialEq)]
pub struct Gray {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Gray {
    pub fn get(&self, x: u32, y: u32) -> u8 {
        self.data.get(y as usize * self.width as usize + x as usize).copied().unwrap_or(255)
    }

    /// Bilinear sample at pixel coordinates (pixel centres at +0.5); `None` outside.
    pub fn sample(&self, x: f64, y: f64) -> Option<u8> {
        let (x, y) = (x - 0.5, y - 0.5);
        if !(x >= 0.0 && y >= 0.0 && x <= f64::from(self.width) - 1.0 && y <= f64::from(self.height) - 1.0) {
            return None;
        }
        let (ix, iy) = (x.floor() as u32, y.floor() as u32);
        let (fx, fy) = (x - x.floor(), y - y.floor());
        let (jx, jy) = ((ix + 1).min(self.width - 1), (iy + 1).min(self.height - 1));
        let v = f64::from(self.get(ix, iy)) * (1.0 - fx) * (1.0 - fy)
            + f64::from(self.get(jx, iy)) * fx * (1.0 - fy)
            + f64::from(self.get(ix, jy)) * (1.0 - fx) * fy
            + f64::from(self.get(jx, jy)) * fx * fy;
        Some(v.round().clamp(0.0, 255.0) as u8)
    }

    /// The paper's brightness around each point: the `pct` percentile of blocks of `block` pixels,
    /// as a small image whose pixel (i, j) covers the block starting at (i·block, j·block).
    pub fn background(&self, block: u32, pct: f64) -> Gray {
        let block = block.max(4);
        let (bw, bh) = (self.width.div_ceil(block), self.height.div_ceil(block));
        let mut data = Vec::with_capacity((bw * bh) as usize);
        let mut hist = [0u32; 256];
        for j in 0..bh {
            for i in 0..bw {
                hist.iter_mut().for_each(|h| *h = 0);
                let mut n = 0u32;
                for y in (j * block..((j + 1) * block).min(self.height)).step_by(2) {
                    for x in (i * block..((i + 1) * block).min(self.width)).step_by(2) {
                        hist[self.get(x, y) as usize] += 1;
                        n += 1;
                    }
                }
                let target = (f64::from(n) * pct.clamp(0.0, 1.0)) as u32;
                let mut acc = 0;
                let mut v = 255u8;
                for (g, c) in hist.iter().enumerate() {
                    acc += c;
                    if acc > target {
                        v = g as u8;
                        break;
                    }
                }
                data.push(v);
            }
        }
        Gray { width: bw, height: bh, data }
    }

    /// Rotated clockwise by `degrees` (0, 90, 180 or 270).
    pub fn rotated(self, degrees: u32) -> Result<Gray> {
        let (w, h) = (self.width, self.height);
        let at = |x: u32, y: u32| self.get(x, y);
        let (nw, nh, f): (u32, u32, Box<dyn Fn(u32, u32) -> u8>) = match degrees {
            0 => return Ok(self),
            90 => (h, w, Box::new(move |x, y| at(y, h - 1 - x))),
            180 => (w, h, Box::new(move |x, y| at(w - 1 - x, h - 1 - y))),
            270 => (h, w, Box::new(move |x, y| at(w - 1 - y, x))),
            _ => return Err(DrawingError::Invalid(format!("rotation must be 0, 90, 180 or 270 (got {degrees})"))),
        };
        let mut data = Vec::with_capacity(nw as usize * nh as usize);
        for y in 0..nh {
            for x in 0..nw {
                data.push(f(x, y));
            }
        }
        Ok(Gray { width: nw, height: nh, data })
    }
}

fn load(path: &Path) -> Result<lopdf::Document> {
    let meta = std::fs::metadata(path).map_err(|e| DrawingError::Io(format!("{}: {e}", path.display())))?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(DrawingError::Invalid(format!("{}: larger than {} MB", path.display(), MAX_FILE_BYTES >> 20)));
    }
    lopdf::Document::load(path).map_err(|e| DrawingError::Pdf(e.to_string()))
}

/// Number of pages.
pub fn page_count(path: &Path) -> Result<u32> {
    Ok(load(path)?.get_pages().len() as u32)
}

/// The largest scanned image on a page (1-based), decoded to greyscale, as stored (unrotated).
pub fn page_image(path: &Path, page: u32) -> Result<Gray> {
    let doc = load(path)?;
    let pages = doc.get_pages();
    let id = *pages.get(&page).ok_or_else(|| DrawingError::Invalid(format!("page {page} doesn't exist (the PDF has {} pages)", pages.len())))?;
    let images = doc.get_page_images(id).map_err(|e| DrawingError::Pdf(e.to_string()))?;
    let img = images
        .into_iter()
        .max_by_key(|i| i.width.saturating_mul(i.height))
        .ok_or_else(|| DrawingError::Unsupported(format!("page {page} has no scanned image (vector drawings aren't read yet)")))?;
    let filters = img.filters.clone().unwrap_or_default();
    if filters.iter().map(String::as_str).ne(["DCTDecode"]) {
        return Err(DrawingError::Unsupported(format!("page {page}: image compression {filters:?} (only JPEG scans are read so far)")));
    }
    if img.width <= 0 || img.height <= 0 || (img.width as u64) * (img.height as u64) > MAX_PIXELS {
        return Err(DrawingError::Invalid(format!("page {page}: image of {} × {} px is out of range", img.width, img.height)));
    }
    let opts = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::Luma).set_max_width(65535).set_max_height(65535);
    let mut dec = zune_jpeg::JpegDecoder::new_with_options(zune_core::bytestream::ZCursor::new(img.content), opts);
    let data = dec.decode().map_err(|e| DrawingError::Pdf(format!("page {page}: JPEG: {e:?}")))?;
    let info = dec.info().ok_or_else(|| DrawingError::Pdf(format!("page {page}: JPEG without a header")))?;
    let (w, h) = (u32::from(info.width), u32::from(info.height));
    if data.len() != w as usize * h as usize {
        return Err(DrawingError::Pdf(format!("page {page}: decoded {} bytes for {w} × {h} px", data.len())));
    }
    Ok(Gray { width: w, height: h, data })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g() -> Gray {
        // 3 × 2:  0 1 2 / 3 4 5
        Gray { width: 3, height: 2, data: vec![0, 1, 2, 3, 4, 5] }
    }

    #[test]
    fn rotations() {
        let r = g().rotated(90).unwrap();
        assert_eq!((r.width, r.height), (2, 3));
        assert_eq!(r.data, vec![3, 0, 4, 1, 5, 2]);
        assert_eq!(g().rotated(180).unwrap().data, vec![5, 4, 3, 2, 1, 0]);
        assert_eq!(g().rotated(270).unwrap().data, vec![2, 5, 1, 4, 0, 3]);
        assert!(g().rotated(45).is_err());
    }

    #[test]
    fn bilinear_sampling() {
        let i = g();
        assert_eq!(i.sample(0.5, 0.5), Some(0));
        assert_eq!(i.sample(1.0, 1.0), Some(2)); // between 0, 1, 3, 4
        assert_eq!(i.sample(-1.0, 0.0), None);
    }

    #[test]
    fn non_pdf_input_is_refused() {
        let dir = std::env::temp_dir().join(format!("trigon-drawing-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("x.pdf");
        std::fs::write(&p, b"not a pdf").unwrap();
        assert!(page_image(&p, 1).is_err());
        assert!(page_image(&dir.join("missing.pdf"), 1).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}

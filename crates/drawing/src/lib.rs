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
    if img.width <= 0 || img.height <= 0 || (img.width as u64).checked_mul(img.height as u64).is_none_or(|n| n > MAX_PIXELS) {
        return Err(DrawingError::Invalid(format!("page {page}: image of {} × {} px is out of range", img.width, img.height)));
    }
    let filters = img.filters.clone().unwrap_or_default();
    if filters.iter().map(String::as_str).eq(["CCITTFaxDecode"]) {
        return group4_image(&doc, &img, page);
    }
    if filters.iter().map(String::as_str).ne(["DCTDecode"]) {
        return Err(DrawingError::Unsupported(format!(
            "page {page}: image compression {filters:?} (supported: JPEG and CCITT Group 4 grayscale scans)"
        )));
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

/// Decode the archive's original one-bit scan without resampling or a lossy conversion.
fn group4_image(doc: &lopdf::Document, img: &lopdf::xobject::PdfImage<'_>, page: u32) -> Result<Gray> {
    let invalid = |message: &str| DrawingError::Invalid(format!("page {page}: CCITT {message}"));
    let unsupported = |message: &str| DrawingError::Unsupported(format!("page {page}: CCITT {message}"));
    if img.bits_per_component != Some(1) || img.color_space.as_deref() != Some("DeviceGray") {
        return Err(unsupported("requires 1-bit DeviceGray (indexed palettes and masks are not handled)"));
    }
    let width = u16::try_from(img.width).map_err(|_| invalid("width exceeds 65535 pixels"))?;
    let height = u16::try_from(img.height).map_err(|_| invalid("height exceeds 65535 pixels"))?;
    if img.content.is_empty() {
        return Err(invalid("empty compressed data"));
    }
    let object = img.origin_dict.get(b"DecodeParms").map_err(|_| unsupported("Group 4 needs DecodeParms with K=-1"))?;
    let (_, object) = doc.dereference(object).map_err(|_| invalid("unreadable decode parameters"))?;
    let object = if let lopdf::Object::Array(a) = object {
        if a.len() != 1 {
            return Err(invalid("one filter requires one decode-parameter entry"));
        }
        a.first().ok_or_else(|| invalid("empty decode parameters"))?
    } else {
        object
    };
    let (_, object) = doc.dereference(object).map_err(|_| invalid("unreadable decode parameters"))?;
    let parms = object.as_dict().map_err(|_| invalid("decode parameters must be a dictionary"))?;
    let integer = |name: &[u8], default: i64| -> Result<i64> {
        match parms.get(name) {
            Ok(v) => v.as_i64().map_err(|_| invalid("non-integer decode parameter")),
            Err(_) => Ok(default),
        }
    };
    let boolean = |name: &[u8], default: bool| -> Result<bool> {
        match parms.get(name) {
            Ok(v) => v.as_bool().map_err(|_| invalid("non-boolean decode parameter")),
            Err(_) => Ok(default),
        }
    };
    if integer(b"K", 0)? >= 0 {
        return Err(unsupported("only Group 4 (negative K) is supported"));
    }
    let rows = integer(b"Rows", 0)?;
    if integer(b"Columns", 1728)? != img.width || (rows != 0 && rows != img.height) {
        return Err(invalid("Columns/Rows differ from image dimensions"));
    }
    if boolean(b"EncodedByteAlign", false)? || boolean(b"EndOfLine", false)? {
        return Err(unsupported("byte-aligned rows or end-of-line markers are not handled"));
    }
    let black_is_one = boolean(b"BlackIs1", false)?;
    let reverse = match img.origin_dict.get(b"Decode") {
        Err(_) => false,
        Ok(value) => {
            let (_, value) = doc.dereference(value).map_err(|_| invalid("unreadable Decode array"))?;
            let values = value.as_array().map_err(|_| invalid("Decode must be [0,1] or [1,0]"))?;
            let pair = values
                .iter()
                .map(lopdf::Object::as_float)
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|_| invalid("non-numeric Decode array"))?;
            match pair.as_slice() {
                [0.0, 1.0] => false,
                [1.0, 0.0] => true,
                _ => return Err(unsupported("Decode must be [0,1] or [1,0]")),
            }
        }
    };
    let expected = usize::from(width) * usize::from(height);
    let mut data = Vec::with_capacity(expected);
    let pixel = |color| {
        let black = color == fax::Color::Black;
        if black ^ black_is_one ^ reverse { 0 } else { 255 }
    };
    if boolean(b"EndOfBlock", true)? {
        // The codec accepts EOF as an end marker and pads trailing white rows. Require the
        // PDF's default EOFB marker so a truncated stream cannot silently become white paper.
        let tail = img.content.iter().rev().take(4).enumerate().fold(0_u32, |v, (i, b)| v | (u32::from(*b) << (i * 8)));
        if img.content.len() < 3 || !(0..8).any(|padding| (tail >> padding) & 0x00ff_ffff == 0x001001) {
            return Err(invalid("missing Group 4 end-of-block marker"));
        }
        if fax::decoder::decode_g4(img.content.iter().copied(), width, Some(height), |line| {
            data.extend(fax::decoder::pels(line, width).map(pixel));
        })
        .is_none()
        {
            return Err(invalid("malformed Group 4 data"));
        }
    } else {
        // With EndOfBlock=false the row count terminates the stream; require every row,
        // rather than accepting the convenience decoder's white padding on early EOF.
        let reader = img.content.iter().copied().map(Ok::<u8, std::convert::Infallible>);
        let mut decoder = fax::decoder::Group4Decoder::new(reader, width).map_err(|_| invalid("unreadable Group 4 data"))?;
        for _ in 0..height {
            if decoder.advance().map_err(|_| invalid("malformed Group 4 row"))? == fax::decoder::DecodeStatus::End {
                return Err(invalid("Group 4 data ended before the declared row count"));
            }
            data.extend(fax::decoder::pels(decoder.transition(), width).map(pixel));
        }
    }
    if data.len() != expected {
        return Err(invalid("decoded Group 4 dimensions differ from the image"));
    }
    Ok(Gray { width: u32::from(width), height: u32::from(height), data })
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
    fn fax_document(black_is_one: bool, reverse: bool) -> (lopdf::Document, lopdf::ObjectId, Vec<u8>) {
        use lopdf::{Stream, dictionary};
        let width = 13usize;
        let height = 5usize;
        let pixels: Vec<u8> = (0..width * height).map(|i| if (i / width + i % width).is_multiple_of(4) { 0 } else { 255 }).collect();
        let mut encoder = fax::encoder::Encoder::new(fax::VecWriter::new());
        for row in pixels.chunks(width) {
            encoder.encode_line(row.iter().map(|p| if *p == 0 { fax::Color::Black } else { fax::Color::White }), width as u16).unwrap();
        }
        let encoded = encoder.finish().unwrap().finish();
        let mut doc = lopdf::Document::with_version("1.4");
        let pages = doc.new_object_id();
        let mut image = dictionary! {"Type"=>"XObject","Subtype"=>"Image","Width"=>width as i64,"Height"=>height as i64,
        "BitsPerComponent"=>1,"ColorSpace"=>"DeviceGray","Filter"=>"CCITTFaxDecode",
        "DecodeParms"=>dictionary!{"K"=>-1,"Columns"=>width as i64,"Rows"=>height as i64,"BlackIs1"=>black_is_one}};
        if reverse {
            image.set("Decode", vec![1.into(), 0.into()]);
        }
        let image_id = doc.add_object(Stream::new(image, encoded));
        let content = doc.add_object(Stream::new(dictionary! {}, b"q 13 0 0 5 0 0 cm /Scan Do Q".to_vec()));
        let page = doc.add_object(dictionary! {"Type"=>"Page","Parent"=>pages,"MediaBox"=>vec![0.into(),0.into(),13.into(),5.into()],
        "Resources"=>dictionary!{"XObject"=>dictionary!{"Scan"=>image_id}},"Contents"=>content});
        doc.objects.insert(pages, dictionary! {"Type"=>"Pages","Kids"=>vec![page.into()],"Count"=>1}.into());
        let catalog = doc.add_object(dictionary! {"Type"=>"Catalog","Pages"=>pages});
        doc.trailer.set("Root", catalog);
        (doc, image_id, pixels)
    }

    #[test]
    fn ccitt_group4_pages_preserve_non_byte_aligned_pixels_and_pdf_polarity() {
        for black_is_one in [false, true] {
            for reverse in [false, true] {
                let (mut doc, _, pixels) = fax_document(black_is_one, reverse);
                let path = std::env::temp_dir().join(format!("trigon-fax-{}-{black_is_one}-{reverse}.pdf", std::process::id()));
                doc.save(&path).unwrap();
                let gray = page_image(&path, 1).unwrap();
                assert_eq!((gray.width, gray.height), (13, 5));
                let expected: Vec<_> = pixels.into_iter().map(|p| if black_is_one ^ reverse { 255 - p } else { p }).collect();
                assert_eq!(gray.data, expected);
                std::fs::remove_file(path).unwrap();
            }
        }
    }
    #[test]
    fn ccitt_hostile_dimensions_parameters_and_data_return_errors() {
        for scenario in 0..10 {
            let (mut doc, id, _) = fax_document(false, false);
            let stream = doc.get_object_mut(id).unwrap().as_stream_mut().unwrap();
            match scenario {
                0 => stream.dict.set("Width", i64::MAX),
                1 => stream.dict.set("Height", i64::MAX),
                2 => stream.dict.get_mut(b"DecodeParms").unwrap().as_dict_mut().unwrap().set("Columns", 14),
                3 => stream.dict.get_mut(b"DecodeParms").unwrap().as_dict_mut().unwrap().set("Rows", 6),
                4 => stream.dict.get_mut(b"DecodeParms").unwrap().as_dict_mut().unwrap().set("K", 0),
                5 => stream.dict.set("BitsPerComponent", 8),
                6 => stream.dict.get_mut(b"DecodeParms").unwrap().as_dict_mut().unwrap().set("EncodedByteAlign", true),
                7 => stream.dict.set("Decode", vec![0.into(), 0.into()]),
                8 => stream.content.clear(),
                _ => stream.content = vec![0],
            }
            let path = std::env::temp_dir().join(format!("trigon-fax-hostile-{}-{scenario}.pdf", std::process::id()));
            doc.save(&path).unwrap();
            let result = std::panic::catch_unwind(|| page_image(&path, 1));
            assert!(result.is_ok(), "scenario {scenario} panicked");
            assert!(result.unwrap().is_err(), "scenario {scenario} must be refused");
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn ccitt_accepts_single_filter_arrays_and_indirect_decode_parameters() {
        let (mut doc, id, pixels) = fax_document(false, false);
        let mut parms = doc.get_object(id).unwrap().as_stream().unwrap().dict.get(b"DecodeParms").unwrap().clone();
        parms.as_dict_mut().unwrap().set("EndOfBlock", false);
        let parms_id = doc.add_object(parms);
        let stream = doc.get_object_mut(id).unwrap().as_stream_mut().unwrap();
        stream.dict.set("Filter", vec![lopdf::Object::Name(b"CCITTFaxDecode".to_vec())]);
        stream.dict.set("DecodeParms", vec![lopdf::Object::Reference(parms_id)]);
        let path = std::env::temp_dir().join(format!("trigon-fax-indirect-{}.pdf", std::process::id()));
        doc.save(&path).unwrap();
        assert_eq!(page_image(&path, 1).unwrap().data, pixels);
        std::fs::remove_file(path).unwrap();
    }
}

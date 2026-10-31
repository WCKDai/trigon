//! Skråfoto: calibrated oblique aerial photos, read tile by tile from Cloud-Optimized GeoTIFFs.
//!
//! The photo's STAC item carries a full photogrammetric camera (perspective centre, rotation
//! matrix, focal length, pixel size), so projecting a point onto the photo is the collinearity
//! equation; no structure-from-motion is needed. Only the tiles covering the requested pixel
//! window are downloaded, with HTTP range requests.

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Seek, SeekFrom};

use serde::Serialize;

use crate::{DataError, Result};

/// A calibrated frame camera (EPSG:25832 metres, DVR90 heights).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Camera {
    pub center: [f64; 3],
    /// Row-major; camera coordinates are `R · (X − center)`.
    pub rotation: [f64; 9],
    pub focal_mm: f64,
    pub pixel_mm: f64,
    pub width: f64,
    pub height: f64,
    pub principal_offset_mm: [f64; 2],
}

impl Camera {
    /// Pixel (column, row) of a world point, or `None` if it is behind the camera.
    pub fn project(&self, p: [f64; 3]) -> Option<[f64; 2]> {
        let d = [p[0] - self.center[0], p[1] - self.center[1], p[2] - self.center[2]];
        let r = &self.rotation;
        let c = [r[0] * d[0] + r[1] * d[1] + r[2] * d[2], r[3] * d[0] + r[4] * d[1] + r[5] * d[2], r[6] * d[0] + r[7] * d[1] + r[8] * d[2]];
        if !(c[2] < -1e-6) {
            return None;
        }
        let x = -self.focal_mm * c[0] / c[2] + self.principal_offset_mm[0];
        let y = -self.focal_mm * c[1] / c[2] + self.principal_offset_mm[1];
        Some([self.width / 2.0 + x / self.pixel_mm, self.height / 2.0 - y / self.pixel_mm])
    }

    pub fn contains(&self, px: [f64; 2]) -> bool {
        px[0] >= 0.0 && px[1] >= 0.0 && px[0] < self.width && px[1] < self.height
    }
}

/// Reads a camera from a Skråfoto STAC item's properties.
pub fn camera_from_item(props: &serde_json::Value) -> Option<Camera> {
    let nums = |v: &serde_json::Value| -> Option<Vec<f64>> { v.as_array()?.iter().map(serde_json::Value::as_f64).collect() };
    let center = nums(props.get("pers:perspective_center")?)?;
    let rot = nums(props.get("pers:rotation_matrix")?)?;
    let io = props.get("pers:interior_orientation")?;
    let sensor = nums(io.get("sensor_array_dimensions")?)?;
    let pixel = nums(io.get("pixel_spacing")?)?;
    let ppo = io.get("principal_point_offset").and_then(nums).unwrap_or_else(|| vec![0.0, 0.0]);
    let cam = Camera {
        center: [*center.first()?, *center.get(1)?, *center.get(2)?],
        rotation: rot.try_into().ok()?,
        focal_mm: io.get("focal_length")?.as_f64()?,
        pixel_mm: *pixel.first()?,
        width: *sensor.first()?,
        height: *sensor.get(1)?,
        principal_offset_mm: [*ppo.first()?, *ppo.get(1)?],
    };
    let finite = cam.center.iter().chain(&cam.rotation).all(|v| v.is_finite());
    (finite && cam.focal_mm > 0.0 && cam.pixel_mm > 0.0 && cam.width >= 1.0 && cam.height >= 1.0).then_some(cam)
}

/// An RGB pixel window cut from a photo.
#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    pub x0: u32,
    pub y0: u32,
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

impl Region {
    /// Bilinear sample at photo pixel coordinates; `None` outside the window.
    pub fn sample(&self, px: [f64; 2]) -> Option<[u8; 3]> {
        let x = px[0] - f64::from(self.x0) - 0.5;
        let y = px[1] - f64::from(self.y0) - 0.5;
        if !(x >= 0.0 && y >= 0.0 && x < f64::from(self.width) - 1.0 && y < f64::from(self.height) - 1.0) {
            return None;
        }
        let (ix, iy) = (x.floor() as usize, y.floor() as usize);
        let (fx, fy) = (x - x.floor(), y - y.floor());
        let w = self.width as usize;
        let at = |i: usize, j: usize, k: usize| f64::from(self.rgb.get((j * w + i) * 3 + k).copied().unwrap_or(0));
        let mut out = [0u8; 3];
        for (k, o) in out.iter_mut().enumerate() {
            let v = at(ix, iy, k) * (1.0 - fx) * (1.0 - fy)
                + at(ix + 1, iy, k) * fx * (1.0 - fy)
                + at(ix, iy + 1, k) * (1.0 - fx) * fy
                + at(ix + 1, iy + 1, k) * fx * fy;
            *o = v.round().clamp(0.0, 255.0) as u8;
        }
        Some(out)
    }
}

/// Largest window cut from one photo, pixels per side.
pub const MAX_REGION: u32 = 4096;
const BLOCK: u64 = 256 * 1024;
const MAX_REQUESTS: usize = 400;

/// `Read + Seek` over HTTP range requests, cached in 256 KiB blocks.
pub(crate) struct RangeReader<'a> {
    agent: &'a ureq::Agent,
    url: String,
    len: u64,
    pos: u64,
    blocks: BTreeMap<u64, Vec<u8>>,
    requests: usize,
}

fn io_err(msg: &str) -> std::io::Error {
    std::io::Error::other(msg.to_string())
}

impl<'a> RangeReader<'a> {
    pub(crate) fn open(agent: &'a ureq::Agent, url: String) -> Result<Self> {
        let mut r = Self { agent, url, len: 0, pos: 0, blocks: BTreeMap::new(), requests: 0 };
        let (total, first) = r.fetch(0, BLOCK - 1).map_err(|_| DataError::Unavailable("Skråfoto image could not be read".into()))?;
        r.len = total;
        r.blocks.insert(0, first);
        Ok(r)
    }

    /// One range request; returns (total file length, bytes).
    fn fetch(&mut self, start: u64, end: u64) -> std::io::Result<(u64, Vec<u8>)> {
        self.requests += 1;
        if self.requests > MAX_REQUESTS {
            return Err(io_err("too many range requests for one photo"));
        }
        let mut resp =
            self.agent.get(&self.url).header("Range", &format!("bytes={start}-{end}")).call().map_err(|_| io_err("Skråfoto range request failed"))?;
        if resp.status() != 206 {
            return Err(io_err("Skråfoto server ignored the range request"));
        }
        let total = resp
            .headers()
            .get("content-range")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit('/').next())
            .and_then(|v| v.parse::<u64>().ok())
            .ok_or_else(|| io_err("no Content-Range"))?;
        let bytes = resp.body_mut().with_config().limit(BLOCK * 2).read_to_vec().map_err(|_| io_err("range body"))?;
        Ok((total, bytes))
    }

    fn block(&mut self, i: u64) -> std::io::Result<&[u8]> {
        if !self.blocks.contains_key(&i) {
            let start = i * BLOCK;
            let end = (start + BLOCK - 1).min(self.len.saturating_sub(1));
            let (_, bytes) = self.fetch(start, end)?;
            self.blocks.insert(i, bytes);
        }
        self.blocks.get(&i).map(Vec::as_slice).ok_or_else(|| io_err("block"))
    }
}

impl Read for RangeReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pos >= self.len || buf.is_empty() {
            return Ok(0);
        }
        let (i, off) = (self.pos / BLOCK, (self.pos % BLOCK) as usize);
        let block = self.block(i)?;
        let n = block.len().saturating_sub(off).min(buf.len());
        if n == 0 {
            return Ok(0);
        }
        buf[..n].copy_from_slice(&block[off..off + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for RangeReader<'_> {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let p = match to {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::End(d) => self.len.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        self.pos = p.ok_or_else(|| io_err("seek out of range"))?;
        Ok(self.pos)
    }
}

/// Cuts the pixel window `[x0, x1) × [y0, y1)` from the full-resolution image.
pub(crate) fn read_region<R: Read + Seek>(reader: R, x0: u32, y0: u32, x1: u32, y1: u32) -> Result<Region> {
    let bad = |what: &str| DataError::BadResponse(format!("Skråfoto image ({what})"));
    let mut dec = tiff::decoder::Decoder::new(reader).map_err(|_| bad("not a TIFF"))?;
    let (w, h) = dec.dimensions().map_err(|_| bad("size"))?;
    let (x1, y1) = (x1.min(w), y1.min(h));
    if x0 >= x1 || y0 >= y1 {
        return Err(DataError::NotFound("the facade is outside the photo".into()));
    }
    if x1 - x0 > MAX_REGION || y1 - y0 > MAX_REGION {
        return Err(DataError::Unavailable(format!("the facade covers more than {MAX_REGION} px of the photo")));
    }
    if dec.get_chunk_type() != tiff::decoder::ChunkType::Tile {
        return Err(bad("not tiled"));
    }
    let (tw, th) = dec.chunk_dimensions();
    if tw == 0 || th == 0 {
        return Err(bad("tile size"));
    }
    // Skråfoto tiles are JPEG in YCbCr; the decoder hands those values back unconverted.
    let ycbcr = matches!(dec.colortype(), Ok(tiff::ColorType::YCbCr(8)));
    let across = w.div_ceil(tw);
    let (rw, rh) = (x1 - x0, y1 - y0);
    let mut rgb = vec![0u8; (rw * rh * 3) as usize];
    for ty in y0 / th..=(y1 - 1) / th {
        for tx in x0 / tw..=(x1 - 1) / tw {
            let data = match dec.read_chunk(ty * across + tx).map_err(|_| bad("tile"))? {
                tiff::decoder::DecodingResult::U8(v) => v,
                _ => return Err(bad("pixel format")),
            };
            let (dw, dh) = dec.chunk_data_dimensions(ty * across + tx);
            let channels = data.len() / (dw as usize * dh as usize).max(1);
            if channels < 3 {
                return Err(bad("channels"));
            }
            for j in 0..dh {
                let gy = ty * th + j;
                if gy < y0 || gy >= y1 {
                    continue;
                }
                for i in 0..dw {
                    let gx = tx * tw + i;
                    if gx < x0 || gx >= x1 {
                        continue;
                    }
                    let s = ((j * dw + i) as usize) * channels;
                    let d = (((gy - y0) * rw + (gx - x0)) as usize) * 3;
                    if let (Some(src), Some(dst)) = (data.get(s..s + 3), rgb.get_mut(d..d + 3)) {
                        let px = if ycbcr { ycbcr_to_rgb([src[0], src[1], src[2]]) } else { [src[0], src[1], src[2]] };
                        dst.copy_from_slice(&px);
                    }
                }
            }
        }
    }
    Ok(Region { x0, y0, width: rw, height: rh, rgb })
}

/// ITU-R BT.601 full-range YCbCr (JFIF) to RGB.
pub(crate) fn ycbcr_to_rgb(p: [u8; 3]) -> [u8; 3] {
    let (y, cb, cr) = (f64::from(p[0]), f64::from(p[1]) - 128.0, f64::from(p[2]) - 128.0);
    let c = |v: f64| v.round().clamp(0.0, 255.0) as u8;
    [c(y + 1.402 * cr), c(y - 0.344_136 * cb - 0.714_136 * cr), c(y + 1.772 * cb)]
}

/// The same, from bytes in memory (tests).
#[allow(dead_code)]
pub(crate) fn read_region_bytes(bytes: Vec<u8>, x0: u32, y0: u32, x1: u32, y1: u32) -> Result<Region> {
    read_region(Cursor::new(bytes), x0, y0, x1, y1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The camera of skraafotos2025 item 2025_84_40_2_0050_00002394_100mm (looking north).
    fn north_camera() -> Camera {
        Camera {
            center: [722458.43, 6176784.18, 2202.17],
            rotation: [
                0.999992778848224,
                -0.003519852800858008,
                -0.0014327901686364518,
                0.0035032139084591497,
                0.7076598295012541,
                0.7065446151536132,
                -0.0014730049961312447,
                -0.7065445324581573,
                0.7076670501795072,
            ],
            focal_mm: 123.38,
            pixel_mm: 0.00376,
            width: 14144.0,
            height: 10560.0,
            principal_offset_mm: [0.0, 0.0],
        }
    }

    #[test]
    fn frederikssundsvej_projects_into_the_north_looking_photo() {
        let c = north_camera();
        let px = c.project([721989.65, 6178579.47, 18.78]).unwrap();
        assert!(c.contains(px), "{px:?}");
        // Higher points appear higher in a north-looking oblique (smaller row), further north too.
        let up = c.project([721989.65, 6178579.47, 38.78]).unwrap();
        assert!(up[1] < px[1]);
        let behind = c.project([722458.43, 6176784.18, 3000.0]);
        assert!(behind.is_none());
    }

    #[test]
    fn ycbcr_converts_to_rgb() {
        assert_eq!(ycbcr_to_rgb([128, 128, 128]), [128, 128, 128]);
        assert_eq!(ycbcr_to_rgb([76, 85, 255]), [254, 0, 0]);
    }

    #[test]
    fn region_sampling_is_bilinear_and_bounded() {
        let r = Region { x0: 10, y0: 20, width: 2, height: 2, rgb: vec![0, 0, 0, 100, 100, 100, 0, 0, 0, 100, 100, 100] };
        assert_eq!(r.sample([11.0, 21.0]), Some([50, 50, 50]));
        assert_eq!(r.sample([5.0, 5.0]), None);
    }
}

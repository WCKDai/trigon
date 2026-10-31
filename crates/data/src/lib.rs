//! Data providers: where a building's address, footprint, heights and photos come from.
//!
//! Danish public data first: Dataforsyningen's GSearch (addresses), Danmarks Højdemodel (DSM and
//! DTM) and Skråfoto (oblique photos). OpenStreetMap is the fallback without credentials and the
//! footprint source until GeoDanmark is wired in.
//!
//! Credentials come from the environment (`TRIGON_DATAFORSYNINGEN_TOKEN` or
//! `TRIGON_DATAFORSYNINGEN_TOKEN_FILE`; the same for `TRIGON_DATAFORDELER_KEY`). They are never
//! stored in a document, and error messages never contain a URL: the services echo tokens back
//! in their URLs and responses.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::collections::BTreeMap;
use std::io::Cursor;
use std::time::Duration;

use serde_json::Value;
use trigon_model::{Evidence, GeoPoint};

pub mod coords;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum DataError {
    #[error("{0}")]
    Unavailable(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("unexpected answer from {0}")]
    BadResponse(String),
}

pub type Result<T> = std::result::Result<T, DataError>;

#[derive(Debug, Clone, PartialEq)]
pub struct Address {
    pub text: String,
    /// DAR address id when the address came from Danish data.
    pub id: Option<String>,
    /// ETRS89 / UTM 32N, metres.
    pub utm: [f64; 2],
    pub wgs: GeoPoint,
    pub evidence: Evidence,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Footprint {
    /// Outline in ETRS89 / UTM 32N, metres.
    pub ring_utm: Vec<[f64; 2]>,
    pub tags: BTreeMap<String, String>,
    pub evidence: Evidence,
}

/// Heights measured from the elevation model inside a footprint, metres.
#[derive(Debug, Clone, PartialEq)]
pub struct Heights {
    /// Median terrain height (DVR90).
    pub terrain: f64,
    /// Median of surface minus terrain: roughly the eaves or flat-roof height.
    pub median: f64,
    /// 95th percentile of surface minus terrain: roughly the ridge.
    pub top: f64,
    pub samples: usize,
    pub evidence: Evidence,
}

/// An oblique or nadir photo covering a point. Only ids are kept: image URLs carry the token
/// and are rebuilt when needed.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Photo {
    pub id: String,
    pub collection: String,
    /// `north`, `south`, `east`, `west` (the direction the camera looks) or `nadir`.
    pub direction: String,
    pub datetime: String,
}

pub trait Provider: Send {
    fn geocode(&self, query: &str) -> Result<Address>;
    fn footprint(&self, at: &Address) -> Result<Footprint>;
    fn heights(&self, ring_utm: &[[f64; 2]]) -> Result<Heights>;
    fn photos(&self, at_utm: [f64; 2]) -> Result<Vec<Photo>>;
}

#[derive(Clone, Default)]
pub struct Credentials {
    pub dataforsyningen: Option<String>,
    pub datafordeler: Option<String>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("dataforsyningen", &self.dataforsyningen.as_ref().map(|_| "set"))
            .field("datafordeler", &self.datafordeler.as_ref().map(|_| "set"))
            .finish()
    }
}

fn secret_from_env(name: &str) -> Option<String> {
    let direct = std::env::var(name).ok();
    let from_file = std::env::var(format!("{name}_FILE")).ok().and_then(|p| std::fs::read_to_string(p).ok());
    direct.or(from_file).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

impl Credentials {
    pub fn from_env() -> Self {
        Self { dataforsyningen: secret_from_env("TRIGON_DATAFORSYNINGEN_TOKEN"), datafordeler: secret_from_env("TRIGON_DATAFORDELER_KEY") }
    }
}

const USER_AGENT: &str = "Trigon/0.1 (+https://github.com/WCKDai/trigon)";
const MAX_BODY: u64 = 64 * 1024 * 1024;
const SKRAAFOTO_COLLECTIONS: [&str; 3] = ["skraafotos2025", "skraafotos2023", "skraafotos2021"];
const SDFI: &str = "Indeholder data fra Styrelsen for Dataforsyning og Infrastruktur";
const OSM: &str = "© OpenStreetMap contributors, ODbL 1.0";

/// The live providers over HTTP.
pub struct Live {
    creds: Credentials,
    agent: ureq::Agent,
}

impl Live {
    pub fn new(creds: Credentials) -> Self {
        let config = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(90))).build();
        Self { creds, agent: config.into() }
    }

    pub fn credentials(&self) -> &Credentials {
        &self.creds
    }

    /// GET with query parameters. Errors name the service, never the URL.
    fn get(&self, service: &str, url: &str, query: &[(&str, &str)]) -> Result<Vec<u8>> {
        let resp = self.agent.get(url).header("User-Agent", USER_AGENT).query_pairs(query.iter().copied()).call();
        let mut resp = resp.map_err(|e| match e {
            ureq::Error::StatusCode(401 | 403) => DataError::Unavailable(format!("{service} refused the credentials (HTTP 401/403)")),
            ureq::Error::StatusCode(c) => DataError::Unavailable(format!("{service} answered HTTP {c}")),
            ureq::Error::Timeout(_) => DataError::Unavailable(format!("{service} timed out")),
            _ => DataError::Unavailable(format!("{service} could not be reached")),
        })?;
        resp.body_mut().with_config().limit(MAX_BODY).read_to_vec().map_err(|_| DataError::BadResponse(service.to_string()))
    }

    fn get_json(&self, service: &str, url: &str, query: &[(&str, &str)]) -> Result<Value> {
        let bytes = self.get(service, url, query)?;
        serde_json::from_slice(&bytes).map_err(|_| DataError::BadResponse(service.to_string()))
    }

    fn token(&self) -> Result<&str> {
        self.creds.dataforsyningen.as_deref().ok_or_else(|| {
            DataError::Unavailable("no Dataforsyningen token: set TRIGON_DATAFORSYNINGEN_TOKEN or TRIGON_DATAFORSYNINGEN_TOKEN_FILE".into())
        })
    }

    fn gsearch(&self, query: &str, token: &str) -> Result<Address> {
        let v = self.get_json("GSearch", "https://api.dataforsyningen.dk/rest/gsearch/v2.0/adresse", &[("q", query), ("token", token)])?;
        let items = v.as_array().ok_or_else(|| DataError::BadResponse("GSearch".into()))?;
        // The access address (no floor or door) when there is one.
        let item = items
            .iter()
            .find(|i| i.get("etagebetegnelse").is_some_and(Value::is_null))
            .or_else(|| items.first())
            .ok_or_else(|| DataError::NotFound(format!("address {query}")))?;
        let xy = item.pointer("/geometri/coordinates/0").and_then(Value::as_array).ok_or_else(|| DataError::BadResponse("GSearch".into()))?;
        let (e, n) = match (xy.first().and_then(Value::as_f64), xy.get(1).and_then(Value::as_f64)) {
            (Some(e), Some(n)) => (e, n),
            _ => return Err(DataError::BadResponse("GSearch".into())),
        };
        let id = item.get("id").and_then(Value::as_str).map(str::to_string);
        Ok(Address {
            text: item.get("visningstekst").and_then(Value::as_str).unwrap_or(query).to_string(),
            wgs: coords::utm_to_wgs([e, n])?,
            utm: [e, n],
            evidence: Evidence {
                source: format!("dar:adresse/{}", id.as_deref().unwrap_or("?")),
                supplies: "address".into(),
                attribution: SDFI.into(),
            },
            id,
        })
    }

    fn nominatim(&self, query: &str) -> Result<Address> {
        let v = self.get_json("Nominatim", "https://nominatim.openstreetmap.org/search", &[("q", query), ("format", "json"), ("limit", "1")])?;
        let item = v.get(0).ok_or_else(|| DataError::NotFound(format!("address {query}")))?;
        let num = |k: &str| item.get(k).and_then(Value::as_str).and_then(|s| s.parse::<f64>().ok());
        let (Some(lat), Some(lon)) = (num("lat"), num("lon")) else { return Err(DataError::BadResponse("Nominatim".into())) };
        let wgs = GeoPoint { lat, lon };
        let kind = item.get("osm_type").and_then(Value::as_str).unwrap_or("?");
        let osm_id = item.get("osm_id").and_then(Value::as_u64).unwrap_or(0);
        Ok(Address {
            text: item.get("display_name").and_then(Value::as_str).unwrap_or(query).to_string(),
            id: None,
            utm: coords::wgs_to_utm(wgs)?,
            wgs,
            evidence: Evidence { source: format!("osm:{kind}/{osm_id}"), supplies: "address".into(), attribution: OSM.into() },
        })
    }

    /// One float32 GeoTIFF from Danmarks Højdemodel over `bbox` (UTM metres) at 0.4 m.
    fn dhm(&self, coverage: &str, bbox: [f64; 4], size: (u32, u32)) -> Result<Vec<f32>> {
        let token = self.token()?;
        let b = format!("{},{},{},{}", bbox[0], bbox[1], bbox[2], bbox[3]);
        let (w, h) = (size.0.to_string(), size.1.to_string());
        let q = [
            ("service", "WCS"),
            ("version", "1.0.0"),
            ("request", "GetCoverage"),
            ("coverage", coverage),
            ("crs", "EPSG:25832"),
            ("bbox", b.as_str()),
            ("width", w.as_str()),
            ("height", h.as_str()),
            ("format", "GTiff"),
            ("token", token),
        ];
        let bytes = self.get("Danmarks Højdemodel", "https://api.dataforsyningen.dk/dhm_wcs_DAF", &q)?;
        let bad = || DataError::BadResponse(format!("Danmarks Højdemodel ({coverage})"));
        let mut dec = tiff::decoder::Decoder::new(Cursor::new(bytes)).map_err(|_| bad())?;
        if dec.dimensions().map_err(|_| bad())? != size {
            return Err(bad());
        }
        match dec.read_image().map_err(|_| bad())? {
            tiff::decoder::DecodingResult::F32(v) => Ok(v),
            _ => Err(bad()),
        }
    }
}

impl Provider for Live {
    fn geocode(&self, query: &str) -> Result<Address> {
        match self.creds.dataforsyningen.as_deref() {
            Some(token) => self.gsearch(query, token),
            None => self.nominatim(query),
        }
    }

    fn footprint(&self, at: &Address) -> Result<Footprint> {
        let q = format!("[out:json][timeout:25];way(around:30,{},{})[\"building\"];out tags geom;", at.wgs.lat, at.wgs.lon);
        let v = self.get_json("OpenStreetMap (Overpass)", "https://overpass-api.de/api/interpreter", &[("data", q.as_str())])?;
        let ways = v.get("elements").and_then(Value::as_array).ok_or_else(|| DataError::BadResponse("Overpass".into()))?;
        let mut best: Option<(f64, Footprint)> = None;
        for w in ways {
            let Some(geom) = w.get("geometry").and_then(Value::as_array) else { continue };
            let ring: Vec<[f64; 2]> = geom
                .iter()
                .filter_map(|p| Some(GeoPoint { lat: p.get("lat")?.as_f64()?, lon: p.get("lon")?.as_f64()? }))
                .filter_map(|g| coords::wgs_to_utm(g).ok())
                .collect();
            if ring.len() < 4 {
                continue;
            }
            // Prefer the outline containing the address point, then the nearest.
            let score = if coords::contains(&ring, at.utm) { 0.0 } else { 1.0 + coords::distance_to_ring(&ring, at.utm) };
            if best.as_ref().is_some_and(|(s, _)| *s <= score) {
                continue;
            }
            let tags = w
                .get("tags")
                .and_then(Value::as_object)
                .map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect())
                .unwrap_or_default();
            let id = w.get("id").and_then(Value::as_u64).unwrap_or(0);
            let evidence = Evidence { source: format!("osm:way/{id}"), supplies: "footprint".into(), attribution: OSM.into() };
            best = Some((score, Footprint { ring_utm: ring, tags, evidence }));
        }
        best.map(|(_, f)| f).ok_or_else(|| DataError::NotFound(format!("no building outline within 30 m of {}", at.text)))
    }

    fn heights(&self, ring: &[[f64; 2]]) -> Result<Heights> {
        let (lo, hi) = coords::bounds(ring).ok_or_else(|| DataError::NotFound("empty footprint".into()))?;
        let pad = 2.0;
        let bbox = [(lo[0] - pad).floor(), (lo[1] - pad).floor(), (hi[0] + pad).ceil(), (hi[1] + pad).ceil()];
        let px = 0.4;
        let w = ((bbox[2] - bbox[0]) / px).round().clamp(1.0, 2000.0) as u32;
        let h = ((bbox[3] - bbox[1]) / px).round().clamp(1.0, 2000.0) as u32;
        let dsm = self.dhm("dhm_overflade", bbox, (w, h))?;
        let dtm = self.dhm("dhm_terraen", bbox, (w, h))?;
        let (dx, dy) = ((bbox[2] - bbox[0]) / f64::from(w), (bbox[3] - bbox[1]) / f64::from(h));
        let mut rel = Vec::new();
        let mut ground = Vec::new();
        for j in 0..h {
            for i in 0..w {
                let p = [bbox[0] + (f64::from(i) + 0.5) * dx, bbox[3] - (f64::from(j) + 0.5) * dy];
                if !coords::contains(ring, p) {
                    continue;
                }
                let k = (j * w + i) as usize;
                let (Some(&s), Some(&t)) = (dsm.get(k), dtm.get(k)) else { continue };
                let (s, t) = (f64::from(s), f64::from(t));
                if (-50.0..500.0).contains(&s) && (-50.0..500.0).contains(&t) {
                    rel.push(s - t);
                    ground.push(t);
                }
            }
        }
        if rel.len() < 10 {
            return Err(DataError::NotFound("too few elevation samples inside the footprint".into()));
        }
        rel.sort_by(f64::total_cmp);
        ground.sort_by(f64::total_cmp);
        let q = |v: &[f64], f: f64| v.get(((v.len() - 1) as f64 * f).round() as usize).copied().unwrap_or(0.0);
        Ok(Heights {
            terrain: q(&ground, 0.5),
            median: q(&rel, 0.5),
            top: q(&rel, 0.95),
            samples: rel.len(),
            evidence: Evidence {
                source: "dhm:dhm_overflade,dhm_terraen".into(),
                supplies: "heights".into(),
                attribution: format!("{SDFI} (Danmarks Højdemodel)"),
            },
        })
    }

    fn photos(&self, at: [f64; 2]) -> Result<Vec<Photo>> {
        let token = self.token()?;
        let bbox = format!("{},{},{},{}", at[0] - 2.0, at[1] - 2.0, at[0] + 2.0, at[1] + 2.0);
        let mut out = Vec::new();
        for c in SKRAAFOTO_COLLECTIONS {
            let url = format!("https://api.dataforsyningen.dk/rest/skraafoto_api/v1.0/collections/{c}/items");
            let q = [("bbox", bbox.as_str()), ("bbox-crs", "http://www.opengis.net/def/crs/EPSG/0/25832"), ("limit", "50"), ("token", token)];
            let v = self.get_json("Skråfoto", &url, &q)?;
            for f in v.get("features").and_then(Value::as_array).into_iter().flatten() {
                let s = |p: &str| f.pointer(p).and_then(Value::as_str).unwrap_or_default().to_string();
                out.push(Photo {
                    id: s("/id"),
                    collection: c.to_string(),
                    direction: s("/properties/direction"),
                    datetime: s("/properties/datetime"),
                });
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_debug_never_shows_values() {
        let c = Credentials { dataforsyningen: Some("secret-value".into()), datafordeler: None };
        let s = format!("{c:?}");
        assert!(!s.contains("secret-value"));
        assert!(s.contains("set"));
    }
}

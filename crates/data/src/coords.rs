//! WGS84 ↔ ETRS89 / UTM 32N (EPSG:25832) and small plane-geometry helpers.
//!
//! ETRS89 and WGS84 differ by well under a metre in Denmark, far below what footprints from
//! OpenStreetMap resolve, so they are treated as the same datum here.

use proj4rs::Proj;
use proj4rs::adaptors::transform_xy;
use trigon_model::GeoPoint;

use crate::{DataError, Result};

const LONGLAT: &str = "+proj=longlat +ellps=GRS80 +no_defs";
const UTM32: &str = "+proj=utm +zone=32 +ellps=GRS80 +units=m +no_defs";

fn projs() -> Result<(Proj, Proj)> {
    let bad = |_| DataError::BadResponse("coordinate system definition".into());
    Ok((Proj::from_proj_string(LONGLAT).map_err(bad)?, Proj::from_proj_string(UTM32).map_err(bad)?))
}

pub fn wgs_to_utm(p: GeoPoint) -> Result<[f64; 2]> {
    if !(p.lat.abs() <= 90.0 && p.lon.abs() <= 180.0) {
        return Err(DataError::BadResponse(format!("coordinates {}, {}", p.lat, p.lon)));
    }
    let (ll, utm) = projs()?;
    let (e, n) = transform_xy(&ll, &utm, p.lon.to_radians(), p.lat.to_radians()).map_err(|_| DataError::BadResponse("projection".into()))?;
    Ok([e, n])
}

pub fn utm_to_wgs(p: [f64; 2]) -> Result<GeoPoint> {
    if !(p[0].is_finite() && p[1].is_finite()) {
        return Err(DataError::BadResponse("coordinates".into()));
    }
    let (ll, utm) = projs()?;
    let (lon, lat) = transform_xy(&utm, &ll, p[0], p[1]).map_err(|_| DataError::BadResponse("projection".into()))?;
    Ok(GeoPoint { lat: lat.to_degrees(), lon: lon.to_degrees() })
}

/// Even-odd point-in-polygon test.
pub fn contains(ring: &[[f64; 2]], p: [f64; 2]) -> bool {
    let n = ring.len();
    let mut inside = false;
    for i in 0..n {
        let a = ring[i];
        let b = ring[(i + n - 1) % n];
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0] {
            inside = !inside;
        }
    }
    inside
}

pub fn distance_to_ring(ring: &[[f64; 2]], p: [f64; 2]) -> f64 {
    let n = ring.len();
    (0..n)
        .map(|i| {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            let l2 = dx * dx + dy * dy;
            let t = if l2 > 0.0 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
            ((a[0] + t * dx - p[0]).powi(2) + (a[1] + t * dy - p[1]).powi(2)).sqrt()
        })
        .fold(f64::INFINITY, f64::min)
}

pub fn bounds(ring: &[[f64; 2]]) -> Option<([f64; 2], [f64; 2])> {
    let first = *ring.first()?;
    Some(ring.iter().fold((first, first), |(lo, hi), p| ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copenhagen_round_trips_and_matches_gsearch() {
        // Frederikssundsvej 28: GSearch gives E 721989.65 N 6178579.47; OSM puts the house
        // node at 55.702076, 12.533055. They should agree to a few metres.
        let utm = wgs_to_utm(GeoPoint { lat: 55.702076, lon: 12.533055 }).unwrap();
        assert!((utm[0] - 721989.65).abs() < 15.0 && (utm[1] - 6178579.47).abs() < 15.0, "{utm:?}");
        let back = utm_to_wgs(utm).unwrap();
        assert!((back.lat - 55.702076).abs() < 1e-8 && (back.lon - 12.533055).abs() < 1e-8);
    }

    #[test]
    fn point_in_polygon() {
        let sq = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        assert!(contains(&sq, [5.0, 5.0]));
        assert!(!contains(&sq, [15.0, 5.0]));
        assert!((distance_to_ring(&sq, [15.0, 5.0]) - 5.0).abs() < 1e-12);
    }
}

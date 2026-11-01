//! Temporary drawing-plane guides. Exact endpoint/intersection/midpoint snaps win before guides;
//! guides win before the grid. No geometry or constraints are added to the model.
use crate::{DrawingSnap, SnapKind, SnapOptions, snap_drawing};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AlignmentOptions {
    #[serde(default)]
    pub anchors: Vec<[f64; 2]>,
    #[serde(default)]
    pub offset_mm: f64,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct AlignedSnap {
    pub snap: DrawingSnap,
    pub guides: Vec<[[f64; 2]; 2]>,
}

impl AlignmentOptions {
    pub fn validate(&self) -> Result<(), trigon_model::ModelError> {
        if self.anchors.len() > 512 || self.anchors.iter().flatten().any(|v| !v.is_finite() || v.abs() > 1_000_000.0) {
            return Err(trigon_model::ModelError::Invalid("alignment needs at most 512 finite drawing anchors".into()));
        }
        trigon_model::checked("guide offset (mm)", self.offset_mm, -100_000.0, 100_000.0)?;
        Ok(())
    }
}

pub fn snap_drawing_aligned(
    segments: &[[[f64; 2]; 2]],
    point: [f64; 2],
    tolerance_mm: f64,
    options: SnapOptions,
    alignment: Option<&AlignmentOptions>,
) -> Option<AlignedSnap> {
    let exact = snap_drawing(segments, point, tolerance_mm, SnapOptions { grid_mm: None, ..options });
    if let Some(snap) = exact {
        return Some(AlignedSnap { snap, guides: Vec::new() });
    }
    if point.iter().any(|v| !v.is_finite() || v.abs() > 1_000_000.0) || !tolerance_mm.is_finite() || !(0.0..=100_000.0).contains(&tolerance_mm) {
        return None;
    }
    if let Some(settings) = alignment.filter(|s| s.validate().is_ok()) {
        let valid = |p: [f64; 2]| p.iter().all(|v| v.is_finite() && v.abs() <= 1_000_000.0);
        let mut axes: [Option<(f64, [f64; 2])>; 2] = [None, None];
        let candidates =
            segments.iter().flat_map(|[a, b]| [*a, *b, [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0]]).chain(settings.anchors.iter().copied());
        for anchor in candidates.filter(|p| valid(*p)) {
            for axis in 0..2 {
                for offset in [settings.offset_mm, -settings.offset_mm] {
                    let value = anchor[axis] + offset;
                    let distance = (point[axis] - value).abs();
                    let old = axes[axis];
                    if distance <= tolerance_mm
                        && old.is_none_or(|(v, a)| {
                            distance
                                .total_cmp(&(point[axis] - v).abs())
                                .then(value.total_cmp(&v))
                                .then(anchor[0].total_cmp(&a[0]))
                                .then(anchor[1].total_cmp(&a[1]))
                                .is_lt()
                        })
                    {
                        axes[axis] = Some((value, anchor));
                    }
                }
            }
        }
        let mut target = point;
        for (axis, value) in axes.iter().enumerate() {
            if let Some((v, _)) = value {
                target[axis] = *v;
            }
        }
        let mut guides = Vec::new();
        for (axis, value) in axes.iter().enumerate() {
            if let Some((v, anchor)) = value {
                let mut start = *anchor;
                start[axis] = *v;
                guides.push([start, target]);
            }
        }
        let mut best =
            (!guides.is_empty() && valid(target)).then_some(AlignedSnap { snap: DrawingSnap { point: target, kind: SnapKind::Alignment }, guides });
        // Infinite offset lines also follow oblique drawing geometry; the perpendicular projection
        // is computed in mm, not screen coordinates. Collinear wall/annotation directions agree.
        for &[a, b] in segments {
            if !valid(a) || !valid(b) {
                continue;
            }
            let d = [b[0] - a[0], b[1] - a[1]];
            let length = d[0].hypot(d[1]);
            if length < 0.001 {
                continue;
            }
            let n = [-d[1] / length, d[0] / length];
            let normal = (point[0] - a[0]) * n[0] + (point[1] - a[1]) * n[1];
            for offset in [settings.offset_mm, -settings.offset_mm] {
                let distance = (normal - offset).abs();
                let target = [point[0] - (normal - offset) * n[0], point[1] - (normal - offset) * n[1]];
                let current = best.as_ref().map(|b| (b.snap.point[0] - point[0]).hypot(b.snap.point[1] - point[1]));
                // Prefer two-axis alignments; one-axis and oblique candidates use nearest distance.
                if valid(target)
                    && distance <= tolerance_mm
                    && best.as_ref().is_none_or(|b| {
                        b.guides.len() < 2 && current.is_none_or(|v| distance < v - 1e-8 || ((distance - v).abs() < 1e-8 && target < b.snap.point))
                    })
                {
                    let along = (target[0] - a[0]) * d[0] / length + (target[1] - a[1]) * d[1] / length;
                    let extent = length.max(along.abs()).max(1000.0);
                    best = Some(AlignedSnap {
                        snap: DrawingSnap { point: target, kind: SnapKind::Alignment },
                        guides: vec![[
                            [a[0] + n[0] * offset - d[0] / length * extent, a[1] + n[1] * offset - d[1] / length * extent],
                            [a[0] + n[0] * offset + d[0] / length * extent * 2.0, a[1] + n[1] * offset + d[1] / length * extent * 2.0],
                        ]],
                    });
                }
            }
        }
        if best.is_some() {
            return best;
        }
    }
    snap_drawing(segments, point, tolerance_mm, options).map(|snap| AlignedSnap { snap, guides: Vec::new() })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn axes_offsets_and_oblique_guides_are_exact_order_independent_and_opt_out() {
        let opts = SnapOptions { grid_mm: None, ..Default::default() };
        let anchors = AlignmentOptions { anchors: vec![[1000.0, 3000.0], [4000.0, 2000.0]], offset_mm: 0.0 };
        let hit = snap_drawing_aligned(&[], [1002.0, 2003.0], 10.0, opts, Some(&anchors)).unwrap();
        assert_eq!(hit.snap.point, [1000.0, 2000.0]);
        assert_eq!(hit.guides.len(), 2);
        assert!(snap_drawing_aligned(&[], [1002.0, 2003.0], 10.0, opts, None).is_none());
        let reversed = AlignmentOptions { anchors: anchors.anchors.iter().rev().copied().collect(), ..Default::default() };
        assert_eq!(hit, snap_drawing_aligned(&[], [1002.0, 2003.0], 10.0, opts, Some(&reversed)).unwrap());
        let offset = AlignmentOptions { offset_mm: 500.0, ..anchors };
        assert_eq!(snap_drawing_aligned(&[], [1502.0, 2503.0], 10.0, opts, Some(&offset)).unwrap().snap.point, [1500.0, 2500.0]);
        let line = [[[0.0, 0.0], [1000.0, 1000.0]]];
        let hit = snap_drawing_aligned(&line, [2000.0, 2004.0], 10.0, opts, Some(&Default::default())).unwrap();
        assert!((hit.snap.point[0] - 2002.0).abs() < 1e-8 && (hit.snap.point[1] - 2002.0).abs() < 1e-8);
        let hit = snap_drawing_aligned(&line, [1001.0, 1003.0], 10.0, opts, Some(&Default::default())).unwrap();
        assert_eq!(hit.snap.kind, SnapKind::Endpoint);
        assert!(hit.guides.is_empty());
        let hit =
            snap_drawing_aligned(&[[[99.0, 100.0], [99.0, 300.0]]], [100.0, 100.0], 10.0, SnapOptions::default(), Some(&Default::default())).unwrap();
        assert_eq!(hit.snap.point, [99.0, 100.0], "exact geometry must beat a closer grid point");
        assert!(snap_drawing_aligned(&line, [f64::NAN, 0.0], 10.0, opts, Some(&offset)).is_none());
    }
}

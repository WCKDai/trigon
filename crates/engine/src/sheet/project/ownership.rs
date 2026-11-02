//! Broad-phase lookup for large source-edge groups. Exact attribution still uses `overlap`;
//! this tree only excludes spatially disjoint candidates and returns original source order.

use super::{LIMIT, P3};

#[derive(Clone, Copy)]
struct Bounds {
    lo: P3,
    hi: P3,
}

impl Bounds {
    fn edge(edge: [P3; 2]) -> Option<Self> {
        if edge.iter().flatten().any(|x| !x.is_finite() || x.abs() > LIMIT) {
            return None;
        }
        Some(Self { lo: std::array::from_fn(|k| edge[0][k].min(edge[1][k])), hi: std::array::from_fn(|k| edge[0][k].max(edge[1][k])) })
    }

    fn union(self, other: Self) -> Self {
        Self { lo: std::array::from_fn(|k| self.lo[k].min(other.lo[k])), hi: std::array::from_fn(|k| self.hi[k].max(other.hi[k])) }
    }

    fn overlaps(self, other: Self) -> bool {
        (0..3).all(|k| self.lo[k] <= other.hi[k] && other.lo[k] <= self.hi[k])
    }
}

struct Item {
    bounds: Bounds,
    original: usize,
}

struct Node {
    bounds: Bounds,
    range: std::ops::Range<usize>,
    children: Option<[usize; 2]>,
}

pub(super) struct EdgeIndex {
    items: Vec<Item>,
    nodes: Vec<Node>,
}

impl EdgeIndex {
    pub(super) fn new(edges: &[(usize, [P3; 2])]) -> Self {
        let mut result = Self {
            items: edges.iter().enumerate().filter_map(|(original, (_, edge))| Bounds::edge(*edge).map(|bounds| Item { bounds, original })).collect(),
            nodes: Vec::new(),
        };
        Self::build(&mut result.items, 0, &mut result.nodes);
        result
    }

    fn build(items: &mut [Item], offset: usize, nodes: &mut Vec<Node>) -> Option<usize> {
        let bounds = items.iter().map(|item| item.bounds).reduce(Bounds::union)?;
        let index = nodes.len();
        nodes.push(Node { bounds, range: offset..offset + items.len(), children: None });
        if items.len() > 16 {
            let mut axis = 0;
            for k in 1..3 {
                if bounds.hi[k] - bounds.lo[k] > bounds.hi[axis] - bounds.lo[axis] {
                    axis = k;
                }
            }
            items.sort_unstable_by(|a, b| {
                (a.bounds.lo[axis] + a.bounds.hi[axis]).total_cmp(&(b.bounds.lo[axis] + b.bounds.hi[axis])).then(a.original.cmp(&b.original))
            });
            let middle = items.len() / 2;
            let (left, right) = items.split_at_mut(middle);
            if let (Some(left), Some(right)) = (Self::build(left, offset, nodes), Self::build(right, offset + middle, nodes)) {
                nodes[index].children = Some([left, right]);
            }
        }
        Some(index)
    }

    pub(super) fn query(&self, edge: [P3; 2], out: &mut Vec<usize>) {
        out.clear();
        let Some(mut query) = Bounds::edge(edge) else { return };
        // The exact predicate retains its 0.1 mm tolerance. The extra margin only makes this
        // rejection bound conservative against arithmetic rounding at coordinates near LIMIT.
        for k in 0..3 {
            query.lo[k] -= 0.1 + 1e-6;
            query.hi[k] += 0.1 + 1e-6;
        }
        if !self.nodes.is_empty() {
            self.visit(0, query, out);
        }
        out.sort_unstable();
    }

    fn visit(&self, index: usize, query: Bounds, out: &mut Vec<usize>) {
        let Some(node) = self.nodes.get(index).filter(|node| node.bounds.overlaps(query)) else { return };
        if let Some(children) = node.children {
            for child in children {
                self.visit(child, query, out);
            }
        } else {
            for item in self.items.get(node.range.clone()).into_iter().flatten().filter(|item| item.bounds.overlaps(query)) {
                out.push(item.original);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheet::overlap;

    #[test]
    fn lookup_preserves_linear_overlap_results_and_original_order_at_large_coordinates() {
        for shift in [0.0, 90_000_000.0, -90_000_000.0] {
            let mut edges: Vec<_> = (0..10000)
                .map(|k| {
                    let a = [shift + (k % 100) as f64 * 200.0, (k % 3) as f64 * 10.0, (k / 100) as f64 * 200.0];
                    (k % 7, [a, [a[0] + 120.0, a[1] + 20.0, a[2] + 40.0]])
                })
                .collect();
            edges.extend([(0, [[f64::NAN; 3]; 2]), (0, [[f64::INFINITY; 3]; 2])]);
            let index = EdgeIndex::new(&edges);
            let mut candidates = Vec::new();
            for id in [0, 1, 13, 511, 9999] {
                let [a, b] = edges[id].1;
                let point = |t: f64, offset: f64| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t + offset, a[2] + (b[2] - a[2]) * t];
                for query in [
                    [a, b],
                    [b, a],
                    [point(0.25, 0.0), point(0.75, 0.0)],
                    [point(0.0, 0.0999), point(1.0, 0.0999)],
                    [point(0.0, 0.1001), point(1.0, 0.1001)],
                    [a, a],
                    [[f64::NAN; 3]; 2],
                ] {
                    let expected: Vec<_> =
                        edges.iter().enumerate().filter_map(|(k, (_, source))| overlap(query, *source, 0.1).map(|part| (k, part))).collect();
                    index.query(query, &mut candidates);
                    assert!(candidates.windows(2).all(|p| p[0] < p[1]), "source order and duplicates must not depend on tree traversal");
                    let actual: Vec<_> = candidates.iter().filter_map(|&k| overlap(query, edges[k].1, 0.1).map(|part| (k, part))).collect();
                    assert_eq!(actual, expected);
                    assert!(candidates.len() <= 32, "a local edge lookup must not scan an entire detailed surface: {}", candidates.len());
                }
            }
        }
    }
}

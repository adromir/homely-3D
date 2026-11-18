//! 2D poly-Bezier tessellation and wall footprint geometry.

use crate::{Wall, WallAnchorPoint};
use serde::{Deserialize, Serialize};

const MAX_BEZIER_DEPTH: u8 = 10;

/// 2D point or vector in floorplan coordinates (meters).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

impl Vec2 {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    fn sub(self, rhs: Self) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }

    fn add(self, rhs: Self) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }

    fn scale(self, s: f64) -> Self {
        Self::new(self.x * s, self.y * s)
    }

    fn dot(self, rhs: Self) -> f64 {
        self.x * rhs.x + self.y * rhs.y
    }

    fn len(self) -> f64 {
        (self.x * self.x + self.y * self.y).sqrt()
    }

    fn normalized(self) -> Option<Self> {
        let l = self.len();
        if l > 1e-9 {
            Some(Self::new(self.x / l, self.y / l))
        } else {
            None
        }
    }

    fn perp_left(self) -> Self {
        Self::new(-self.y, self.x)
    }
}

/// Tessellates a [`Wall`]'s poly-Bezier path into a 2D centerline polyline.
///
/// `flatness_tolerance` controls the maximum allowed perpendicular deviation (in meters)
/// of cubic Bezier control points from the chord before subdividing.
pub fn tessellate_wall_centerline(wall: &Wall, flatness_tolerance: f64) -> Vec<Vec2> {
    if wall.anchors.is_empty() {
        return Vec::new();
    }
    if wall.anchors.len() == 1 {
        return vec![anchor_pos(&wall.anchors[0])];
    }

    let tol = flatness_tolerance.max(1e-4);
    let mut points = vec![anchor_pos(&wall.anchors[0])];

    let segment_count = if wall.is_closed_loop {
        wall.anchors.len()
    } else {
        wall.anchors.len() - 1
    };

    for i in 0..segment_count {
        let a = &wall.anchors[i];
        let b = &wall.anchors[(i + 1) % wall.anchors.len()];
        tessellate_segment(a, b, tol, &mut points);
    }

    points
}

fn anchor_pos(a: &WallAnchorPoint) -> Vec2 {
    Vec2::new(a.pos_x, a.pos_y)
}

fn tessellate_segment(
    a: &WallAnchorPoint,
    b: &WallAnchorPoint,
    flatness_tol: f64,
    out: &mut Vec<Vec2>,
) {
    let p0 = anchor_pos(a);
    let p3 = anchor_pos(b);

    let has_out = a.handle_out_x.is_some() && a.handle_out_y.is_some();
    let has_in = b.handle_in_x.is_some() && b.handle_in_y.is_some();

    if !has_out && !has_in {
        out.push(p3);
        return;
    }

    let p1 = match (a.handle_out_x, a.handle_out_y) {
        (Some(x), Some(y)) => Vec2::new(x, y),
        _ => p0,
    };
    let p2 = match (b.handle_in_x, b.handle_in_y) {
        (Some(x), Some(y)) => Vec2::new(x, y),
        _ => p3,
    };

    subdivide_cubic(p0, p1, p2, p3, flatness_tol, 0, out);
}

fn subdivide_cubic(
    p0: Vec2,
    p1: Vec2,
    p2: Vec2,
    p3: Vec2,
    flatness_tol: f64,
    depth: u8,
    out: &mut Vec<Vec2>,
) {
    if depth >= MAX_BEZIER_DEPTH || is_flat(p0, p1, p2, p3, flatness_tol) {
        out.push(p3);
        return;
    }

    // De Casteljau midpoint split
    let p01 = midpoint(p0, p1);
    let p12 = midpoint(p1, p2);
    let p23 = midpoint(p2, p3);
    let p012 = midpoint(p01, p12);
    let p123 = midpoint(p12, p23);
    let p0123 = midpoint(p012, p123);

    subdivide_cubic(p0, p01, p012, p0123, flatness_tol, depth + 1, out);
    subdivide_cubic(p0123, p123, p23, p3, flatness_tol, depth + 1, out);
}

fn midpoint(a: Vec2, b: Vec2) -> Vec2 {
    Vec2::new((a.x + b.x) * 0.5, (a.y + b.y) * 0.5)
}

fn is_flat(p0: Vec2, p1: Vec2, p2: Vec2, p3: Vec2, tol: f64) -> bool {
    let chord = p3.sub(p0);
    let len_sq = chord.dot(chord);
    if len_sq < 1e-12 {
        return p1.sub(p0).len() <= tol && p2.sub(p0).len() <= tol;
    }
    let len = len_sq.sqrt();
    let d1 = ((p1.x - p0.x) * chord.y - (p1.y - p0.y) * chord.x).abs() / len;
    let d2 = ((p2.x - p0.x) * chord.y - (p2.y - p0.y) * chord.x).abs() / len;
    d1 <= tol && d2 <= tol
}

/// Computes mitered `(left, right)` offset pairs for every wall in `walls`,
/// taking neighboring walls that meet at shared endpoints into account so
/// 2-point walls form seamless corners at any angle.
pub fn compute_level_wall_offsets(
    walls: &[Wall],
    flatness_tolerance: f64,
) -> Vec<Vec<(Vec2, Vec2)>> {
    let centerlines: Vec<Vec<Vec2>> = walls
        .iter()
        .map(|w| tessellate_wall_centerline(w, flatness_tolerance))
        .collect();

    let mut result = Vec::with_capacity(walls.len());
    for (wi, wall) in walls.iter().enumerate() {
        let pts = &centerlines[wi];
        let mut pairs = wall_offset_pairs(pts, wall.thickness, wall.is_closed_loop);
        if pairs.len() < 2 || wall.is_closed_loop {
            result.push(pairs);
            continue;
        }

        let half = wall.thickness * 0.5;
        let last_idx = pts.len() - 1;

        // Miter start endpoint if exactly 1 neighbor wall meets at pts[0]
        if let Some(n_dir_in) = find_single_neighbor_dir_into(wi, pts[0], &centerlines) {
            if let Some(w_dir_out) = pts[1].sub(pts[0]).normalized() {
                let n1 = n_dir_in.perp_left();
                let n2 = w_dir_out.perp_left();
                if let Some(avg) = n1.add(n2).normalized() {
                    let cos_half = avg.dot(n2).max(0.25);
                    let offset = avg.scale(half / cos_half);
                    pairs[0] = (pts[0].add(offset), pts[0].sub(offset));
                }
            }
        }

        // Miter end endpoint if exactly 1 neighbor wall meets at pts[last_idx]
        if let Some(n_dir_out) = find_single_neighbor_dir_out(wi, pts[last_idx], &centerlines) {
            if let Some(w_dir_in) = pts[last_idx].sub(pts[last_idx - 1]).normalized() {
                let n1 = w_dir_in.perp_left();
                let n2 = n_dir_out.perp_left();
                if let Some(avg) = n1.add(n2).normalized() {
                    let cos_half = avg.dot(n1).max(0.25);
                    let offset = avg.scale(half / cos_half);
                    pairs[last_idx] = (pts[last_idx].add(offset), pts[last_idx].sub(offset));
                }
            }
        }

        result.push(pairs);
    }
    result
}

fn find_single_neighbor_dir_into(
    self_idx: usize,
    endpoint: Vec2,
    centerlines: &[Vec<Vec2>],
) -> Option<Vec2> {
    let mut dirs = Vec::new();
    for (oi, pts) in centerlines.iter().enumerate() {
        if oi == self_idx || pts.len() < 2 {
            continue;
        }
        if pts[pts.len() - 1].sub(endpoint).len() < 1e-4 {
            if let Some(d) = pts[pts.len() - 1].sub(pts[pts.len() - 2]).normalized() {
                dirs.push(d);
            }
        } else if pts[0].sub(endpoint).len() < 1e-4 {
            // Neighbor also starts here: its incoming direction into endpoint is reversed
            if let Some(d) = pts[0].sub(pts[1]).normalized() {
                dirs.push(d);
            }
        }
    }
    if dirs.len() == 1 {
        Some(dirs[0])
    } else {
        None
    }
}

fn find_single_neighbor_dir_out(
    self_idx: usize,
    endpoint: Vec2,
    centerlines: &[Vec<Vec2>],
) -> Option<Vec2> {
    let mut dirs = Vec::new();
    for (oi, pts) in centerlines.iter().enumerate() {
        if oi == self_idx || pts.len() < 2 {
            continue;
        }
        if pts[0].sub(endpoint).len() < 1e-4 {
            if let Some(d) = pts[1].sub(pts[0]).normalized() {
                dirs.push(d);
            }
        } else if pts[pts.len() - 1].sub(endpoint).len() < 1e-4 {
            // Neighbor also ends here: its outgoing direction away from endpoint is reversed
            if let Some(d) = pts[pts.len() - 2].sub(pts[pts.len() - 1]).normalized() {
                dirs.push(d);
            }
        }
    }
    if dirs.len() == 1 {
        Some(dirs[0])
    } else {
        None
    }
}

/// Computes mitered `(left, right)` 2D offset pairs along `centerline`.
///
/// When `is_closed` is `true`, the first and last vertices wrap around so closed
/// room corners are mitered seamlessly at any angle.
pub fn wall_offset_pairs(
    centerline: &[Vec2],
    thickness: f64,
    is_closed: bool,
) -> Vec<(Vec2, Vec2)> {
    if centerline.len() < 2 || thickness <= 0.0 {
        return Vec::new();
    }

    let half = thickness * 0.5;
    let n = centerline.len();
    let mut pairs = Vec::with_capacity(n);

    for i in 0..n {
        let normal = vertex_normal(centerline, i, is_closed);
        let offset = normal.scale(half);
        pairs.push((centerline[i].add(offset), centerline[i].sub(offset)));
    }

    pairs
}

/// Offsets a 2D centerline polyline by `thickness / 2.0` on both sides,
/// returning a closed 2D footprint polygon.
pub fn wall_footprint_polygon(centerline: &[Vec2], thickness: f64) -> Vec<Vec2> {
    let pairs = wall_offset_pairs(centerline, thickness, false);
    if pairs.is_empty() {
        return Vec::new();
    }
    let mut polygon: Vec<Vec2> = pairs.iter().map(|(l, _)| *l).collect();
    polygon.extend(pairs.iter().rev().map(|(_, r)| *r));
    polygon
}

fn vertex_normal(pts: &[Vec2], idx: usize, is_closed: bool) -> Vec2 {
    let n = pts.len();
    let joined_ends = n >= 3 && pts[0].sub(pts[n - 1]).len() < 1e-6;

    let prev_pt = if idx > 0 {
        Some(pts[idx - 1])
    } else if joined_ends {
        Some(pts[n - 2])
    } else if is_closed && n >= 2 {
        Some(pts[n - 1])
    } else {
        None
    };

    let next_pt = if idx + 1 < n {
        Some(pts[idx + 1])
    } else if joined_ends {
        Some(pts[1])
    } else if is_closed && n >= 2 {
        Some(pts[0])
    } else {
        None
    };

    let prev_n = prev_pt.and_then(|p| pts[idx].sub(p).normalized().map(Vec2::perp_left));
    let next_n = next_pt.and_then(|p| p.sub(pts[idx]).normalized().map(Vec2::perp_left));

    match (prev_n, next_n) {
        (Some(n1), Some(n2)) => {
            let avg = n1.add(n2).normalized().unwrap_or(n1);
            // Miter scale clamped to avoid extreme spikes on very acute corners
            let cos_half = avg.dot(n1).max(0.25);
            avg.scale(1.0 / cos_half)
        }
        (Some(n1), None) => n1,
        (None, Some(n2)) => n2,
        (None, None) => Vec2::new(0.0, 1.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PointType;

    fn corner(id: &str, x: f64, y: f64) -> WallAnchorPoint {
        WallAnchorPoint {
            id: id.into(),
            pos_x: x,
            pos_y: y,
            handle_in_x: None,
            handle_in_y: None,
            handle_out_x: None,
            handle_out_y: None,
            point_type: PointType::Corner,
        }
    }

    #[test]
    fn straight_wall_footprint() {
        let wall = Wall {
            id: "w1".into(),
            level_id: "l1".into(),
            is_closed_loop: false,
            thickness: 0.24,
            start_height: 2.5,
            end_height: 2.5,
            anchors: vec![corner("a", 0.0, 0.0), corner("b", 4.0, 0.0)],
        };
        let line = tessellate_wall_centerline(&wall, 0.01);
        assert_eq!(line, vec![Vec2::new(0.0, 0.0), Vec2::new(4.0, 0.0)]);

        let poly = wall_footprint_polygon(&line, wall.thickness);
        assert_eq!(poly.len(), 4);
        assert!((poly[0].y - 0.12).abs() < 1e-9);
        assert!((poly[3].y + 0.12).abs() < 1e-9);
    }

    #[test]
    fn curved_bezier_wall_subdivides() {
        let mut a = corner("a", 0.0, 0.0);
        a.handle_out_x = Some(1.0);
        a.handle_out_y = Some(2.0);
        a.point_type = PointType::Smooth;

        let mut b = corner("b", 4.0, 0.0);
        b.handle_in_x = Some(3.0);
        b.handle_in_y = Some(2.0);

        let wall = Wall {
            id: "w2".into(),
            level_id: "l1".into(),
            is_closed_loop: false,
            thickness: 0.20,
            start_height: 2.5,
            end_height: 2.5,
            anchors: vec![a, b],
        };
        let line = tessellate_wall_centerline(&wall, 0.01);
        assert!(line.len() > 4);
        assert_eq!(line.first(), Some(&Vec2::new(0.0, 0.0)));
        assert_eq!(line.last(), Some(&Vec2::new(4.0, 0.0)));
    }
}

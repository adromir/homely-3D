//! Automatic planar-graph room detection and polygon triangulation.

use crate::geometry::{tessellate_wall_centerline, Vec2};
use crate::Wall;
use std::collections::{HashMap, HashSet};

/// A detected enclosed room polygon on a building level.
#[derive(Debug, Clone, PartialEq)]
pub struct DetectedRoom {
    pub polygon: Vec<Vec2>,
    pub area_sqm: f64,
    pub volume_cbm: f64,
    pub centroid: Vec2,
}

/// Detects enclosed rooms formed by `walls` on a single level using a planar
/// half-edge minimal-cycle walk.
pub fn detect_rooms(walls: &[Wall], level_height: f64) -> Vec<DetectedRoom> {
    let mut vertices: Vec<Vec2> = Vec::new();
    let mut vert_map: HashMap<(i64, i64), usize> = HashMap::new();
    let mut edges: HashSet<(usize, usize)> = HashSet::new();

    let mut intern = |p: Vec2| -> usize {
        let key = ((p.x * 1000.0).round() as i64, (p.y * 1000.0).round() as i64);
        if let Some(&idx) = vert_map.get(&key) {
            idx
        } else {
            let idx = vertices.len();
            vertices.push(p);
            vert_map.insert(key, idx);
            idx
        }
    };

    for wall in walls {
        let line = tessellate_wall_centerline(wall, 0.05);
        for win in line.windows(2) {
            let u = intern(win[0]);
            let v = intern(win[1]);
            if u != v {
                edges.insert((u.min(v), u.max(v)));
            }
        }
    }

    if edges.is_empty() {
        return Vec::new();
    }

    // Build CCW-sorted adjacency list at every vertex
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); vertices.len()];
    for &(u, v) in &edges {
        adj[u].push(v);
        adj[v].push(u);
    }
    for (u, nbrs) in adj.iter_mut().enumerate() {
        let pu = vertices[u];
        nbrs.sort_by(|&a, &b| {
            let ang_a = (vertices[a].y - pu.y).atan2(vertices[a].x - pu.x);
            let ang_b = (vertices[b].y - pu.y).atan2(vertices[b].x - pu.x);
            ang_a.total_cmp(&ang_b)
        });
    }

    // Walk minimal faces (in 2D screen coords where +Y is down, CW in (x,y) = positive signed area)
    let mut visited: HashSet<(usize, usize)> = HashSet::new();
    let mut rooms = Vec::new();
    let max_steps = edges.len() * 2 + 4;

    for &(u0, v0) in &edges {
        for &(start_u, start_v) in &[(u0, v0), (v0, u0)] {
            if visited.contains(&(start_u, start_v)) {
                continue;
            }
            let mut cycle = Vec::new();
            let mut cur_u = start_u;
            let mut cur_v = start_v;
            let mut closed = false;

            for _ in 0..max_steps {
                if !visited.insert((cur_u, cur_v)) {
                    break;
                }
                cycle.push(vertices[cur_u]);

                let nbrs = &adj[cur_v];
                let Some(pos) = nbrs.iter().position(|&x| x == cur_u) else {
                    break;
                };
                // Pick previous neighbor in CCW order (= next turn for minimal interior face)
                let next_w = nbrs[(pos + nbrs.len() - 1) % nbrs.len()];
                cur_u = cur_v;
                cur_v = next_w;

                if cur_u == start_u && cur_v == start_v {
                    closed = true;
                    break;
                }
            }

            if closed && cycle.len() >= 3 {
                let signed_area = polygon_signed_area(&cycle);
                // Positive signed area = interior face; negative = outer unbounded face
                if signed_area > 0.1 {
                    let centroid = polygon_centroid(&cycle, signed_area);
                    rooms.push(DetectedRoom {
                        polygon: cycle,
                        area_sqm: signed_area,
                        volume_cbm: signed_area * level_height.max(0.0),
                        centroid,
                    });
                }
            }
        }
    }

    rooms
}

/// Collects all wall IDs on a level that belong to the same connected building
/// component as `seed_wall_id` (via shared endpoints within 1 mm).
pub fn collect_connected_building_walls(walls: &[Wall], seed_wall_id: &str) -> Vec<String> {
    let Some(seed_idx) = walls.iter().position(|w| w.id == seed_wall_id) else {
        return Vec::new();
    };
    let mut visited = vec![false; walls.len()];
    let mut queue = std::collections::VecDeque::new();
    visited[seed_idx] = true;
    queue.push_back(seed_idx);

    let endpoints = |w: &Wall| -> Vec<Vec2> {
        if w.anchors.is_empty() {
            return Vec::new();
        }
        let mut pts = vec![Vec2::new(w.anchors[0].pos_x, w.anchors[0].pos_y)];
        if w.anchors.len() > 1 {
            let last = &w.anchors[w.anchors.len() - 1];
            pts.push(Vec2::new(last.pos_x, last.pos_y));
        }
        pts
    };

    let mut result = Vec::new();
    while let Some(idx) = queue.pop_front() {
        result.push(walls[idx].id.clone());
        let pts_a = endpoints(&walls[idx]);
        for (oi, other) in walls.iter().enumerate() {
            if visited[oi] {
                continue;
            }
            let pts_b = endpoints(other);
            let touches = pts_a.iter().any(|pa| {
                pts_b
                    .iter()
                    .any(|pb| ((pa.x - pb.x).powi(2) + (pa.y - pb.y).powi(2)).sqrt() < 1e-3)
            });
            if touches {
                visited[oi] = true;
                queue.push_back(oi);
            }
        }
    }
    result
}

/// Computes a canonical signature for a detected room polygon so custom names
/// persist across geometry updates.
pub fn room_signature(poly: &[Vec2]) -> String {
    let mut keys: Vec<(i64, i64)> = poly
        .iter()
        .map(|p| ((p.x * 100.0).round() as i64, (p.y * 100.0).round() as i64))
        .collect();
    keys.sort_unstable();
    keys.dedup();
    keys.into_iter()
        .map(|(x, y)| format!("{x}:{y}"))
        .collect::<Vec<_>>()
        .join("|")
}

/// Returns `true` if `point` lies inside `poly` (ray-casting).
pub fn point_in_polygon(point: Vec2, poly: &[Vec2]) -> bool {
    let mut inside = false;
    let n = poly.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let pi = poly[i];
        let pj = poly[j];
        let intersects = ((pi.y > point.y) != (pj.y > point.y))
            && (point.x < (pj.x - pi.x) * (point.y - pi.y) / (pj.y - pi.y + 1e-12) + pi.x);
        if intersects {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// Ear-clipping triangulation of a simple 2D polygon into triangle vertex index triples.
pub fn triangulate_polygon(poly: &[Vec2]) -> Vec<[usize; 3]> {
    let n = poly.len();
    if n < 3 {
        return Vec::new();
    }
    let mut indices: Vec<usize> = (0..n).collect();
    if polygon_signed_area(poly) < 0.0 {
        indices.reverse();
    }

    let mut tris = Vec::with_capacity(n - 2);
    let mut guard = 0;
    while indices.len() > 3 && guard < n * n {
        guard += 1;
        let len = indices.len();
        let mut clipped = false;
        for i in 0..len {
            let prev = indices[(i + len - 1) % len];
            let curr = indices[i];
            let next = indices[(i + 1) % len];

            if !is_convex(poly[prev], poly[curr], poly[next]) {
                continue;
            }
            let contains_other = indices.iter().any(|&p_idx| {
                p_idx != prev
                    && p_idx != curr
                    && p_idx != next
                    && point_in_triangle(poly[p_idx], poly[prev], poly[curr], poly[next])
            });
            if !contains_other {
                tris.push([prev, curr, next]);
                indices.remove(i);
                clipped = true;
                break;
            }
        }
        if !clipped {
            break;
        }
    }
    if indices.len() == 3 {
        tris.push([indices[0], indices[1], indices[2]]);
    }
    tris
}

fn polygon_signed_area(poly: &[Vec2]) -> f64 {
    let mut sum = 0.0;
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        sum += a.x * b.y - b.x * a.y;
    }
    sum * 0.5
}

fn polygon_centroid(poly: &[Vec2], signed_area: f64) -> Vec2 {
    if signed_area.abs() < 1e-9 {
        return poly[0];
    }
    let mut cx = 0.0;
    let mut cy = 0.0;
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        let cross = a.x * b.y - b.x * a.y;
        cx += (a.x + b.x) * cross;
        cy += (a.y + b.y) * cross;
    }
    let factor = 1.0 / (6.0 * signed_area);
    Vec2::new(cx * factor, cy * factor)
}

fn cross_2d(a: Vec2, b: Vec2, c: Vec2) -> f64 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

fn is_convex(a: Vec2, b: Vec2, c: Vec2) -> bool {
    cross_2d(a, b, c) > 1e-9
}

fn point_in_triangle(p: Vec2, a: Vec2, b: Vec2, c: Vec2) -> bool {
    let c1 = cross_2d(a, b, p) >= -1e-9;
    let c2 = cross_2d(b, c, p) >= -1e-9;
    let c3 = cross_2d(c, a, p) >= -1e-9;
    c1 && c2 && c3
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PointType, WallAnchorPoint};

    fn seg_wall(id: &str, x0: f64, y0: f64, x1: f64, y1: f64) -> Wall {
        let anchor = |aid: &str, x: f64, y: f64| WallAnchorPoint {
            id: aid.into(),
            pos_x: x,
            pos_y: y,
            handle_in_x: None,
            handle_in_y: None,
            handle_out_x: None,
            handle_out_y: None,
            point_type: PointType::Corner,
        };
        Wall {
            id: id.into(),
            name: String::new(),
            level_id: "l1".into(),
            is_closed_loop: false,
            thickness: 0.24,
            start_height: 2.5,
            end_height: 2.5,
            side_a_finish: crate::SurfaceFinish::default_wall(),
            side_b_finish: crate::SurfaceFinish::default_wall(),
            anchors: vec![anchor("a", x0, y0), anchor("b", x1, y1)],
        }
    }

    #[test]
    fn detects_two_adjacent_rooms() {
        // 4x3 room + adjacent 3x3 room sharing middle wall at x=4
        let walls = vec![
            seg_wall("w1", 0.0, 0.0, 4.0, 0.0),
            seg_wall("w2", 4.0, 0.0, 7.0, 0.0),
            seg_wall("w3", 7.0, 0.0, 7.0, 3.0),
            seg_wall("w4", 7.0, 3.0, 4.0, 3.0),
            seg_wall("w5", 4.0, 3.0, 0.0, 3.0),
            seg_wall("w6", 0.0, 3.0, 0.0, 0.0),
            seg_wall("w7", 4.0, 0.0, 4.0, 3.0),
        ];
        let mut rooms = detect_rooms(&walls, 2.5);
        rooms.sort_by(|a, b| a.area_sqm.total_cmp(&b.area_sqm));
        assert_eq!(rooms.len(), 2);
        assert!((rooms[0].area_sqm - 9.0).abs() < 1e-6);
        assert!((rooms[1].area_sqm - 12.0).abs() < 1e-6);
        assert_eq!(triangulate_polygon(&rooms[0].polygon).len(), 2);
    }
}

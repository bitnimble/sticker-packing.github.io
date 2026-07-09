use geo::{
    Area, AffineOps, AffineTransform, BooleanOps, BoundingRect, Contains, ConvexHull, LineString,
    MultiPoint, MultiPolygon, Point, Polygon,
};
use geo_types::{coord, Coord};
use std::f64::consts::PI;

pub type Poly = Polygon<f64>;
pub type Multi = MultiPolygon<f64>;

pub fn poly_from(pts: &[(f64, f64)]) -> Poly {
    Polygon::new(
        LineString::new(pts.iter().map(|&(x, y)| coord! {x: x, y: y}).collect()),
        vec![],
    )
}

pub fn poly_bbox(p: &Poly) -> (f64, f64, f64, f64) {
    let r = p.bounding_rect().unwrap();
    (r.min().x, r.min().y, r.max().x, r.max().y)
}

/// Largest-area polygon of a (possibly multi) result. Panics on empty.
pub fn largest(m: &Multi) -> Poly {
    m.0.iter()
        .max_by(|a, b| a.unsigned_area().partial_cmp(&b.unsigned_area()).unwrap())
        .expect("empty multipolygon")
        .clone()
}

// --- affine ---------------------------------------------------------------

/// SVG-convention rotation about the origin: matrix [[cos,-sin],[sin,cos]] (positive = the
/// same visual direction as SVG rotate() in a y-down space), so output transforms match.
pub fn rot_origin(deg: f64) -> AffineTransform<f64> {
    let (s, c) = deg.to_radians().sin_cos();
    AffineTransform::new(c, -s, 0.0, s, c, 0.0)
}

pub fn rotate_m(m: &Multi, deg: f64) -> Multi {
    m.affine_transform(&rot_origin(deg))
}

pub fn rotate_p(p: &Poly, deg: f64) -> Poly {
    p.affine_transform(&rot_origin(deg))
}

pub fn translate_m(m: &Multi, dx: f64, dy: f64) -> Multi {
    m.affine_transform(&AffineTransform::translate(dx, dy))
}

// --- triangulation & Minkowski --------------------------------------------

pub type Tri = [Coord<f64>; 3];

fn push_ring(data: &mut Vec<f64>, ls: &LineString<f64>) {
    let cs = &ls.0;
    let n = cs.len().saturating_sub(1); // drop closing duplicate
    for cpt in &cs[..n] {
        data.push(cpt.x);
        data.push(cpt.y);
    }
}

pub fn triangulate(poly: &Poly) -> Vec<Tri> {
    let mut data: Vec<f64> = Vec::new();
    let mut holes: Vec<usize> = Vec::new();
    push_ring(&mut data, poly.exterior());
    for hole in poly.interiors() {
        holes.push(data.len() / 2);
        push_ring(&mut data, hole);
    }
    let idx = match earcutr::earcut(&data, &holes, 2) {
        Ok(v) => v,
        Err(_) => return vec![],
    };
    idx.chunks_exact(3)
        .map(|t| {
            let g = |k: usize| coord! {x: data[2 * k], y: data[2 * k + 1]};
            [g(t[0]), g(t[1]), g(t[2])]
        })
        .collect()
}

pub fn neg_tris(tris: &[Tri]) -> Vec<Tri> {
    tris.iter()
        .map(|t| [
            coord! {x: -t[0].x, y: -t[0].y},
            coord! {x: -t[1].x, y: -t[1].y},
            coord! {x: -t[2].x, y: -t[2].y},
        ])
        .collect()
}

/// Union of many polygons via divide-and-conquer (only geo pairwise union needed).
pub fn union_all(mut polys: Vec<Multi>) -> Multi {
    if polys.is_empty() {
        return MultiPolygon::new(vec![]);
    }
    while polys.len() > 1 {
        let mut next = Vec::with_capacity(polys.len().div_ceil(2));
        let mut it = polys.into_iter();
        while let Some(a) = it.next() {
            match it.next() {
                Some(b) => next.push(a.union(&b)),
                None => next.push(a),
            }
        }
        polys = next;
    }
    polys.pop().unwrap()
}

/// Exact Minkowski sum A (+) B from triangulations: each triangle-pair contributes the convex
/// hull of the 9 vertex sums; union them all. Handles concave A and B.
pub fn minkowski(tris_a: &[Tri], tris_b: &[Tri]) -> Multi {
    let mut pieces: Vec<Multi> = Vec::with_capacity(tris_a.len() * tris_b.len());
    for ta in tris_a {
        for tb in tris_b {
            let mut pts = Vec::with_capacity(9);
            for a in ta {
                for b in tb {
                    pts.push(Point::new(a.x + b.x, a.y + b.y));
                }
            }
            pieces.push(MultiPolygon::new(vec![MultiPoint::new(pts).convex_hull()]));
        }
    }
    union_all(pieces)
}

fn disk(r: f64, nseg: usize) -> Poly {
    let pts = (0..nseg)
        .map(|i| {
            let a = 2.0 * PI * i as f64 / nseg as f64;
            coord! {x: r * a.cos(), y: r * a.sin()}
        })
        .collect();
    Polygon::new(LineString::new(pts), vec![])
}

/// Round-join outward buffer as a Minkowski sum with a disk polygon (reuses the NFP path,
/// avoiding a separate offset library).
pub fn buffer(poly: &Poly, r: f64, nseg: usize) -> Poly {
    let m = minkowski(&triangulate(poly), &triangulate(&disk(r, nseg)));
    largest(&m)
}

/// Collision body K = P (+) (-P): two copies overlap iff their offset is in K's interior.
pub fn collision_body(p: &Poly) -> Multi {
    let t = triangulate(p);
    let nt = neg_tris(&t);
    minkowski(&t, &nt)
}

// --- queries --------------------------------------------------------------

pub fn contains_pt(m: &Multi, x: f64, y: f64) -> bool {
    m.contains(&Point::new(x, y))
}

/// Vertex of `m` minimizing `key` (the fill corner/axis order), lexicographic on the pair.
pub fn extreme_vertex<F: Fn(f64, f64) -> (f64, f64)>(m: &Multi, key: F) -> Option<(f64, f64)> {
    let mut best: Option<((f64, f64), (f64, f64))> = None;
    for poly in &m.0 {
        for ring in std::iter::once(poly.exterior()).chain(poly.interiors()) {
            for cc in &ring.0 {
                let k = key(cc.x, cc.y);
                if best.as_ref().map_or(true, |(bk, _)| k < *bk) {
                    best = Some((k, (cc.x, cc.y)));
                }
            }
        }
    }
    best.map(|(_, p)| p)
}

/// n points evenly spaced by arc length along the largest polygon's exterior ring.
pub fn sample_boundary(ring: &LineString<f64>, n: usize) -> Vec<(f64, f64)> {
    let cs = &ring.0;
    let mut seglen = Vec::with_capacity(cs.len());
    let mut total = 0.0;
    for w in cs.windows(2) {
        let d = ((w[1].x - w[0].x).powi(2) + (w[1].y - w[0].y).powi(2)).sqrt();
        seglen.push(d);
        total += d;
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let mut target = total * i as f64 / n as f64;
        let mut j = 0;
        while j < seglen.len() && target > seglen[j] {
            target -= seglen[j];
            j += 1;
        }
        if j >= seglen.len() {
            out.push((cs[0].x, cs[0].y));
            continue;
        }
        let f = if seglen[j] > 0.0 { target / seglen[j] } else { 0.0 };
        out.push((
            cs[j].x + f * (cs[j + 1].x - cs[j].x),
            cs[j].y + f * (cs[j + 1].y - cs[j].y),
        ));
    }
    out
}

pub fn area_poly(p: &Poly) -> f64 {
    p.unsigned_area()
}

pub fn simplify_poly(p: &Poly, eps: f64) -> Poly {
    use geo::Simplify;
    p.simplify(&eps)
}

/// A 2x3 affine as (a,b,c,d,e,f): x' = a*x + b*y + c, y' = d*x + e*y + f.
pub type Mat = [f64; 6];

/// Compose: (p ∘ q)(v) = p(q(v)), apply q first, then p.
pub fn mat_compose(p: &Mat, q: &Mat) -> Mat {
    [
        p[0] * q[0] + p[1] * q[3],
        p[0] * q[1] + p[1] * q[4],
        p[0] * q[2] + p[1] * q[5] + p[2],
        p[3] * q[0] + p[4] * q[3],
        p[3] * q[1] + p[4] * q[4],
        p[3] * q[2] + p[4] * q[5] + p[5],
    ]
}

/// Placement transform: rotate `deg` about origin (SVG convention) then translate (tx,ty).
pub fn place_mat(deg: f64, tx: f64, ty: f64) -> Mat {
    let (s, c) = deg.to_radians().sin_cos();
    [c, -s, tx, s, c, ty]
}

/// Scale so the polygon is `target_w` wide (if given) and shift its bbox-min to the origin.
/// Returns the normalized polygon and the matrix (original coords -> normalized), which the
/// caller also applies to the *image* content so it stays aligned with the border.
pub fn normalize(p: &Poly, target_w: Option<f64>) -> (Poly, Mat) {
    let (minx, miny, maxx, _) = poly_bbox(p);
    let s = target_w.map_or(1.0, |w| w / (maxx - minx));
    let m: Mat = [s, 0.0, -s * minx, 0.0, s, -s * miny];
    let t = AffineTransform::new(m[0], m[1], m[2], m[3], m[4], m[5]);
    (p.affine_transform(&t), m)
}

/// Apply a 2x3 matrix to a polygon.
pub fn transform_poly(p: &Poly, m: &Mat) -> Poly {
    p.affine_transform(&AffineTransform::new(m[0], m[1], m[2], m[3], m[4], m[5]))
}

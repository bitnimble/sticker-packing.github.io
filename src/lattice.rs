use crate::geom::*;
use crate::greedy::Placement;
use crate::par;
use geo_types::Coord;

type Vec2 = (f64, f64);

fn det(a: Vec2, b: Vec2) -> f64 {
    a.0 * b.1 - a.1 * b.0
}

/// Gaussian lattice reduction: shortest equivalent basis (same lattice, same cell area).
fn reduce_basis(mut a: Vec2, mut b: Vec2) -> (Vec2, Vec2) {
    for _ in 0..64 {
        let aa = a.0 * a.0 + a.1 * a.1;
        if aa > 0.0 {
            let mu = ((b.0 * a.0 + b.1 * a.1) / aa).round();
            b = (b.0 - mu * a.0, b.1 - mu * a.1);
        }
        if b.0 * b.0 + b.1 * b.1 < a.0 * a.0 + a.1 * a.1 {
            std::mem::swap(&mut a, &mut b);
        } else {
            break;
        }
    }
    (a, b)
}

fn admissible(k: &Body, v1: Vec2, v2: Vec2, shells: i32) -> bool {
    for i in -shells..=shells {
        for j in -shells..=shells {
            if i == 0 && j == 0 {
                continue;
            }
            let (x, y) = (i as f64 * v1.0 + j as f64 * v2.0, i as f64 * v1.1 + j as f64 * v2.1);
            if k.contains(x, y) {
                return false;
            }
        }
    }
    true
}

/// Given row vector v1, the tightest v2: smallest cell area over v2 that clears the whole
/// v1-row. Candidates are K's vertices shifted by k*v1; admissible iff inside no K + k'*v1.
/// Cells below `min_area` are physically impossible (rejects the v2-parallel-to-v1 degeneracy).
fn best_v2(k: &Body, k_verts: &[Coord<f64>], min_area: f64, v1: Vec2) -> Option<(Vec2, f64)> {
    let v1len = (v1.0 * v1.0 + v1.1 * v1.1).sqrt();
    if v1len < 1e-9 {
        return None;
    }
    let projs: Vec<f64> = k_verts.iter().map(|c| (c.x * v1.0 + c.y * v1.1) / v1len).collect();
    let span = projs.iter().cloned().fold(f64::MIN, f64::max) - projs.iter().cloned().fold(f64::MAX, f64::min);
    let m = ((span / v1len) as i32 + 1).min(20);

    let mut best: Option<(Vec2, f64)> = None;
    for c in k_verts {
        for kk in -m..=m {
            let cand = (c.x + kk as f64 * v1.0, c.y + kk as f64 * v1.1);
            let area = det(v1, cand).abs();
            if area < min_area * (1.0 - 1e-6) {
                continue;
            }
            let inside = (-m..=m).any(|k2| k.contains(cand.0 - k2 as f64 * v1.0, cand.1 - k2 as f64 * v1.1));
            if !inside && best.map_or(true, |(_, ba)| area < ba) {
                best = Some((cand, area));
            }
        }
    }
    best
}

/// Densest admissible lattice for collision body K (min cell area). Seeded with the
/// bounding-box grid so a result always exists. K may be multipart (cluster bodies).
fn densest_lattice(k: &Multi, part_area: f64, n: usize) -> (Vec2, Vec2) {
    let body = Body::new(k);
    let main = largest(k);
    let (_, _, w, h) = poly_bbox(&main);
    let k_verts: Vec<Coord<f64>> = {
        let cs = &main.exterior().0;
        cs[..cs.len().saturating_sub(1)].to_vec()
    };
    let seed = reduce_basis((w, 0.0), (0.0, h));
    let mut best = (w * h, seed.0, seed.1);

    let v1s = sample_boundary(main.exterior(), n);
    let results: Vec<(Vec2, Vec2, f64)> =
        par::map_slice(&v1s, |&v1| best_v2(&body, &k_verts, part_area, v1).map(|(v2, a)| (v1, v2, a)))
            .into_iter()
            .flatten()
            .collect();

    for (v1, v2, area) in results {
        if area >= best.0 || area < 1e-9 {
            continue;
        }
        let (rv1, rv2) = reduce_basis(v1, v2);
        if admissible(&body, rv1, rv2, 3) {
            best = (area, rv1, rv2);
        }
    }
    (best.1, best.2)
}

/// Densest double lattice: interleave copies at orientation 0 and 180. The flipped copy just
/// touches P when the offset t is on the boundary of P (+) P; lattice-pack the pair cluster.
fn double_lattice(grown: &Poly, part_area: f64) -> (Vec2, Vec2, Vec2) {
    let tp = convex_pieces(grown);
    let k2 = largest(&minkowski(&tp, &tp));
    let ts = sample_boundary(k2.exterior(), 36);

    let results: Vec<(f64, Vec2, Vec2, Vec2)> = par::map_slice(&ts, |&t| {
            let neg = neg_pieces(&tp);
            let cluster: Vec<Piece> = tp
                .iter()
                .cloned()
                .chain(neg.iter().map(|pc| pc.iter().map(|c| Coord { x: c.x + t.0, y: c.y + t.1 }).collect()))
                .collect();
            let kc = minkowski(&cluster, &neg_pieces(&cluster));
            let (v1, v2) = densest_lattice(&kc, 2.0 * part_area, 72);
            (det(v1, v2).abs(), v1, v2, t)
        });

    let best = results
        .into_iter()
        .min_by(|a, b| a.0.partial_cmp(&b.0).unwrap())
        .unwrap();
    (best.1, best.2, best.3)
}

fn rot_vec(v: Vec2, c: f64, s: f64) -> Vec2 {
    (c * v.0 - s * v.1, s * v.0 + c * v.1)
}

/// One sublattice fitted to the page: reference-point inner-fit rect `(lox, hix, loy, hiy)`, the
/// rotated lattice offset, its angle, and keep-out reference rects `(lox, hix, loy, hiy)` the
/// reference point must avoid.
struct Sub {
    rect: (f64, f64, f64, f64),
    off: Vec2,
    ang: f64,
    forbid: Vec<(f64, f64, f64, f64)>,
}

/// Place a lattice (one or more angle-offset sublattices sharing v1,v2) onto the page,
/// searching global rotation + phase for the most copies fully inside page-minus-margin.
fn fit_to_page(
    grown: &Poly,
    v1: Vec2,
    v2: Vec2,
    sublattices: &[(f64, Vec2)],
    page_w: f64,
    page_h: f64,
    reserve: &Reserve,
    rotations: &[f64],
    phases: usize,
) -> Vec<Placement> {
    par::map_slice(rotations, |&a| {
            let (s, c) = a.to_radians().sin_cos();
            let (w1, w2) = (rot_vec(v1, c, s), rot_vec(v2, c, s));
            // per-sublattice inner-fit rect + rotated offset + keep-out reference rects
            let mut subs: Vec<Sub> = Vec::new();
            let mut ok = true;
            for &(dangle, doff) in sublattices {
                let ang = (a + dangle).rem_euclid(360.0);
                let (minx, miny, maxx, maxy) = poly_bbox(&rotate_p(grown, ang));
                let rect = (reserve.left - minx, (page_w - reserve.right) - maxx, reserve.top - miny, (page_h - reserve.bottom) - maxy);
                if rect.1 < rect.0 || rect.3 < rect.2 {
                    ok = false;
                    break;
                }
                // reference points where the part's bbox would overlap a keep-out rect
                let forbid = reserve.rects.iter().map(|k| (k[0] - maxx, k[2] - minx, k[1] - maxy, k[3] - miny)).collect();
                subs.push(Sub { rect, off: rot_vec(doff, c, s), ang, forbid });
            }
            if !ok {
                return Vec::new();
            }

            let dt = det(w1, w2);
            if dt.abs() < 1e-12 {
                return Vec::new();
            }
            // i,j range covering all rects (corners minus offsets), expanded by one cell.
            let (mut imn, mut imx, mut jmn, mut jmx) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
            for sub in &subs {
                for &cx in &[sub.rect.0, sub.rect.1] {
                    for &cy in &[sub.rect.2, sub.rect.3] {
                        let (px, py) = (cx - sub.off.0, cy - sub.off.1);
                        let i = (w2.1 * px - w2.0 * py) / dt;
                        let j = (-w1.1 * px + w1.0 * py) / dt;
                        imn = imn.min(i); imx = imx.max(i);
                        jmn = jmn.min(j); jmx = jmx.max(j);
                    }
                }
            }
            let (i0, i1) = (imn.floor() as i64 - 1, imx.ceil() as i64 + 1);
            let (j0, j1) = (jmn.floor() as i64 - 1, jmx.ceil() as i64 + 1);
            let pts0: Vec<Vec2> = (i0..=i1)
                .flat_map(|i| (j0..=j1).map(move |j| (i, j)))
                .map(|(i, j)| (i as f64 * w1.0 + j as f64 * w2.0, i as f64 * w1.1 + j as f64 * w2.1))
                .collect();

            let mut best: Vec<Placement> = Vec::new();
            for pf in 0..phases {
                for qf in 0..phases {
                    let (fx, fy) = (pf as f64 / phases as f64, qf as f64 / phases as f64);
                    let o = (fx * w1.0 + fy * w2.0, fx * w1.1 + fy * w2.1);
                    let mut placed: Vec<Placement> = Vec::new();
                    for sub in &subs {
                        let (ox, oy) = (o.0 + sub.off.0, o.1 + sub.off.1);
                        for p in &pts0 {
                            let (x, y) = (p.0 + ox, p.1 + oy);
                            if x >= sub.rect.0 && x <= sub.rect.1 && y >= sub.rect.2 && y <= sub.rect.3
                                && !sub.forbid.iter().any(|f| x >= f.0 && x <= f.1 && y >= f.2 && y <= f.3)
                            {
                                placed.push(Placement { angle: sub.ang, x, y });
                            }
                        }
                    }
                    if placed.len() > best.len() {
                        best = placed;
                    }
                }
            }
            best
        })
        .into_iter()
        .max_by_key(|p| p.len())
        .unwrap_or_default()
}

/// Best of single + double lattice, page-fit and sorted.
pub fn pack(
    grown: &Poly,
    rotations: &[f64],
    page_w: f64,
    page_h: f64,
    reserve: &Reserve,
    max_count: Option<usize>,
) -> Vec<Placement> {
    let area = area_poly(grown);
    let (v1, v2) = densest_lattice(&collision_body(grown), area, 180);
    let single = fit_to_page(grown, v1, v2, &[(0.0, (0.0, 0.0))], page_w, page_h, reserve, rotations, 12);

    let (dv1, dv2, t) = double_lattice(grown, area);
    let double = fit_to_page(grown, dv1, dv2, &[(0.0, (0.0, 0.0)), (180.0, t)], page_w, page_h, reserve, rotations, 12);

    let mut best = if single.len() >= double.len() { single } else { double };
    best.sort_by(|a, b| (a.y, a.x).partial_cmp(&(b.y, b.x)).unwrap());
    if let Some(cap) = max_count {
        best.truncate(cap);
    }
    best
}

use crate::geom::*;
use crate::par;
use geo::BooleanOps;

#[derive(Clone, Copy, Debug)]
pub struct Placement {
    pub angle: f64,
    pub x: f64,
    pub y: f64,
}

type Key = fn(f64, f64) -> (f64, f64);

// Eight fill directions (which page corner/axis the fill drives toward).
static KEYS: [Key; 8] = [
    |x, y| (y, x),
    |x, y| (y, -x),
    |x, y| (-y, x),
    |x, y| (-y, -x),
    |x, y| (x, y),
    |x, y| (x, -y),
    |x, y| (-x, y),
    |x, y| (-x, -y),
];

fn rect(lox: f64, loy: f64, hix: f64, hiy: f64) -> Multi {
    Multi::new(vec![poly_from(&[
        (lox, loy),
        (hix, loy),
        (hix, hiy),
        (lox, hiy),
    ])])
}

/// Shared, immutable precompute for a rotation set: NFP row templates and inner-fit rects.
pub struct Pre {
    pub rotations: Vec<f64>,
    /// nfp[i][m] = rotate(base[m], rotations[i]); template for placed angle i vs candidate
    /// j is nfp[i][(j-i) mod R], translated to the placement.
    nfp: Vec<Vec<Multi>>,
    /// inner-fit rectangle per rotation (None if the part cannot fit at that rotation).
    ifp: Vec<Option<Multi>>,
}

pub fn precompute(
    grown: &Poly,
    rotations: &[f64],
    page_w: f64,
    page_h: f64,
    margin: f64,
) -> Pre {
    let r = rotations.len();
    let parts: Vec<Poly> = rotations.iter().map(|&a| rotate_p(grown, a)).collect();
    let tris: Vec<Vec<Tri>> = parts.iter().map(triangulate).collect();
    let neg: Vec<Vec<Tri>> = tris.iter().map(|t| neg_tris(t)).collect();

    // base[m] = part[0] (+) (-part[m]); one Minkowski per rotation offset (identity below).
    let base: Vec<Multi> = par::map_range(r, |m| minkowski(&tris[0], &neg[m]));

    // nfp[i][m] = rotate(base[m], rotations[i]) -- all R*R templates, parallel over i.
    let nfp: Vec<Vec<Multi>> =
        par::map_range(r, |i| base.iter().map(|b| rotate_m(b, rotations[i])).collect());

    let ifp: Vec<Option<Multi>> = (0..r)
        .map(|i| {
            let (bminx, bminy, bmaxx, bmaxy) = poly_bbox(&parts[i]);
            let (lox, hix) = (margin - bminx, (page_w - margin) - bmaxx);
            let (loy, hiy) = (margin - bminy, (page_h - margin) - bmaxy);
            if hix >= lox && hiy >= loy {
                Some(rect(lox, loy, hix, hiy))
            } else {
                None
            }
        })
        .collect();

    Pre { rotations: rotations.to_vec(), nfp, ifp }
}

fn is_empty(m: &Multi) -> bool {
    m.0.is_empty()
}

/// One NFP bottom-fill toward `key`, optional forced first-piece rotation `first`.
fn fill(pre: &Pre, key: Key, first: Option<usize>, max_count: Option<usize>) -> Vec<Placement> {
    let r = pre.rotations.len();
    let mut feas: Vec<Option<Multi>> = pre.ifp.clone();
    let mut placements: Vec<Placement> = Vec::new();

    let place = |feas: &mut Vec<Option<Multi>>, placements: &mut Vec<Placement>, i: usize, x: f64, y: f64| {
        placements.push(Placement { angle: pre.rotations[i], x, y });
        for j in 0..r {
            if let Some(region) = &feas[j] {
                let blk = translate_m(&pre.nfp[i][(j + r - i) % r], x, y);
                let d = region.difference(&blk);
                feas[j] = if is_empty(&d) { None } else { Some(d) };
            }
        }
    };

    if let Some(f) = first {
        if let Some(region) = &feas[f] {
            if let Some((x, y)) = extreme_vertex(region, key) {
                place(&mut feas, &mut placements, f, x, y);
            }
        }
    }

    loop {
        if let Some(cap) = max_count {
            if placements.len() >= cap {
                break;
            }
        }
        let mut best: Option<((f64, f64), f64, f64, usize)> = None;
        for j in 0..r {
            if let Some(region) = &feas[j] {
                if let Some((x, y)) = extreme_vertex(region, key) {
                    let k = key(x, y);
                    if best.map_or(true, |(bk, ..)| k < bk) {
                        best = Some((k, x, y, j));
                    }
                }
            }
        }
        match best {
            Some((_, x, y, i)) => place(&mut feas, &mut placements, i, x, y),
            None => break,
        }
    }
    placements.sort_by(|a, b| (a.y, a.x).partial_cmp(&(b.y, b.x)).unwrap());
    placements
}

/// Multi-start greedy: 8 fill directions + forced-first orientations sampled across the
/// rotation set, run in parallel, keep the most-placed. `first` cascades hardest, so seeding
/// it is the most productive restart.
pub fn pack(
    grown: &Poly,
    rotations: &[f64],
    page_w: f64,
    page_h: f64,
    margin: f64,
    max_count: Option<usize>,
    attempts: usize,
) -> Vec<Placement> {
    let pre = precompute(grown, rotations, page_w, page_h, margin);
    let placeable: Vec<usize> = (0..rotations.len()).filter(|&i| pre.ifp[i].is_some()).collect();

    let mut configs: Vec<(Key, Option<usize>)> = KEYS.iter().map(|&k| (k, None)).collect();
    let n_seed = attempts.saturating_sub(configs.len());
    if n_seed > 0 && !placeable.is_empty() {
        let mut seen = std::collections::HashSet::new();
        for s in 0..n_seed {
            let idx = placeable[((s as f64 / n_seed as f64) * (placeable.len() as f64 - 1.0)).round() as usize];
            if seen.insert(idx) {
                configs.push((KEYS[0], Some(idx)));
            }
        }
    }
    configs.truncate(attempts);

    par::map_slice(&configs, |&(key, first)| fill(&pre, key, first, max_count))
        .into_iter()
        .max_by_key(|p| p.len())
        .unwrap_or_default()
}

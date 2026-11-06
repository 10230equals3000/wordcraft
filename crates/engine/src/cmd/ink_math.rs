//! Draw › Ink to Math and Insert › Ink Equation: a small geometric recogniser that turns
//! hand-drawn pen or pencil strokes into a UnicodeMath linear-format string. Like
//! [`ink_shape`](super::ink_shape) it is our own fit, not a trained model: each stroke is
//! reduced to a few geometric features, strokes close together are joined into a symbol, the
//! symbols are ordered by reading order, and two-dimensional layout is read as fractions,
//! radicals, superscripts and subscripts.
//!
//! The recogniser covers the hand-written forms the equation model itself can take: symbols
//! (`0`, `1`, `-`, `+`, `x`, `=`, `/`, `÷`, `≠` and `√`), a stacked fraction from a bar with
//! writing above and below, a raised or lowered smaller symbol as `^`/`_`, and a radical as
//! `√(…)`. Multi-stroke relational signs are joined from two or three strokes; hostile input is
//! bounded and anything unrecognised stays ink. The other digits and letters, and nested
//! fractions or scripts of more than one symbol, remain for a later pass.

use wordcraft_doc::freeform::MAX_POINTS;

/// A hand-drawn stroke in page points (one pen or pencil path; highlighter strokes are never
/// recognised as math).
#[derive(Clone, Debug)]
pub struct Stroke {
    pub pts: Vec<[f32; 2]>,
}

/// Maximum input retained by either ink-to-math command, independent of points per path.
pub const MAX_STROKES: usize = 256;
pub const MAX_TOTAL_POINTS: usize = 50_000;
/// Page coordinates beyond this are not useful for recognition and can overflow geometry.
pub const MAX_COORD: f32 = 1.0e6;
/// Bound recursive layout independently of the number of input strokes.
const MAX_LAYOUT_DEPTH: usize = 32;
/// The smallest standalone symbol (its longer side, points). Smaller strokes can be dots.
const MIN_SIZE: f32 = 6.0;
/// How close two strokes' vertical centres may be to join one symbol, as a fraction of the
/// larger symbol size.
const NEAR: f32 = 0.75;

/// The left-to-right UnicodeMath for `strokes`, if every stroke is part of one recognised
/// symbol or layout.
pub fn recognise(strokes: &[Stroke]) -> Option<String> {
    if strokes.is_empty() || strokes.len() > MAX_STROKES {
        return None;
    }
    let mut total = 0usize;
    for stroke in strokes {
        if stroke.pts.is_empty() || stroke.pts.len() > MAX_POINTS {
            return None;
        }
        total = total.checked_add(stroke.pts.len())?;
        if total > MAX_TOTAL_POINTS || stroke.pts.iter().flatten().any(|v| !v.is_finite() || v.abs() > MAX_COORD) {
            return None;
        }
    }
    let glyphs = build_glyphs(strokes)?;
    let refs: Vec<&Glyph> = glyphs.iter().collect();
    to_math(&refs, 0)
}

// --- Symbols and layout ------------------------------------------------------------------

/// A symbol recognised from a glyph, or a radical that takes the rest of its row as its
/// argument.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Sym {
    Char(char),
    Radical,
}

/// The UnicodeMath for a set of glyphs, with layout (a stacked fraction first, otherwise one
/// line of writing with scripts).
fn to_math(glyphs: &[&Glyph], depth: usize) -> Option<String> {
    if depth >= MAX_LAYOUT_DEPTH {
        return None;
    }
    match fraction(glyphs) {
        FractionLayout::Valid(above, below) => {
            let num = to_math(&above, depth + 1)?;
            let den = to_math(&below, depth + 1)?;
            if num.is_empty() || den.is_empty() {
                return None;
            }
            // UnicodeMath's slash only takes the adjacent operands. Keep a whole expression
            // above or below the bar together instead of changing the equation's meaning.
            let operand = |s: String| if s.chars().all(|c| c.is_ascii_alphanumeric()) { s } else { format!("({s})") };
            Some(format!("{}/{}", operand(num), operand(den)))
        }
        FractionLayout::Invalid => None,
        FractionLayout::None => line(glyphs, depth),
    }
}

/// Result of checking a possible isolated stacked fraction. `Invalid` prevents a malformed
/// fraction layout from falling back to a misleading left-to-right interpretation.
enum FractionLayout<'a> {
    None,
    Valid(Vec<&'a Glyph>, Vec<&'a Glyph>),
    Invalid,
}

/// A stacked fraction if one glyph is a long horizontal bar with writing above and below it;
/// the result is the content above over the content below.
fn fraction<'a>(glyphs: &[&'a Glyph]) -> FractionLayout<'a> {
    let max = glyphs.iter().map(|g| g.size()).fold(0.0, f32::max);
    let Some(bar) = glyphs.iter().find(|g| g.kind() == Some(GlyphKind::Bar) && g.root_w() >= 0.9 * max) else {
        return FractionLayout::None;
    };
    // A wide bar does not require correspondingly large gaps: ordinary handwriting may place
    // its numerator and denominator quite close to it. Require just a small visible separation.
    let gap = 0.03 * bar.size();
    let above: Vec<&Glyph> = glyphs.iter().copied().filter(|g| g.bottom() <= bar.top() - gap).collect();
    let below: Vec<&Glyph> = glyphs.iter().copied().filter(|g| g.top() >= bar.bottom() + gap).collect();
    if above.is_empty() || below.is_empty() {
        return FractionLayout::None;
    }
    // A fraction is an isolated layout in this first-pass recogniser. Reject glyphs beside the
    // bar rather than silently include them in its numerator or denominator; surrounding-row
    // composition needs a separate parser.
    let [bar_x, _, bar_w, _] = bar.box_();
    let x_tolerance = 0.1 * bar_w;
    if above.iter().chain(&below).any(|g| {
        let [x, _, w, _] = g.box_();
        x < bar_x - x_tolerance || x + w > bar_x + bar_w + x_tolerance
    }) {
        return FractionLayout::Invalid;
    }
    let assigned = above.len() + below.len() + 1;
    if assigned != glyphs.len() {
        return FractionLayout::Invalid;
    }
    FractionLayout::Valid(above, below)
}

/// One line of symbols left to right: superscripts and subscripts attach to the symbol before
/// them, and a radical takes the rest of the line as `√(…)`.
fn line(glyphs: &[&Glyph], depth: usize) -> Option<String> {
    if depth >= MAX_LAYOUT_DEPTH {
        return None;
    }
    if glyphs.is_empty() {
        return Some(String::new());
    }
    let mut sorted = glyphs.to_vec();
    sorted.sort_by(|a, b| a.reading_x().total_cmp(&b.reading_x()).then_with(|| a.centre_y().total_cmp(&b.centre_y())));
    if let Some(pos) = sorted.iter().position(|g| g.classify() == Some(Sym::Radical)) {
        let right = sorted[pos].radical_right()?;
        let end = sorted[pos + 1..].iter().position(|g| g.reading_x() > right).map_or(sorted.len(), |n| pos + 1 + n);
        if end == pos + 1 {
            return None;
        }
        let left = line(&sorted[..pos], depth + 1)?;
        let inside = line(&sorted[pos + 1..end], depth + 1)?;
        let tail = line(&sorted[end..], depth + 1)?;
        return Some(format!("{left}√({inside}){tail}"));
    }
    let mut out = String::new();
    let mut i = 0;
    while i < sorted.len() {
        let g = sorted[i];
        let sym = g.classify()?;
        let Sym::Char(c) = sym else { return None };
        out.push(c);
        // An attached script: a smaller symbol clearly above or below the one just written.
        let mut j = i + 1;
        while j < sorted.len() {
            let nxt = sorted[j];
            let mark = script_mark(g, nxt);
            let (Some(mark), Some(Sym::Char(c))) = (mark, nxt.classify()) else { break };
            out.push(mark);
            out.push(c);
            j += 1;
        }
        i = j;
    }
    Some(out)
}

/// `^` for a smaller symbol clearly above `base`, `_` for one clearly below, else `None`.
fn script_mark(base: &Glyph, other: &Glyph) -> Option<char> {
    if other.size() > 0.85 * base.size() {
        return None;
    }
    let dy = other.centre_y() - base.centre_y();
    if dy < -0.3 * base.size() {
        Some('^')
    } else if dy > 0.3 * base.size() {
        Some('_')
    } else {
        None
    }
}

/// What a glyph is before it is classified into a character.
#[derive(Clone, Copy, Debug, PartialEq)]
enum GlyphKind {
    /// A single horizontal stroke long enough to be a fraction bar in context.
    Bar,
}

/// A symbol made of one to three strokes that belong together.
#[derive(Clone, Debug)]
struct Glyph {
    infos: Vec<Info>,
}

impl Glyph {
    /// The union bounds of the glyph's strokes.
    fn box_(&self) -> [f32; 4] {
        let mut b = self.infos[0].box_;
        for i in &self.infos[1..] {
            let x = b[0].min(i.box_[0]);
            let y = b[1].min(i.box_[1]);
            let r = (b[0] + b[2]).max(i.box_[0] + i.box_[2]);
            let t = (b[1] + b[3]).max(i.box_[1] + i.box_[3]);
            b = [x, y, r - x, t - y];
        }
        b
    }

    fn centre_x(&self) -> f32 {
        let [x, _, w, _] = self.box_();
        x + w / 2.0
    }

    /// A radical starts at the left of its overbar, not at the centre of the whole glyph.
    fn reading_x(&self) -> f32 {
        if self.classify() == Some(Sym::Radical) { self.box_()[0] } else { self.centre_x() }
    }

    fn radical_right(&self) -> Option<f32> {
        let bar = self.infos.iter().find(|i| i.horizontal())?;
        Some(bar.box_[0] + bar.box_[2])
    }

    fn centre_y(&self) -> f32 {
        let [_, y, _, h] = self.box_();
        y + h / 2.0
    }

    fn top(&self) -> f32 {
        self.box_()[1]
    }

    fn bottom(&self) -> f32 {
        let [_, y, _, h] = self.box_();
        y + h
    }

    fn size(&self) -> f32 {
        let [_, _, w, h] = self.box_();
        w.max(h)
    }

    fn root_w(&self) -> f32 {
        self.infos.first().map_or(0.0, |i| i.box_[2])
    }

    /// Whether the glyph is a single long horizontal stroke (a candidate fraction bar) or a dot.
    fn kind(&self) -> Option<GlyphKind> {
        if self.infos.len() != 1 {
            return None;
        }
        let i = &self.infos[0];
        if i.horizontal() {
            return Some(GlyphKind::Bar);
        }
        None
    }

    /// The one character this glyph is, if it is one.
    fn classify(&self) -> Option<Sym> {
        match self.infos.len() {
            1 => single(&self.infos[0]).map(Sym::Char),
            2 => pair(&self.infos[0], &self.infos[1]),
            3 => trio(&self.infos[0], &self.infos[1], &self.infos[2]),
            _ => None,
        }
    }
}

/// One stroke = one symbol (`0`, `1`, `-` or `/`).
fn single(i: &Info) -> Option<char> {
    if i.size() < MIN_SIZE {
        return None;
    }
    if i.closed_round() {
        return Some('0');
    }
    if i.vertical() {
        return Some('1');
    }
    if i.horizontal() {
        return Some('-');
    }
    if i.diagonal() {
        return Some('/');
    }
    None
}

/// Two strokes together: `=` (two horizontal bars), `+` (a vertical crossing a horizontal), `x`
/// (two crossing diagonals) or `√` (an overbar with a descending stroke at its left end).
fn pair(a: &Info, b: &Info) -> Option<Sym> {
    if a.horizontal() && b.horizontal() && aligned_bars(a, b) {
        return Some(Sym::Char('='));
    }
    if plus(a, b) {
        return Some(Sym::Char('+'));
    }
    if ex(a, b) {
        return Some(Sym::Char('x'));
    }
    if radical(a, b) {
        return Some(Sym::Radical);
    }
    None
}

/// Three strokes together: `÷` (a bar with a dot above and below) or `≠` (two bars crossed by a
/// slash).
fn trio(a: &Info, b: &Info, c: &Info) -> Option<Sym> {
    if div(a, b, c) || div(b, a, c) || div(c, a, b) {
        return Some(Sym::Char('÷'));
    }
    if neq(a, b, c) || neq(b, a, c) || neq(c, a, b) {
        return Some(Sym::Char('≠'));
    }
    None
}

/// Two horizontal bars that sit one above the other and are close in width make `=`.
fn aligned_bars(a: &Info, b: &Info) -> bool {
    let (lo, hi) = if a.centre_y() <= b.centre_y() { (&a.box_, &b.box_) } else { (&b.box_, &a.box_) };
    let ratio = lo[2] / hi[2];
    let overlap = (lo[0] + lo[2]).min(hi[0] + hi[2]) - lo[0].max(hi[0]);
    let separation = (a.centre_y() - b.centre_y()).abs();
    (0.4..=2.5).contains(&ratio)
        && overlap >= 0.5 * lo[2].min(hi[2])
        && separation >= 0.08 * lo[2].min(hi[2])
        && separation <= NEAR * hi[2].max(lo[2])
}

/// Whether one stroke is vertical and the other horizontal and they cross near their middles.
/// The crossing must be interior to both strokes — a bar merely touching or sitting above a
/// near-vertical stroke (a fraction bar with its head, or a radical's overbar with its leg) is
/// not a plus sign.
fn plus(a: &Info, b: &Info) -> bool {
    let (v, h) = match (a.vertical(), a.horizontal(), b.vertical(), b.horizontal()) {
        (true, false, false, true) => (a, b),
        (false, true, true, false) => (b, a),
        _ => return false,
    };
    let vy0 = v.box_[1];
    let vy1 = v.box_[1] + v.box_[3];
    let hx0 = h.box_[0];
    let hx1 = h.box_[0] + h.box_[2];
    let margin = 0.2 * v.size().min(h.size());
    let hy = h.centre_y();
    let vx = v.centre_x();
    hy > vy0 + margin && hy < vy1 - margin && vx > hx0 + margin && vx < hx1 - margin
}

/// Whether two diagonal strokes cross near their middles, one leaning the other way.
fn ex(a: &Info, b: &Info) -> bool {
    if !a.diagonal() || !b.diagonal() {
        return false;
    }
    let (sa, sb) = (slope(a), slope(b));
    if !sa.is_finite() || !sb.is_finite() || !(sa * sb).is_sign_negative() {
        return false;
    }
    let Some(p) = line_intersect(a.first, a.last, b.first, b.last) else { return false };
    let tol = 0.3 * a.size().max(b.size());
    seg_distance(a.first, a.last, std::slice::from_ref(&p)) <= tol && seg_distance(b.first, b.last, std::slice::from_ref(&p)) <= tol
}

/// A radical: a horizontal overbar with a stroke descending from its left end (the stroke may
/// be a shallow diagonal or nearly vertical).
fn radical(a: &Info, b: &Info) -> bool {
    let (bar, other) = match (a.horizontal(), b.horizontal()) {
        (true, false) => (a, b),
        (false, true) => (b, a),
        _ => return false,
    };
    if !other.vertical() && !other.diagonal() {
        return false;
    }
    let (top, bottom) = upper_lower(other);
    if bottom[1] - top[1] < 0.2 * other.size() {
        return false;
    }
    let left = [bar.box_[0], bar.centre_y()];
    dist(left, top) <= 0.35 * bar.size().min(other.size())
}

/// The upper and lower ends of a stroke by their y coordinate.
fn upper_lower(i: &Info) -> ([f32; 2], [f32; 2]) {
    if i.first[1] <= i.last[1] { (i.first, i.last) } else { (i.last, i.first) }
}

/// `÷`: one horizontal bar with a small dot above and one below, near the bar's centre.
fn div(bar: &Info, a: &Info, b: &Info) -> bool {
    if !bar.horizontal() {
        return false;
    }
    let (up, down) = if a.centre_y() <= bar.centre_y() { (a, b) } else { (b, a) };
    up.centre_y() < bar.centre_y()
        && down.centre_y() > bar.centre_y()
        && is_dot(bar, up)
        && is_dot(bar, down)
        && (up.centre_y() - bar.centre_y()).abs().max((down.centre_y() - bar.centre_y()).abs()) <= 1.2 * bar.size()
        && (up.centre_x() - bar.centre_x()).abs() <= 0.35 * bar.size()
        && (down.centre_x() - bar.centre_x()).abs() <= 0.35 * bar.size()
}

/// A dot small enough to sit above or below a `÷` bar: a short tick or a small closed ring.
fn is_dot(bar: &Info, dot: &Info) -> bool {
    let s = dot.size();
    // A mark big enough to be a standalone glyph is ambiguous with a fraction's numerator or
    // denominator; never consume it as a dot. A genuine tap has zero size and length.
    s < MIN_SIZE && s <= 0.35 * bar.size() && (dot.closed_round() || dot.length <= 1.5 * s)
}

/// `≠`: an equals sign (`=`) with a diagonal slash crossing between its bars.
fn neq(a: &Info, b: &Info, slash: &Info) -> bool {
    if !slash.diagonal() {
        return false;
    }
    let Some((bar_a, bar_b)) = (aligned_bars(a, b)).then_some((a, b)) else { return false };
    let top = bar_a.centre_y().min(bar_b.centre_y());
    let bottom = bar_a.centre_y().max(bar_b.centre_y());
    let left = bar_a.box_[0].min(bar_b.box_[0]);
    let right = (bar_a.box_[0] + bar_a.box_[2]).max(bar_b.box_[0] + bar_b.box_[2]);
    let p = [slash.centre_x(), slash.centre_y()];
    p[0] >= left && p[0] <= right && p[1] >= top && p[1] <= bottom
}

fn slope(i: &Info) -> f32 {
    let dx = i.last[0] - i.first[0];
    let dy = i.last[1] - i.first[1];
    if dx.abs() < 1e-3 { f32::INFINITY } else { dy / dx }
}

// --- Stroke preparation and geometry -----------------------------------------------------

/// Group strokes into symbols, sorted into reading order (left to right, top to bottom) and
/// then composing adjacent strokes by recognised relation (a crossing, an aligned pair, a bar
/// with dots), never by mere proximity — a fraction's bar must stay separate from its head and
/// tail until layout.
fn build_glyphs(strokes: &[Stroke]) -> Option<Vec<Glyph>> {
    // Do not silently drop taps, malformed strokes or other unpreparable input: callers may
    // delete the source ink after successful recognition, so every input must be accounted for.
    let mut infos: Vec<Info> = strokes.iter().map(prepare).collect::<Option<Vec<_>>>()?;
    if infos.is_empty() {
        return None;
    }
    infos.sort_by(|a, b| a.centre_x().total_cmp(&b.centre_x()).then_with(|| a.centre_y().total_cmp(&b.centre_y())));
    let mut glyphs: Vec<Glyph> = Vec::new();
    let mut used = vec![false; infos.len()];
    // A radicand can lie between a radical leg and its overbar in centre-X order. Pair the
    // connected strokes first, even when other glyphs lie under the bar.
    for i in 0..infos.len() {
        if used[i] || infos[i].horizontal() {
            continue;
        }
        let bar = (0..infos.len()).find(|&j| !used[j] && j != i && radical(&infos[i], &infos[j]) && !plus(&infos[i], &infos[j]));
        if let Some(j) = bar {
            used[i] = true;
            used[j] = true;
            glyphs.push(Glyph { infos: vec![infos[i].clone(), infos[j].clone()] });
        }
    }
    let infos: Vec<Info> = infos.into_iter().enumerate().filter_map(|(i, info)| (!used[i]).then_some(info)).collect();
    let mut i = 0;
    while i < infos.len() {
        if i + 2 < infos.len() && trio(&infos[i], &infos[i + 1], &infos[i + 2]).is_some() {
            glyphs.push(Glyph { infos: vec![infos[i].clone(), infos[i + 1].clone(), infos[i + 2].clone()] });
            i += 3;
            continue;
        }
        if i + 1 < infos.len() && pair(&infos[i], &infos[i + 1]).is_some() {
            glyphs.push(Glyph { infos: vec![infos[i].clone(), infos[i + 1].clone()] });
            i += 2;
            continue;
        }
        if single(&infos[i]).is_some() {
            glyphs.push(Glyph { infos: vec![infos[i].clone()] });
            i += 1;
            continue;
        }
        return None;
    }
    Some(glyphs)
}

/// One stroke, reduced to what recognition needs.
#[derive(Clone, Debug)]
struct Info {
    box_: [f32; 4],
    pts: Vec<[f32; 2]>,
    first: [f32; 2],
    last: [f32; 2],
    length: f32,
    gap: f32,
}

impl Info {
    fn centre_x(&self) -> f32 {
        self.box_[0] + self.box_[2] / 2.0
    }

    fn centre_y(&self) -> f32 {
        self.box_[1] + self.box_[3] / 2.0
    }

    fn size(&self) -> f32 {
        self.box_[2].max(self.box_[3])
    }

    /// A closed, roughly round stroke (an `0`).
    fn closed_round(&self) -> bool {
        self.gap <= 0.25 * self.size() && ellipse_rms(&self.pts, self.box_) < 0.09
    }

    /// A straight line whose points barely leave the chord between its ends.
    fn straight(&self, tol: f32) -> bool {
        seg_distance(self.first, self.last, &self.pts) <= tol
    }

    /// A vertical line (a `1`): tall, narrow, straight and open.
    fn vertical(&self) -> bool {
        let [_, _, w, h] = self.box_;
        h >= MIN_SIZE && w <= 0.3 * h && self.gap >= 0.7 * self.length && self.straight(0.14 * h)
    }

    /// A horizontal line (a `-`): long, flat, straight and open.
    fn horizontal(&self) -> bool {
        let [_, _, w, h] = self.box_;
        w >= MIN_SIZE && h <= 0.3 * w && self.gap >= 0.7 * self.length && self.straight(0.14 * w)
    }

    /// A diagonal line (`/`): open and straight at a slope away from both axes.
    fn diagonal(&self) -> bool {
        let [_, _, w, h] = self.box_;
        w >= MIN_SIZE
            && h >= MIN_SIZE
            && (h / w).abs() > 0.3
            && (h / w).abs() < 3.0
            && self.gap >= 0.7 * self.length
            && self.straight(0.14 * self.size())
    }
}

/// Prepare a stroke for recognition, retaining taps so they can participate in a division sign.
fn prepare(s: &Stroke) -> Option<Info> {
    let pts = clean(&s.pts);
    let first = *pts.first()?;
    let last = *pts.last()?;
    let box_ = bbox(&pts)?;
    Some(Info { box_, length: length(&pts), gap: dist(first, last), pts, first, last })
}

/// Deduplicate nearly coincident points (a stroke's pauses collapse into one).
fn clean(pts: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let mut out: Vec<[f32; 2]> = Vec::new();
    for p in pts {
        if out.last().is_none_or(|l| dist(*l, *p) > 0.5) {
            out.push(*p);
        }
        if out.len() >= MAX_POINTS {
            break;
        }
    }
    out
}

/// Left, top, width, height of `p`.
fn bbox(p: &[[f32; 2]]) -> Option<[f32; 4]> {
    let first = *p.first()?;
    let (mut x0, mut y0, mut x1, mut y1) = (first[0], first[1], first[0], first[1]);
    for q in p {
        x0 = x0.min(q[0]);
        y0 = y0.min(q[1]);
        x1 = x1.max(q[0]);
        y1 = y1.max(q[1]);
    }
    Some([x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)])
}

/// Total path length.
fn length(p: &[[f32; 2]]) -> f32 {
    p.windows(2).map(|w| dist(w[0], w[1])).sum()
}

fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    (b[0] - a[0]).hypot(b[1] - a[1])
}

/// Largest distance of any point of `p` from the segment `a`–`b`.
fn seg_distance(a: [f32; 2], b: [f32; 2], p: &[[f32; 2]]) -> f32 {
    p.iter().map(|q| point_seg(a, b, *q)).fold(0.0, f32::max)
}

fn point_seg(a: [f32; 2], b: [f32; 2], p: [f32; 2]) -> f32 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dx * dx + dy * dy;
    if len2 <= f32::EPSILON {
        return dist(a, p);
    }
    let t = (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0);
    dist(p, [a[0] + t * dx, a[1] + t * dy])
}

/// Root-mean-square distance of `pts` from the unit ellipse inscribed in its bounds.
fn ellipse_rms(pts: &[[f32; 2]], [x, y, w, h]: [f32; 4]) -> f32 {
    if w < 1.0 || h < 1.0 {
        return f32::INFINITY;
    }
    let (cx, cy, rx, ry) = (x + w / 2.0, y + h / 2.0, w / 2.0, h / 2.0);
    let mut n = 0usize;
    let mut sum = 0.0f32;
    for q in pts {
        let nx = (q[0] - cx) / rx;
        let ny = (q[1] - cy) / ry;
        let e = (nx * nx + ny * ny).sqrt() - 1.0;
        sum += e * e;
        n += 1;
    }
    if n == 0 {
        return f32::INFINITY;
    }
    (sum / n as f32).sqrt()
}

/// The point where the infinite lines through the two chords meet, if they are not parallel.
fn line_intersect(a0: [f32; 2], a1: [f32; 2], b0: [f32; 2], b1: [f32; 2]) -> Option<[f32; 2]> {
    let (x0, y0) = (a0[0], a0[1]);
    let (x1, y1) = (a1[0], a1[1]);
    let (x2, y2) = (b0[0], b0[1]);
    let (x3, y3) = (b1[0], b1[1]);
    let denom = (x1 - x0) * (y3 - y2) - (y1 - y0) * (x3 - x2);
    if denom.abs() < 1e-9 {
        return None;
    }
    let t = ((x2 - x0) * (y3 - y2) - (y2 - y0) * (x3 - x2)) / denom;
    Some([x0 + t * (x1 - x0), y0 + t * (y1 - y0)])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(from: [f32; 2], to: [f32; 2]) -> Vec<[f32; 2]> {
        let n = 24;
        (0..=n)
            .map(|i| {
                let t = i as f32 / n as f32;
                [from[0] + t * (to[0] - from[0]), from[1] + t * (to[1] - from[1])]
            })
            .collect()
    }

    fn circle(cx: f32, cy: f32, r: f32) -> Vec<[f32; 2]> {
        let n = 40;
        (0..=n)
            .map(|i| {
                let a = std::f32::consts::TAU * i as f32 / n as f32;
                [cx + r * a.cos(), cy + r * a.sin()]
            })
            .collect()
    }

    fn s(pts: Vec<[f32; 2]>) -> Stroke {
        Stroke { pts }
    }

    #[test]
    fn recognises_one_minus_one_equals_zero() {
        let strokes = [
            s(line([0.0, 10.0], [0.0, 40.0])),
            s(line([14.0, 25.0], [30.0, 25.0])),
            s(line([40.0, 10.0], [40.0, 40.0])),
            s(line([50.0, 22.0], [66.0, 22.0])),
            s(line([50.0, 30.0], [66.0, 30.0])),
            s(circle(80.0, 25.0, 12.0)),
        ];
        assert_eq!(recognise(&strokes).as_deref(), Some("1-1=0"));
    }

    #[test]
    fn recognises_a_two_stroke_plus() {
        let strokes = [
            s(line([0.0, 10.0], [0.0, 40.0])),
            s(line([10.0, 4.0], [10.0, 46.0])),
            s(line([2.0, 24.0], [18.0, 24.0])),
            s(line([30.0, 10.0], [30.0, 40.0])),
        ];
        assert_eq!(recognise(&strokes).as_deref(), Some("1+1"));
    }

    #[test]
    fn recognises_two_crossing_diagonals_as_x() {
        let strokes = [s(line([10.0, 30.0], [30.0, 10.0])), s(line([10.0, 10.0], [30.0, 30.0]))];
        assert_eq!(recognise(&strokes).as_deref(), Some("x"));
    }

    #[test]
    fn a_tap_stays_ink() {
        assert_eq!(recognise(&[s(vec![[0.0, 0.0], [0.2, 0.2]])]), None);
    }

    #[test]
    fn two_scribbles_are_not_math() {
        let strokes = [s(vec![[0.0, 0.0], [4.0, 6.0], [8.0, 2.0], [12.0, 8.0]]), s(vec![[20.0, 0.0], [24.0, 8.0], [28.0, 0.0], [32.0, 8.0]])];
        assert_eq!(recognise(&strokes), None);
    }

    #[test]
    fn tap_cannot_be_dropped_from_a_valid_expression() {
        let strokes = [s(line([0.0, 10.0], [0.0, 40.0])), s(vec![[20.0, 10.0], [20.2, 10.2]])];
        assert_eq!(recognise(&strokes), None);
    }

    #[test]
    fn recognises_divide_and_not_equal_from_three_strokes() {
        // ÷ : a bar with a dot above and below.
        let div = [s(line([10.0, 25.0], [30.0, 25.0])), s(circle(20.0, 8.0, 2.0)), s(circle(20.0, 42.0, 2.0))];
        assert_eq!(recognise(&div).as_deref(), Some("÷"));
        let taps = [s(line([10.0, 25.0], [30.0, 25.0])), s(vec![[20.0, 18.0]]), s(vec![[20.0, 32.0]])];
        assert_eq!(recognise(&taps).as_deref(), Some("÷"));
        // ≠ : an equals sign with a diagonal slash across it.
        let neq = [s(line([10.0, 20.0], [30.0, 20.0])), s(line([10.0, 30.0], [30.0, 30.0])), s(line([10.0, 34.0], [30.0, 16.0]))];
        assert_eq!(recognise(&neq).as_deref(), Some("≠"));
    }

    #[test]
    fn recognises_a_stacked_fraction() {
        let strokes = [
            s(line([0.0, 0.0], [0.0, 24.0])),     // numerator 1
            s(line([-14.0, 30.0], [14.0, 30.0])), // fraction bar
            s(circle(0.0, 55.0, 12.0)),           // denominator 0
        ];
        assert_eq!(recognise(&strokes).as_deref(), Some("1/0"));
    }

    #[test]
    fn fraction_layout_does_not_discard_glyphs_outside_the_bar() {
        let strokes = [
            s(line([-40.0, 0.0], [-40.0, 24.0])), // leading 1
            s(line([0.0, 0.0], [0.0, 24.0])),     // numerator 1
            s(line([-14.0, 30.0], [14.0, 30.0])), // fraction bar
            s(circle(0.0, 55.0, 12.0)),           // denominator 0
        ];
        assert_eq!(recognise(&strokes), None, "until full mixed-layout reading is supported, reject rather than lose the leading glyph");
    }

    #[test]
    fn fractions_keep_complete_numerators_and_do_not_become_division_signs() {
        for (bar_y, den_y) in [(30.0, 45.0), (35.0, 50.0)] {
            let one_over_one =
                [s(line([30.0, 0.0], [30.0, 20.0])), s(line([0.0, bar_y], [80.0, bar_y])), s(line([30.0, den_y], [30.0, den_y + 20.0]))];
            assert_eq!(recognise(&one_over_one).as_deref(), Some("1/1"));
        }

        let sum_over_eleven = [
            s(line([7.0, 0.0], [7.0, 35.0])),
            s(line([15.0, 17.0], [25.0, 17.0])),
            s(line([20.0, 5.0], [20.0, 30.0])),
            s(line([35.0, 0.0], [35.0, 35.0])),
            s(line([0.0, 50.0], [80.0, 50.0])),
            s(line([25.0, 70.0], [25.0, 105.0])),
            s(line([45.0, 70.0], [45.0, 105.0])),
        ];
        let linear = recognise(&sum_over_eleven).unwrap();
        assert_eq!(linear, "(1+1)/11");
        let nodes = wordcraft_doc::math::parse_linear(&linear);
        let [wordcraft_doc::math::MNode::Frac { num, den, .. }] = nodes.as_slice() else { panic!("not a single fraction: {nodes:?}") };
        assert_eq!(wordcraft_doc::math::to_linear(num), "1+1");
        assert_eq!(wordcraft_doc::math::to_linear(den), "11");

        let zero_over_sum = [
            s(circle(35.0, 17.0, 17.0)),
            s(line([0.0, 50.0], [80.0, 50.0])),
            s(line([7.0, 65.0], [7.0, 100.0])),
            s(line([15.0, 82.0], [25.0, 82.0])),
            s(line([20.0, 70.0], [20.0, 95.0])),
            s(line([35.0, 65.0], [35.0, 100.0])),
        ];
        let nodes = wordcraft_doc::math::parse_linear(&recognise(&zero_over_sum).unwrap());
        let [wordcraft_doc::math::MNode::Frac { num, den, .. }] = nodes.as_slice() else { panic!("not a single fraction: {nodes:?}") };
        assert_eq!(wordcraft_doc::math::to_linear(num), "0");
        assert_eq!(wordcraft_doc::math::to_linear(den), "1+1");
    }

    #[test]
    fn separate_minus_signs_do_not_become_equals() {
        let strokes = [
            s(line([0.0, 10.0], [0.0, 40.0])),
            s(line([14.0, 25.0], [30.0, 25.0])),
            s(line([45.0, 25.0], [61.0, 25.0])),
            s(line([75.0, 10.0], [75.0, 40.0])),
        ];
        assert_eq!(recognise(&strokes).as_deref(), Some("1--1"));
    }

    #[test]
    fn input_budgets_and_coordinates_are_checked_before_recognition() {
        let stroke = s(line([0.0, 0.0], [0.0, 20.0]));
        assert_eq!(recognise(&vec![stroke; MAX_STROKES + 1]), None);
        let many_points = s(vec![[0.0, 0.0]; MAX_POINTS]);
        assert_eq!(recognise(&vec![many_points; MAX_TOTAL_POINTS / MAX_POINTS + 1]), None);
        assert_eq!(recognise(&[s(vec![[0.0, 0.0], [f32::INFINITY, 20.0]])]), None);
        assert_eq!(recognise(&[s(vec![[0.0, 0.0], [MAX_COORD * 2.0, 20.0]])]), None);
    }

    #[test]
    fn recognises_superscripts_and_subscripts() {
        let strokes = [
            s(line([0.0, 0.0], [0.0, 30.0])), // base 1
            s(circle(18.0, -8.0, 3.0)),       // raised 0 → 1^0
        ];
        assert_eq!(recognise(&strokes).as_deref(), Some("1^0"));
        let strokes = [
            s(line([0.0, 0.0], [0.0, 30.0])),    // base 1
            s(line([18.0, 44.0], [18.0, 22.0])), // lowered 1 → 1_1
        ];
        assert_eq!(recognise(&strokes).as_deref(), Some("1_1"));
    }

    #[test]
    fn recognises_a_radical_with_its_radicand_under_the_bar() {
        let strokes = [
            s(line([0.0, 0.0], [40.0, 0.0])),   // overbar
            s(line([0.0, 0.0], [7.0, 24.0])),   // diagonal at the bar's left end
            s(line([18.0, 5.0], [18.0, 25.0])), // radicand 1, before the bar's centre
        ];
        assert_eq!(recognise(&strokes).as_deref(), Some("√(1)"));
        let mut followed = strokes.to_vec();
        followed.extend([s(line([42.0, 12.0], [58.0, 12.0])), s(line([50.0, 2.0], [50.0, 22.0])), s(line([75.0, 0.0], [75.0, 24.0]))]);
        assert_eq!(recognise(&followed).as_deref(), Some("√(1)+1"));
    }
}

//! The member tree's lines, one canvas per member row under the PM. Two kinds
//! of traffic, told apart by color, rhythm and direction:
//! - working: blue dashes run down from the PM into the member (work going
//!   out); the trunk above it runs too.
//! - waiting for you: red dots climb from the member back up to the PM (a
//!   question coming back). Where the trunk is shared, both run on one line.
//!
//! Dashes are anchored to the tree's own y, so they line up across rows, and
//! their phase follows the clock, so a re-render never makes them jump.

use crate::style;
use gpui::*;
use std::sync::OnceLock;
use std::time::Instant;

pub const ROW_H: f32 = style::MEMBER_ROW;
/// From one member row's top to the next: the row and the gap under it, which its canvas covers too.
pub const PITCH: f32 = style::MEMBER_ROW + style::MEMBER_GAP;
/// The canvas sits this far left of its row (the trunk's x).
pub const LEFT: f32 = -15.;
pub const WIDTH: f32 = 18.;
const FLOW_P: f32 = 11.5;
const FLOW_DASH: f32 = 4.;
const FLOW_MS: f32 = 600.;
const ASK_P: f32 = 6.5;
const ASK_MS: f32 = 1100.;

/// What one row's canvas draws.
#[derive(Clone, Copy)]
pub struct Branch {
    pub index: usize,
    pub last: bool,
    /// The trunk past this row runs blue / red (someone further down is working / waiting).
    pub trunk_flow: bool,
    pub trunk_ask: bool,
    /// The trunk is already lit blue, so the red glow under the dots is skipped there.
    pub trunk_lit: bool,
    pub working: bool,
    pub waiting: bool,
}

impl Branch {
    /// Extra height over the row for the first branch: the trunk starts under the
    /// PM's tile, across the gap under the PM's row.
    fn top(&self) -> f32 {
        if self.index == 0 { 12. + style::MEMBER_GAP } else { 0. }
    }
    pub fn height(&self) -> f32 {
        PITCH + self.top()
    }
    pub fn moving(&self) -> bool {
        self.trunk_flow || self.trunk_ask || self.working || self.waiting
    }
}

/// The lines for every crew member, from their statuses in order.
pub fn branches(statuses: &[String]) -> Vec<Branch> {
    let last_working = statuses.iter().rposition(|s| s == "working");
    let last_waiting = statuses.iter().rposition(|s| s == "blocked");
    let n = statuses.len();
    statuses
        .iter()
        .enumerate()
        .map(|(i, s)| Branch {
            index: i,
            last: i + 1 == n,
            trunk_flow: last_working.is_some_and(|w| i < w),
            trunk_ask: last_waiting.is_some_and(|w| i < w),
            trunk_lit: last_working.is_some_and(|w| i < w),
            working: s == "working",
            waiting: s == "blocked",
        })
        .collect()
}

fn clock_ms() -> f32 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f32() * 1000.
}

/// A polyline with its length so far at each point.
struct Line {
    pts: Vec<(f32, f32)>,
    at: Vec<f32>,
}

impl Line {
    fn new(pts: Vec<(f32, f32)>) -> Line {
        let mut at = vec![0.];
        for w in pts.windows(2) {
            let d = ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt();
            at.push(at.last().unwrap() + d);
        }
        Line { pts, at }
    }
    fn len(&self) -> f32 {
        *self.at.last().unwrap_or(&0.)
    }
    fn point(&self, s: f32) -> (f32, f32) {
        let s = s.clamp(0., self.len());
        let i = self.at.iter().rposition(|&a| a <= s).unwrap_or(0).min(self.pts.len().saturating_sub(2));
        let span = (self.at[i + 1] - self.at[i]).max(1e-4);
        let t = (s - self.at[i]) / span;
        let (a, b) = (self.pts[i], self.pts[i + 1]);
        (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
    }
    /// The part between two lengths, as points.
    fn part(&self, a: f32, b: f32) -> Vec<(f32, f32)> {
        let mut out = vec![self.point(a)];
        out.extend(self.pts.iter().zip(&self.at).filter(|(_, s)| **s > a && **s < b).map(|(p, _)| *p));
        out.push(self.point(b));
        out
    }
}

/// M1 0 V(mid−8) Q1 mid 9 mid H17: down the trunk, curving into the member.
fn elbow(top: f32) -> Line {
    let mid = top + ROW_H / 2.;
    let mut pts = vec![(1., 0.), (1., mid - 8.)];
    for k in 1..=8 {
        let t = k as f32 / 8.;
        let (p0, p1, p2) = ((1., mid - 8.), (1., mid), (9., mid));
        let u = 1. - t;
        pts.push((u * u * p0.0 + 2. * u * t * p1.0 + t * t * p2.0, u * u * p0.1 + 2. * u * t * p1.1 + t * t * p2.1));
    }
    pts.push((17., mid));
    Line::new(pts)
}

fn trunk(h: f32) -> Line {
    Line::new(vec![(1., 0.), (1., h)])
}

fn stroke(line: &[(f32, f32)], origin: Point<Pixels>, width: f32, color: Hsla, window: &mut Window) {
    let mut b = PathBuilder::stroke(px(width));
    let p = |(x, y): (f32, f32)| origin + point(px(x), px(y));
    b.move_to(p(line[0]));
    for &q in &line[1..] {
        b.line_to(p(q));
    }
    if let Ok(path) = b.build() {
        window.paint_path(path, color);
    }
}

/// Blue dashes moving forward along the line (down, then into the member).
fn flow(line: &Line, y0: f32, origin: Point<Pixels>, window: &mut Window) {
    let t = (clock_ms() % FLOW_MS) / FLOW_MS;
    let offset = y0.rem_euclid(FLOW_P) - FLOW_P * t;
    let mut s = -offset - FLOW_P;
    while s < line.len() {
        let (a, b) = (s.max(0.), (s + FLOW_DASH).min(line.len()));
        if b > a {
            stroke(&line.part(a, b), origin, 1.5, style::blue(), window);
        }
        s += FLOW_P;
    }
}

/// Red dots climbing back along the line (out of the member, up the trunk).
fn ask(line: &Line, y0: f32, origin: Point<Pixels>, window: &mut Window) {
    let t = (clock_ms() % ASK_MS) / ASK_MS;
    let offset = y0.rem_euclid(ASK_P) + ASK_P * t;
    let mut s = -offset - ASK_P;
    while s < line.len() + 1. {
        if s >= 0. && s <= line.len() {
            let (x, y) = line.point(s);
            let c = origin + point(px(x - 1.), px(y - 1.));
            window.paint_quad(fill(Bounds::new(c, size(px(2.), px(2.))), style::red()).corner_radii(px(1.)));
        }
        s += ASK_P;
    }
}

/// The canvas for one row; place it absolutely at (LEFT, −top) in the row.
pub fn canvas_for(b: Branch) -> impl IntoElement {
    let top = b.top();
    let h = b.height();
    let y0 = b.index as f32 * PITCH - top;
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let o = bounds.origin;
            let line = gpui::Hsla::from(rgba(0xffffff21));
            let lit = style::blue().opacity(0.28);
            let lit_ask = style::red().opacity(0.26);
            let el = elbow(top);
            let mid = top + ROW_H / 2.;
            stroke(&el.pts, o, 1.5, line, window);
            if !b.last {
                stroke(&[(1., mid - 8.), (1., h)], o, 1.5, line, window);
            }
            let tr = trunk(h);
            if b.trunk_flow {
                stroke(&tr.pts, o, 1.5, lit, window);
                flow(&tr, y0, o, window);
            }
            if b.working {
                stroke(&el.pts, o, 1.5, lit, window);
                flow(&el, y0, o, window);
            }
            if b.trunk_ask {
                if !b.trunk_lit {
                    stroke(&tr.pts, o, 1.5, lit_ask, window);
                }
                ask(&tr, y0, o, window);
            }
            if b.waiting {
                stroke(&el.pts, o, 1.5, lit_ask, window);
                ask(&el, y0, o, window);
            }
            if b.moving() {
                window.request_animation_frame();
            }
        },
    )
    .absolute()
    .left(px(LEFT))
    .top(px(-top))
    .w(px(WIDTH))
    .h(px(h))
}

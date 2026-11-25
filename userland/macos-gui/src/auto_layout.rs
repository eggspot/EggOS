//! Auto Layout's arithmetic (D769): one axis of a view placed by constraints with priorities
//! and relations (the stronger ones first, a weaker one that cannot hold yields), the baselines
//! of a line of text in a box, which languages read right to left, and a stack view's arranged
//! subviews placed by its distribution and alignment. Lengths run from the axis's start: the
//! left or the leading edge across, the top down.

use alloc::vec::Vec;

/// `NSLayoutRelation`'s values.
pub mod relation {
    pub const LESS_OR_EQUAL: i64 = -1;
    pub const EQUAL: i64 = 0;
    pub const GREATER_OR_EQUAL: i64 = 1;
}

/// A constraint every solution keeps.
pub const REQUIRED: f32 = 1000.0;
/// How strongly a view keeps to its intrinsic size against growing, unless it says otherwise.
pub const DEFAULT_HUGGING: f32 = 250.0;
/// How strongly a view keeps to its intrinsic size against shrinking, unless it says otherwise.
pub const DEFAULT_COMPRESSION_RESISTANCE: f32 = 750.0;
/// The standard space between neighbouring views (`systemSpacing`).
pub const SYSTEM_SPACING: f64 = 8.0;

const CLOSE_ENOUGH: f64 = 0.001;

/// Which quantity of an axis a constraint pins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Start,
    End,
    Length,
    Centre,
}

impl Part {
    fn weights(self) -> (f64, f64) {
        match self {
            Part::Start => (1.0, 0.0),
            Part::End => (1.0, 1.0),
            Part::Length => (0.0, 1.0),
            Part::Centre => (1.0, 0.5),
        }
    }

    /// The part's value for an axis starting at `start` and `length` long.
    pub fn value_at(self, start: f64, length: f64) -> f64 {
        let (on_start, on_length) = self.weights();
        start * on_start + length * on_length
    }
}

/// What one constraint wants of an axis: `part` related to `value` by `relation`, at
/// `priority` (1 to 1000).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Want {
    pub part: Part,
    pub value: f64,
    pub relation: i64,
    pub priority: f32,
}

impl Want {
    fn holds(self, start: f64, length: f64) -> bool {
        let actual = self.part.value_at(start, length);
        match self.relation {
            relation::LESS_OR_EQUAL => actual <= self.value + CLOSE_ENOUGH,
            relation::GREATER_OR_EQUAL => actual >= self.value - CLOSE_ENOUGH,
            _ => (actual - self.value).abs() <= CLOSE_ENOUGH,
        }
    }
}

fn allows(bounds: &[Want], part: Part, value: f64) -> bool {
    bounds
        .iter()
        .filter(|bound| bound.part == part)
        .all(|bound| match bound.relation {
            relation::LESS_OR_EQUAL => value <= bound.value + CLOSE_ENOUGH,
            _ => value >= bound.value - CLOSE_ENOUGH,
        })
}

fn solve_pair(first: (Part, f64), second: (Part, f64)) -> Option<(f64, f64)> {
    let (a, b) = first.0.weights();
    let (c, d) = second.0.weights();
    let determinant = a * d - b * c;
    if determinant.abs() < CLOSE_ENOUGH {
        return None;
    }
    let start = (first.1 * d - b * second.1) / determinant;
    let length = (a * second.1 - first.1 * c) / determinant;
    Some((start, length))
}

fn clamp_free(
    bounds: &[Want],
    fixed: (Part, f64),
    preferred: f64,
    free_is_length: bool,
) -> (f64, f64) {
    let (fixed_on_start, fixed_on_length) = fixed.0.weights();
    let place = |free: f64| match free_is_length {
        true => ((fixed.1 - fixed_on_length * free) / fixed_on_start, free),
        false => (free, (fixed.1 - fixed_on_start * free) / fixed_on_length),
    };
    let mut low = f64::NEG_INFINITY;
    let mut high = f64::INFINITY;
    for bound in bounds {
        let at_zero = bound.part.value_at(place(0.0).0, place(0.0).1);
        let at_one = bound.part.value_at(place(1.0).0, place(1.0).1);
        let slope = at_one - at_zero;
        if slope.abs() < CLOSE_ENOUGH {
            continue;
        }
        let edge = (bound.value - at_zero) / slope;
        let raises_low = (bound.relation == relation::GREATER_OR_EQUAL) == (slope > 0.0);
        let (next_low, next_high) = match raises_low {
            true => (low.max(edge), high),
            false => (low, high.min(edge)),
        };
        if next_low <= next_high + CLOSE_ENOUGH {
            low = next_low;
            high = next_high;
        }
    }
    place(preferred.max(low).min(high.max(low)))
}

/// Where an axis's start goes and how long it is, under `wants` (in any order): the strongest
/// first, each equality pinning a part unless the parts pinned before already decide it
/// otherwise, each inequality kept unless the pinned parts break it; then the axis keeps
/// `length` (its frame's or intrinsic length) and `start` as far as the kept inequalities let
/// it. Of two wants of the same priority, the first given wins.
pub fn solve(wants: &[Want], start: f64, length: f64) -> (f64, f64) {
    let mut order: Vec<Want> = wants.to_vec();
    order.sort_by(|a, b| b.priority.total_cmp(&a.priority));
    let mut pinned: Vec<(Part, f64)> = Vec::with_capacity(2);
    let mut decided: Option<(f64, f64)> = None;
    let mut bounds: Vec<Want> = Vec::new();
    for want in order {
        if let Some((start, length)) = decided {
            if want.relation != relation::EQUAL && want.holds(start, length) {
                bounds.push(want);
            }
            continue;
        }
        if want.relation != relation::EQUAL {
            bounds.push(want);
            continue;
        }
        match pinned.first().copied() {
            None if allows(&bounds, want.part, want.value) => {
                pinned.push((want.part, want.value))
            }
            None => {}
            Some((part, _)) if part == want.part => {}
            Some(first) => {
                let both = solve_pair(first, (want.part, want.value)).filter(|&(start, length)| {
                    bounds.iter().all(|bound| bound.holds(start, length))
                });
                if let Some(both) = both {
                    pinned.push((want.part, want.value));
                    decided = Some(both);
                }
            }
        }
    }
    if let Some((start, length)) = decided {
        return (start, length.max(0.0));
    }
    let (start, length) = match pinned.first().copied() {
        Some(fixed) if fixed.0 == Part::Length => clamp_free(&bounds, fixed, start, false),
        Some(fixed) => clamp_free(&bounds, fixed, length, true),
        None => {
            let length_bounds: Vec<Want> = bounds
                .iter()
                .copied()
                .filter(|bound| bound.part == Part::Length)
                .collect();
            let length = clamp_free(&length_bounds, (Part::Start, 0.0), length, true).1;
            clamp_free(&bounds, (Part::Length, length), start, false)
        }
    };
    (start, length.max(0.0))
}

/// The wants a view's intrinsic `length` adds to its axis: no longer than it at the view's
/// `hugging` priority, no shorter at its `resistance` priority.
pub fn intrinsic_wants(length: f64, hugging: f32, resistance: f32) -> [Want; 2] {
    [
        Want {
            part: Part::Length,
            value: length,
            relation: relation::LESS_OR_EQUAL,
            priority: hugging,
        },
        Want {
            part: Part::Length,
            value: length,
            relation: relation::GREATER_OR_EQUAL,
            priority: resistance,
        },
    ]
}

/// Where a line of text sits in a box `height` tall, centred: its first baseline's distance
/// from the box's top and its last baseline's from the bottom, for a font of `ascender` and
/// `descender` (negative below the baseline).
pub fn baseline_offsets(height: f64, ascender: f64, descender: f64) -> (f64, f64) {
    let line = ascender - descender;
    let gap = ((height - line) / 2.0).max(0.0);
    (gap + ascender, gap - descender)
}

/// Whether the language of locale name `locale` (`ar_EG.UTF-8`, `he`, `fa-IR`) is written
/// right to left.
pub fn is_right_to_left(locale: &[u8]) -> bool {
    let language: &[u8] = locale
        .split(|&byte| matches!(byte, b'_' | b'-' | b'.' | b'@'))
        .next()
        .unwrap_or(&[]);
    matches!(
        language,
        b"ar" | b"he" | b"iw" | b"fa" | b"ur" | b"yi" | b"ji" | b"ps" | b"sd" | b"ug" | b"dv"
            | b"ckb" | b"ku" | b"syr" | b"arc"
    )
}

/// How a stack view shares its length among its arranged subviews.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Distribution {
    GravityAreas,
    Fill,
    FillEqually,
    FillProportionally,
    EqualSpacing,
    EqualCentering,
}

impl Distribution {
    /// `NSStackViewDistribution`'s and `UIStackViewDistribution`'s value `raw`.
    pub fn from_raw(raw: i64) -> Self {
        match raw {
            -1 => Distribution::GravityAreas,
            1 => Distribution::FillEqually,
            2 => Distribution::FillProportionally,
            3 => Distribution::EqualSpacing,
            4 => Distribution::EqualCentering,
            _ => Distribution::Fill,
        }
    }
}

/// Where a stack view puts each arranged subview across its axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alignment {
    Fill,
    Start,
    Centre,
    End,
    FirstBaseline,
    LastBaseline,
}

impl Alignment {
    /// `UIStackViewAlignment`'s value `raw`.
    pub fn from_uikit(raw: i64) -> Self {
        match raw {
            1 => Alignment::Start,
            2 => Alignment::FirstBaseline,
            3 => Alignment::Centre,
            4 => Alignment::End,
            5 => Alignment::LastBaseline,
            _ => Alignment::Fill,
        }
    }

    /// `NSStackView`'s alignment, an `NSLayoutAttribute` `raw`; none is the centre.
    pub fn from_appkit(raw: i64) -> Self {
        match raw {
            1 | 3 | 5 => Alignment::Start,
            2 | 4 | 6 => Alignment::End,
            7 | 8 => Alignment::Fill,
            11 => Alignment::LastBaseline,
            12 => Alignment::FirstBaseline,
            _ => Alignment::Centre,
        }
    }
}

/// An arranged subview as a stack view sees it: its own length along the axis and across it
/// (0 when it has none), its hugging priority along the axis, and its first and last
/// baselines' distances from its top and bottom.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Arranged {
    pub length: f64,
    pub cross: f64,
    pub hugging: f32,
    pub first_baseline: f64,
    pub last_baseline: f64,
}

/// Where an arranged subview goes: its start and length along the axis, and across it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slot {
    pub start: f64,
    pub length: f64,
    pub cross_start: f64,
    pub cross_length: f64,
}

fn stretched(views: &[Arranged]) -> usize {
    let mut chosen = views.len().saturating_sub(1);
    for (index, view) in views.iter().enumerate() {
        if view.hugging <= views[chosen].hugging {
            chosen = index;
        }
    }
    chosen
}

fn fitted_lengths(views: &[Arranged], room: f64, stretch: bool) -> Vec<f64> {
    let mut lengths: Vec<f64> = views.iter().map(|view| view.length.max(0.0)).collect();
    let used: f64 = lengths.iter().sum();
    let extra = room - used;
    if views.is_empty() {
        return lengths;
    }
    let chosen = stretched(views);
    if extra > 0.0 {
        if stretch {
            lengths[chosen] += extra;
        }
        return lengths;
    }
    let mut shortage = -extra;
    let order = core::iter::once(chosen).chain((0..views.len()).rev().filter(|&at| at != chosen));
    for index in order {
        let taken = lengths[index].min(shortage);
        lengths[index] -= taken;
        shortage -= taken;
    }
    lengths
}

fn main_axis(
    views: &[Arranged],
    length: f64,
    spacing: f64,
    distribution: Distribution,
) -> Vec<(f64, f64)> {
    let count = views.len();
    if count == 0 {
        return Vec::new();
    }
    let gaps = spacing * (count - 1) as f64;
    let room = (length - gaps).max(0.0);
    let packed = |lengths: Vec<f64>, gap: f64| {
        let mut at = 0.0;
        lengths
            .into_iter()
            .map(|own| {
                let slot = (at, own);
                at += own + gap;
                slot
            })
            .collect::<Vec<_>>()
    };
    match distribution {
        Distribution::Fill => packed(fitted_lengths(views, room, true), spacing),
        Distribution::GravityAreas => packed(fitted_lengths(views, room, false), spacing),
        Distribution::FillEqually => packed(alloc::vec![room / count as f64; count], spacing),
        Distribution::FillProportionally => {
            let total: f64 = views.iter().map(|view| view.length.max(0.0)).sum();
            let lengths = views
                .iter()
                .map(|view| match total > 0.0 {
                    true => room * view.length.max(0.0) / total,
                    false => room / count as f64,
                })
                .collect();
            packed(lengths, spacing)
        }
        Distribution::EqualSpacing => {
            let lengths = fitted_lengths(views, room, false);
            let used: f64 = lengths.iter().sum();
            let gap = match count > 1 {
                true => ((length - used) / (count - 1) as f64).max(spacing),
                false => spacing,
            };
            packed(lengths, gap)
        }
        Distribution::EqualCentering => {
            let lengths = fitted_lengths(views, room, false);
            if count == 1 {
                return packed(lengths, spacing);
            }
            let first = lengths[0] / 2.0;
            let last = length - lengths[count - 1] / 2.0;
            let step = (last - first) / (count - 1) as f64;
            let centred: Vec<(f64, f64)> = lengths
                .iter()
                .enumerate()
                .map(|(index, &own)| (first + step * index as f64 - own / 2.0, own))
                .collect();
            let overlaps = centred
                .windows(2)
                .any(|pair| pair[0].0 + pair[0].1 + spacing > pair[1].0 + CLOSE_ENOUGH);
            match overlaps {
                true => packed(lengths, spacing),
                false => centred,
            }
        }
    }
}

/// Where each of `views` goes in a stack `length` long and `cross` across, `spacing` apart,
/// by `distribution` and `alignment`; baselines line up across a horizontal stack only
/// (`horizontal`), a vertical one puts baseline-aligned views at the start.
pub fn stack(
    views: &[Arranged],
    (length, cross): (f64, f64),
    spacing: f64,
    distribution: Distribution,
    alignment: Alignment,
    horizontal: bool,
) -> Vec<Slot> {
    let along = main_axis(views, length, spacing, distribution);
    let own_cross = |view: &Arranged| match view.cross > 0.0 {
        true => view.cross.min(cross),
        false => cross,
    };
    let first_line = views
        .iter()
        .map(|view| view.first_baseline)
        .fold(0.0, f64::max);
    let last_line = views
        .iter()
        .map(|view| own_cross(view) - view.last_baseline)
        .fold(0.0, f64::max);
    views
        .iter()
        .zip(along)
        .map(|(view, (start, own))| {
            let across = own_cross(view);
            let (cross_start, cross_length) = match (alignment, horizontal) {
                (Alignment::Fill, _) => (0.0, cross),
                (Alignment::Start, _) => (0.0, across),
                (Alignment::Centre, _) => ((cross - across) / 2.0, across),
                (Alignment::End, _) => (cross - across, across),
                (Alignment::FirstBaseline, true) => (first_line - view.first_baseline, across),
                (Alignment::LastBaseline, true) => {
                    (last_line - (across - view.last_baseline), across)
                }
                (_, false) => (0.0, across),
            };
            Slot {
                start,
                length: own,
                cross_start,
                cross_length,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn want(part: Part, value: f64, relation: i64, priority: f32) -> Want {
        Want {
            part,
            value,
            relation,
            priority,
        }
    }

    #[test]
    fn required_edges_beat_a_weaker_width_and_an_inequality_clamps_the_rest() {
        let wants = [
            want(Part::Length, 50.0, relation::EQUAL, 500.0),
            want(Part::Start, 10.0, relation::EQUAL, REQUIRED),
            want(Part::End, 210.0, relation::EQUAL, REQUIRED),
        ];
        assert_eq!(solve(&wants, 0.0, 30.0), (10.0, 200.0));
        let at_least = [
            want(Part::Start, 20.0, relation::EQUAL, REQUIRED),
            want(Part::Length, 120.0, relation::GREATER_OR_EQUAL, REQUIRED),
        ];
        assert_eq!(solve(&at_least, 0.0, 80.0), (20.0, 120.0));
        let at_most = [
            want(Part::Start, 20.0, relation::EQUAL, REQUIRED),
            want(Part::End, 100.0, relation::LESS_OR_EQUAL, REQUIRED),
        ];
        assert_eq!(solve(&at_most, 0.0, 300.0), (20.0, 80.0));
    }

    #[test]
    fn intrinsic_size_yields_to_required_edges_and_holds_against_weak_ones() {
        let mut wants = Vec::from(intrinsic_wants(60.0, DEFAULT_HUGGING, 750.0));
        wants.push(want(Part::Start, 0.0, relation::EQUAL, REQUIRED));
        wants.push(want(Part::End, 100.0, relation::EQUAL, 200.0));
        assert_eq!(solve(&wants, 5.0, 0.0), (0.0, 60.0));
        wants.push(want(Part::End, 100.0, relation::EQUAL, REQUIRED));
        assert_eq!(solve(&wants, 5.0, 0.0), (0.0, 100.0));
        let centred = [want(Part::Centre, 50.0, relation::EQUAL, REQUIRED)];
        assert_eq!(solve(&centred, 0.0, 20.0), (40.0, 20.0));
    }

    #[test]
    fn baselines_and_right_to_left_languages() {
        assert_eq!(baseline_offsets(24.0, 12.0, -4.0), (16.0, 8.0));
        assert!(is_right_to_left(b"ar_EG.UTF-8"));
        assert!(is_right_to_left(b"he"));
        assert!(!is_right_to_left(b"en_US.UTF-8"));
        assert!(!is_right_to_left(b"C"));
    }

    fn arranged(length: f64, cross: f64, hugging: f32) -> Arranged {
        Arranged {
            length,
            cross,
            hugging,
            first_baseline: cross,
            last_baseline: 0.0,
        }
    }

    #[test]
    fn each_distribution_shares_three_hundred_points() {
        let views = [
            arranged(40.0, 20.0, 250.0),
            arranged(60.0, 30.0, 200.0),
            arranged(40.0, 10.0, 250.0),
        ];
        let starts = |distribution| {
            stack(&views, (300.0, 40.0), 10.0, distribution, Alignment::Fill, true)
                .iter()
                .map(|slot| (slot.start, slot.length))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            starts(Distribution::Fill),
            vec![(0.0, 40.0), (50.0, 200.0), (260.0, 40.0)]
        );
        assert_eq!(
            starts(Distribution::FillEqually)[1],
            (280.0 / 3.0 + 10.0, 280.0 / 3.0)
        );
        assert_eq!(starts(Distribution::FillProportionally)[1], (90.0, 120.0));
        assert_eq!(
            starts(Distribution::EqualSpacing),
            vec![(0.0, 40.0), (120.0, 60.0), (260.0, 40.0)]
        );
        assert_eq!(starts(Distribution::EqualCentering)[1], (120.0, 60.0));
        assert_eq!(starts(Distribution::GravityAreas)[2], (120.0, 40.0));
    }

    #[test]
    fn alignments_place_views_across_and_baselines_line_up() {
        let views = [arranged(40.0, 20.0, 250.0), arranged(40.0, 30.0, 250.0)];
        let across = |alignment| {
            stack(&views, (100.0, 40.0), 0.0, Distribution::Fill, alignment, true)
                .iter()
                .map(|slot| (slot.cross_start, slot.cross_length))
                .collect::<Vec<_>>()
        };
        assert_eq!(across(Alignment::Fill), vec![(0.0, 40.0), (0.0, 40.0)]);
        assert_eq!(across(Alignment::Centre), vec![(10.0, 20.0), (5.0, 30.0)]);
        assert_eq!(across(Alignment::End), vec![(20.0, 20.0), (10.0, 30.0)]);
        assert_eq!(across(Alignment::FirstBaseline), vec![(10.0, 20.0), (0.0, 30.0)]);
        assert_eq!(across(Alignment::LastBaseline), vec![(10.0, 20.0), (0.0, 30.0)]);
        assert_eq!(Alignment::from_appkit(0), Alignment::Centre);
        assert_eq!(Alignment::from_uikit(0), Alignment::Fill);
        assert_eq!(Distribution::from_raw(-1), Distribution::GravityAreas);
    }
}

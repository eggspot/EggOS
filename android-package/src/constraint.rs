//! `ConstraintLayout`'s placement of its children (D766): each child's horizontal and vertical
//! sides constrained to the parent's or a sibling's sides with margins; a child held on both
//! sides sits between its anchors by its bias (centred by default), one held on one side sits
//! beside that anchor, an unconstrained one at the parent's start. Children placed after the
//! siblings they depend on; a cycle leaves the rest at the start.

use alloc::vec::Vec;

/// Which side of its target an anchor is: the start (left or top) or the end (right or
/// bottom).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Start,
    End,
}

/// What a constraint holds on to: the parent, or the sibling with this view id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Parent,
    Sibling(u32),
}

/// One side's constraint: the target and which of its edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anchor {
    pub target: Target,
    pub edge: Edge,
}

/// One axis of a child's constraints: its start side's and end side's anchors, the margins in
/// pixels, and the bias between them in thousandths (500 centred).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Axis {
    pub start: Option<Anchor>,
    pub end: Option<Anchor>,
    pub start_margin: i32,
    pub end_margin: i32,
    pub bias: i32,
}

impl Default for Axis {
    fn default() -> Self {
        Self {
            start: None,
            end: None,
            start_margin: 0,
            end_margin: 0,
            bias: 500,
        }
    }
}

/// A child to place: its view id (0 for none), its size and its two axes' constraints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Child {
    pub id: u32,
    pub size: (i32, i32),
    pub horizontal: Axis,
    pub vertical: Axis,
}

fn edge_of(position: i32, length: i32, edge: Edge) -> i32 {
    match edge {
        Edge::Start => position,
        Edge::End => position + length,
    }
}

fn anchor_place(
    anchor: Anchor,
    parent: i32,
    children: &[Child],
    placed: &[Option<(i32, i32)>],
    axis: usize,
) -> Option<i32> {
    match anchor.target {
        Target::Parent => Some(edge_of(0, parent, anchor.edge)),
        Target::Sibling(id) => {
            let index = children.iter().position(|child| child.id == id && id != 0)?;
            let position = placed[index]?;
            let (at, length) = match axis {
                0 => (position.0, children[index].size.0),
                _ => (position.1, children[index].size.1),
            };
            Some(edge_of(at, length, anchor.edge))
        }
    }
}

fn place_axis(
    child: &Child,
    axis: usize,
    parent: i32,
    children: &[Child],
    placed: &[Option<(i32, i32)>],
) -> Option<i32> {
    let constraints = match axis {
        0 => child.horizontal,
        _ => child.vertical,
    };
    let length = match axis {
        0 => child.size.0,
        _ => child.size.1,
    };
    let start = match constraints.start {
        Some(anchor) => Some(anchor_place(anchor, parent, children, placed, axis)?),
        None => None,
    };
    let end = match constraints.end {
        Some(anchor) => Some(anchor_place(anchor, parent, children, placed, axis)?),
        None => None,
    };
    Some(match (start, end) {
        (Some(start), Some(end)) => {
            let low = start + constraints.start_margin;
            let high = end - constraints.end_margin;
            let room = high - low - length;
            low + room * constraints.bias.clamp(0, 1000) / 1000
        }
        (Some(start), None) => start + constraints.start_margin,
        (None, Some(end)) => end - constraints.end_margin - length,
        (None, None) => constraints.start_margin,
    })
}

/// Each child's top-left corner in a parent `parent` pixels wide and high.
pub fn solve(children: &[Child], parent: (i32, i32)) -> Vec<(i32, i32)> {
    let mut placed: Vec<Option<(i32, i32)>> = alloc::vec![None; children.len()];
    for _ in 0..=children.len() {
        let mut progressed = false;
        for (index, child) in children.iter().enumerate() {
            if placed[index].is_some() {
                continue;
            }
            let x = place_axis(child, 0, parent.0, children, &placed);
            let y = place_axis(child, 1, parent.1, children, &placed);
            if let (Some(x), Some(y)) = (x, y) {
                placed[index] = Some((x, y));
                progressed = true;
            }
        }
        if !progressed {
            break;
        }
    }
    placed
        .into_iter()
        .map(|position| position.unwrap_or((0, 0)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor(target: Target, edge: Edge) -> Option<Anchor> {
        Some(Anchor { target, edge })
    }

    #[test]
    fn a_centred_title_and_a_button_below_it_on_the_right() {
        let title = Child {
            id: 1,
            size: (100, 20),
            horizontal: Axis {
                start: anchor(Target::Parent, Edge::Start),
                end: anchor(Target::Parent, Edge::End),
                ..Axis::default()
            },
            vertical: Axis {
                start: anchor(Target::Parent, Edge::Start),
                start_margin: 16,
                ..Axis::default()
            },
        };
        let button = Child {
            id: 2,
            size: (80, 30),
            horizontal: Axis {
                end: anchor(Target::Parent, Edge::End),
                end_margin: 8,
                ..Axis::default()
            },
            vertical: Axis {
                start: anchor(Target::Sibling(1), Edge::End),
                start_margin: 4,
                ..Axis::default()
            },
        };
        let positions = solve(&[button, title], (300, 400));
        assert_eq!(positions[1], (100, 16));
        assert_eq!(positions[0], (212, 40));
    }

    #[test]
    fn a_bias_moves_a_held_child_and_a_cycle_falls_back_to_the_start() {
        let biased = Child {
            id: 1,
            size: (100, 10),
            horizontal: Axis {
                start: anchor(Target::Parent, Edge::Start),
                end: anchor(Target::Parent, Edge::End),
                bias: 250,
                ..Axis::default()
            },
            ..Child::default()
        };
        assert_eq!(solve(&[biased], (300, 100))[0], (50, 0));
        let first = Child {
            id: 1,
            horizontal: Axis {
                start: anchor(Target::Sibling(2), Edge::End),
                ..Axis::default()
            },
            ..Child::default()
        };
        let second = Child {
            id: 2,
            horizontal: Axis {
                start: anchor(Target::Sibling(1), Edge::End),
                ..Axis::default()
            },
            ..Child::default()
        };
        assert_eq!(solve(&[first, second], (100, 100)), vec![(0, 0), (0, 0)]);
    }
}

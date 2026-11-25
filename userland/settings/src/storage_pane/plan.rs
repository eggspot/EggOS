//! The Storage pane's plan panel (D783; docs/ux/installer-dual-boot-v0.1.md sections 2 and 3):
//! on a disk that holds partitions, `Install EggOS...` first offers installing alongside what
//! is there, with a slider for EggOS's size and a map of the disk, or using the whole disk.
//! `Next` leads to the confirmation, which `Back` leaves for the plan again as it was.

use super::{BUTTON_SIZE, CAPACITY, PADDING, add, at, format_panel, line, show_line};
use alloc::string::String;
use alloc::vec::Vec;
use eggos_files::partitions::{self, DiskLayout, Placement, TableKind};
use eggos_toolkit::canvas::Canvas;
use eggos_toolkit::geometry::Rect;
use eggos_toolkit::theme::Token;
use eggos_toolkit::tree::{WidgetId, WidgetTree};
use eggos_toolkit::widget::{Kind, Length, Place};

const PANEL: (i32, i32) = (400, 276);
const PANEL_X: i32 = 32;
const MAP: (i32, i32, i32, i32) = (16, 124, 368, 16);
const SEGMENTS: usize = 8;
const SMALLEST_SEGMENT: i32 = 4;
const CHOICES: [&str; 2] = ["Install alongside", "Use the whole disk"];
const ALONGSIDE: usize = 0;
const WHOLE_DISK: usize = 1;
const KEPT: u32 = 0;
const EGGOS: u32 = 1;
const ESP_BYTES: u64 = 256 << 20;
const GIB: u64 = 1 << 30;

/// How many widgets the plan panel adds (with the confirmation's `Back`).
pub const WIDGETS: usize = 22;

fn map_segment(canvas: &mut Canvas<'_>, rect: Rect, data: u32) {
    let palette = canvas.palette();
    match data {
        EGGOS => canvas.fill(rect, palette.accent),
        _ => {
            canvas.fill(rect, palette.menu);
            canvas.outline(rect, &[], 1, palette.text_dim);
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PlanWidgets {
    pub panel: WidgetId,
    title: WidgetId,
    found: WidgetId,
    choice: WidgetId,
    segments: [WidgetId; SEGMENTS],
    legend: WidgetId,
    gets: WidgetId,
    pub slider: WidgetId,
    keeps: WidgetId,
    pub cancel: WidgetId,
    pub next: WidgetId,
    refusal: [WidgetId; 2],
    pub back: WidgetId,
}

/// The plan as the panel left it: the disk, what is on it, where EggOS goes and how big.
#[derive(Debug, Clone)]
pub(super) struct Plan {
    pub device: &'static str,
    pub short: &'static str,
    pub layout: DiskLayout,
    pub placement: Option<Placement>,
    pub size_gib: u64,
    pub whole_disk: bool,
}

impl Plan {
    /// Whether `layout` holds anything worth a plan: a table with a partition.
    pub fn wanted(layout: &DiskLayout) -> bool {
        matches!(layout.table, TableKind::Gpt | TableKind::Mbr) && !layout.partitions.is_empty()
    }

    pub fn new(device: &'static str, short: &'static str, layout: DiskLayout) -> Self {
        let placement = partitions::place(&layout).ok();
        let size_gib = placement.as_ref().map_or(0, partitions::opening_size_gib);
        Self {
            device,
            short,
            whole_disk: placement.is_none(),
            layout,
            placement,
            size_gib,
        }
    }

    /// The confirmation's lines for installing alongside.
    pub fn confirmation(&self) -> Option<[String; 5]> {
        let placement = self.placement.as_ref().filter(|_| !self.whole_disk)?;
        let texts =
            partitions::plan_texts(&self.layout, self.device, self.short, placement, self.size_gib);
        Some(texts.confirmation)
    }
}

pub(super) fn build(
    tree: &mut WidgetTree<CAPACITY>,
    pane: WidgetId,
    install_panel: WidgetId,
) -> PlanWidgets {
    let panel = add(
        tree,
        pane,
        at(PANEL_X, 28, PANEL.0, PANEL.1),
        Kind::Panel {
            fill: None,
            border_right: None,
        },
    );
    add(
        tree,
        panel,
        Place::fill(),
        Kind::Custom {
            painter: format_panel,
            data: 0,
        },
    );
    let width = PANEL.0 - 2 * PADDING;
    let title = line(tree, panel, PADDING, 16, width);
    let found = line(tree, panel, PADDING, 36, width);
    let choice = add(
        tree,
        panel,
        Place::at(PADDING, 60, Length::Px(width), Length::Px(56)),
        Kind::radios(&CHOICES, ALONGSIDE, false),
    );
    let segments = core::array::from_fn(|_| {
        add(
            tree,
            panel,
            at(MAP.0, MAP.1, SMALLEST_SEGMENT, MAP.3),
            Kind::Custom {
                painter: map_segment,
                data: KEPT,
            },
        )
    });
    let legend = line(tree, panel, PADDING, 146, width);
    let gets = add(
        tree,
        panel,
        Place::at(PADDING, 176, Length::Content, Length::Content),
        Kind::label("EggOS gets", Token::Text),
    );
    let slider = add(
        tree,
        panel,
        at(112, 170, 272, 28),
        Kind::slider("EggOS size", 1, 1, 1, 1, " GB"),
    );
    let keeps = line(tree, panel, PADDING, 206, width);
    let cancel = add(
        tree,
        panel,
        at(176, 232, BUTTON_SIZE.0, BUTTON_SIZE.1),
        Kind::button("Cancel", false),
    );
    let next = add(
        tree,
        panel,
        at(280, 232, BUTTON_SIZE.0, BUTTON_SIZE.1),
        Kind::button("Next", true),
    );
    let refusal = [line(tree, panel, PADDING, 166, width), line(tree, panel, PADDING, 186, width)];
    let back = add(
        tree,
        install_panel,
        at(PADDING, 160, BUTTON_SIZE.0, BUTTON_SIZE.1),
        Kind::button("Back", false),
    );
    tree.set_shown(panel, false);
    tree.set_shown(back, false);
    PlanWidgets {
        panel,
        title,
        found,
        choice,
        segments,
        legend,
        gets,
        slider,
        keeps,
        cancel,
        next,
        refusal,
        back,
    }
}

/// Widths for `bytes`, in order, sharing `room` pixels: each at least [`SMALLEST_SEGMENT`],
/// the rest in proportion, the largest taking the rounding.
fn segment_widths(bytes: &[u64], room: i32) -> Vec<i32> {
    let count = bytes.len() as i32;
    if count == 0 {
        return Vec::new();
    }
    let gaps = count - 1;
    let shared = (room - gaps - SMALLEST_SEGMENT * count).max(0);
    let total: u64 = bytes.iter().sum::<u64>().max(1);
    let mut widths: Vec<i32> = bytes
        .iter()
        .map(|&each| {
            SMALLEST_SEGMENT + (u128::from(each) * shared as u128 / u128::from(total)) as i32
        })
        .collect();
    let used: i32 = widths.iter().sum::<i32>() + gaps;
    if let Some(largest) = (0..widths.len()).max_by_key(|&index| bytes[index]) {
        widths[largest] += room - used;
    }
    widths
}

impl PlanWidgets {
    /// Shows `plan` in the panel: the title, what was found, the choice, the map, the slider and
    /// the lines under it.
    pub fn show(&self, tree: &mut WidgetTree<CAPACITY>, plan: &Plan) {
        show_line(
            tree,
            self.title,
            &alloc::format!("Install EggOS on {}", plan.device),
            Token::Text,
        );
        let placement = plan.placement.as_ref();
        let texts = placement.map(|placement| {
            partitions::plan_texts(&plan.layout, plan.device, plan.short, placement, plan.size_gib)
        });
        let found = texts.as_ref().map_or_else(
            || alloc::format!("Found: {} partitions.", plan.layout.partitions.len()),
            |texts| texts.found.clone(),
        );
        show_line(tree, self.found, &found, Token::TextDim);
        tree.set_selected(
            self.choice,
            match plan.whole_disk {
                true => WHOLE_DISK,
                false => ALONGSIDE,
            },
        );
        tree.set_enabled(self.choice, placement.is_some());
        let alongside_shown = !plan.whole_disk && placement.is_some();
        for id in [self.gets, self.slider] {
            tree.set_shown(id, alongside_shown);
        }
        if let Some(placement) = placement {
            let (smallest, largest) = partitions::size_range_gib(placement);
            tree.change(self.slider, |kind| {
                if let Kind::Slider(slider) = kind {
                    slider.min = smallest as i32;
                    slider.max = largest.max(smallest) as i32;
                    slider.value = (plan.size_gib as i32).clamp(slider.min, slider.max);
                }
            });
            tree.set_enabled(self.slider, largest > smallest);
        }
        let refused = placement.is_none();
        for id in self.refusal {
            tree.set_shown(id, refused);
        }
        if refused && let Err(refusal) = partitions::place(&plan.layout) {
            let (first, second) = partitions::refusal_lines(refusal, &plan.layout);
            show_line(tree, self.refusal[0], &first, Token::TextDim);
            show_line(tree, self.refusal[1], &second, Token::TextDim);
        }
        let total: u64 = plan.layout.partitions.iter().map(|each| each.size_bytes()).sum::<u64>()
            + plan.layout.unallocated.iter().map(|run| run.size_bytes()).sum::<u64>();
        let (legend, keeps, keeps_ink) = match (&texts, plan.whole_disk) {
            (Some(texts), false) => (texts.legend.clone(), texts.keeps.clone(), Token::TextDim),
            _ => {
                let erased = partitions::erased_systems_text(&plan.layout).unwrap_or_else(|| {
                    alloc::format!("{} partitions", plan.layout.partitions.len())
                });
                (
                    alloc::format!(
                        "EFI 256 MB \u{b7} EggOS {} GB",
                        total.saturating_sub(ESP_BYTES) / GIB
                    ),
                    alloc::format!("Erases everything: {erased}."),
                    Token::Text,
                )
            }
        };
        show_line(tree, self.legend, &legend, Token::TextDim);
        show_line(tree, self.keeps, &keeps, keeps_ink);
        self.show_map(tree, plan);
    }

    fn show_map(&self, tree: &mut WidgetTree<CAPACITY>, plan: &Plan) {
        let mut pieces: Vec<(u64, u64, u32)> = Vec::new();
        match (plan.whole_disk, plan.placement.as_ref()) {
            (false, Some(placement)) => {
                let taken = plan.size_gib * GIB;
                let into = match *placement {
                    Placement::ShrunkTail { partition, .. } => Some(partition),
                    Placement::Unallocated(_) => None,
                };
                for partition in &plan.layout.partitions {
                    let mut bytes = partition.size_bytes();
                    if into == Some(partition.number) {
                        bytes = bytes.saturating_sub(taken);
                        pieces.push((partition.first_sector, bytes, KEPT));
                        pieces.push((partition.first_sector + 1, taken, EGGOS));
                        continue;
                    }
                    pieces.push((partition.first_sector, bytes, KEPT));
                }
                if let Placement::Unallocated(run) = placement {
                    pieces.push((run.first_sector, taken.min(run.size_bytes()), EGGOS));
                }
            }
            _ => {
                let total: u64 = plan.layout.partitions.iter().map(|each| each.size_bytes()).sum();
                pieces.push((0, ESP_BYTES, EGGOS));
                pieces.push((1, total.saturating_sub(ESP_BYTES), EGGOS));
            }
        }
        pieces.sort_by_key(|piece| piece.0);
        pieces.truncate(SEGMENTS);
        let bytes: Vec<u64> = pieces.iter().map(|piece| piece.1).collect();
        let widths = segment_widths(&bytes, MAP.2);
        let mut x = MAP.0;
        for (index, id) in self.segments.iter().enumerate() {
            let Some((&width, piece)) = widths.get(index).zip(pieces.get(index)) else {
                tree.set_shown(*id, false);
                continue;
            };
            tree.set_shown(*id, true);
            tree.set_place(*id, at(x, MAP.1, width, MAP.3));
            let kind = piece.2;
            tree.change(*id, |widget| {
                if let Kind::Custom { data, .. } = widget {
                    *data = kind;
                }
            });
            x += width + 1;
        }
    }

    /// Reads the choice and the slider back into `plan`.
    pub fn read(&self, tree: &WidgetTree<CAPACITY>, plan: &mut Plan) {
        plan.whole_disk =
            plan.placement.is_none() || tree.selected(self.choice) == Some(WHOLE_DISK);
        if let Some(value) = tree.value(self.slider) {
            plan.size_gib = value.max(1) as u64;
        }
    }

    /// The radios, for the pane's responses.
    pub fn choice(&self) -> WidgetId {
        self.choice
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_are_four_pixels_at_least_and_fill_the_map() {
        let widths = segment_widths(&[1, 1000, 10], 368);
        assert!(widths.iter().all(|&width| width >= SMALLEST_SEGMENT));
        assert_eq!(widths.iter().sum::<i32>() + 2, 368);
        assert!(widths[1] > widths[2]);
    }
}

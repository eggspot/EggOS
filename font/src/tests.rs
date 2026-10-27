use super::*;
use std::fmt::Write;

const DESIGN_FILE: &str = include_str!("../../docs/ux/font-8x16.txt");

fn design_glyphs() -> Vec<(u32, GlyphRows)> {
    let mut glyphs = Vec::new();
    let mut lines = DESIGN_FILE.lines().peekable();
    while let Some(line) = lines.next() {
        let Some(header) = line.strip_prefix("U+") else {
            continue;
        };
        let codepoint = u32::from_str_radix(&header[..4], 16).unwrap();
        let mut rows = [0u8; GLYPH_HEIGHT];
        for row in &mut rows {
            let line = lines.next().unwrap();
            assert_eq!(line.len(), GLYPH_WIDTH, "U+{codepoint:04X}: {line:?}");
            for (column, pixel) in line.bytes().enumerate() {
                match pixel {
                    b'#' => *row |= 0x80 >> column,
                    b'.' => {}
                    other => panic!("U+{codepoint:04X}: {other:?}"),
                }
            }
        }
        glyphs.push((codepoint, rows));
    }
    glyphs
}

fn rust_table(glyphs: &[(u32, GlyphRows)]) -> String {
    let mut source = String::new();
    for (_, rows) in glyphs {
        let rows: Vec<String> = rows.iter().map(|row| format!("0x{row:02x}")).collect();
        writeln!(source, "    [{}],", rows.join(", ")).unwrap();
    }
    source
}

#[test]
fn the_ascii_glyphs_are_the_design_file_s() {
    let glyphs = design_glyphs();
    let codepoints: Vec<u32> = glyphs.iter().map(|(codepoint, _)| *codepoint).collect();
    assert_eq!(codepoints, (0x20..=0x7e).collect::<Vec<_>>());
    let table: Vec<GlyphRows> = glyphs.iter().map(|(_, rows)| *rows).collect();
    assert!(
        table == egg_mono::ASCII,
        "egg_mono::ASCII differs from docs/ux/font-8x16.txt; its rows are:\n{}",
        rust_table(&glyphs)
    );
}

#[test]
fn the_design_s_metrics_hold_for_every_glyph() {
    for (codepoint, rows) in design_glyphs() {
        for (row, bits) in rows.iter().enumerate() {
            if codepoint != u32::from(b'_') {
                assert_eq!(bits & 1, 0, "U+{codepoint:04X} row {row}: column 7");
            }
        }
    }
}

fn picture(id: u8) -> Vec<String> {
    rows(id)
        .iter()
        .map(|row| {
            (0..GLYPH_WIDTH)
                .map(|column| {
                    if row & (0x80 >> column) != 0 {
                        '#'
                    } else {
                        '.'
                    }
                })
                .collect()
        })
        .collect()
}

#[test]
fn single_lines_cross_at_row_8_and_column_3_so_adjacent_cells_join() {
    let horizontal = picture(glyph_id('─' as u32));
    assert_eq!(horizontal[8], "########");
    assert!(
        horizontal
            .iter()
            .enumerate()
            .all(|(row, line)| row == 8 || line == "........")
    );
    let vertical = picture(glyph_id('│' as u32));
    assert!(vertical.iter().all(|line| line == "...#...."));
    let cross = picture(glyph_id('┼' as u32));
    assert_eq!(cross[8], "########");
    assert_eq!(cross[0], "...#....");
    assert_eq!(cross[15], "...#....");
    let corner = picture(glyph_id('┌' as u32));
    assert_eq!(corner[7], "........");
    assert_eq!(corner[8], "...#####");
    assert_eq!(corner[15], "...#....");
    let tee = picture(glyph_id('┤' as u32));
    assert_eq!(tee[8], "####....");
    assert_eq!(tee[0], "...#....");
}

#[test]
fn double_lines_use_rows_7_and_9_and_columns_2_and_4() {
    let horizontal = picture(glyph_id('═' as u32));
    assert_eq!(horizontal[7], "########");
    assert_eq!(horizontal[8], "........");
    assert_eq!(horizontal[9], "########");
    let vertical = picture(glyph_id('║' as u32));
    assert!(vertical.iter().all(|line| line == "..#.#..."));
    let top_left = picture(glyph_id('╔' as u32));
    assert_eq!(top_left[6], "........");
    assert_eq!(top_left[7], "..######");
    assert_eq!(top_left[8], "..#.....");
    assert_eq!(top_left[9], "..#.####");
    assert_eq!(top_left[15], "..#.#...");
    let bottom_right = picture(glyph_id('╝' as u32));
    assert_eq!(bottom_right[0], "..#.#...");
    assert_eq!(bottom_right[7], "###.#...");
    assert_eq!(bottom_right[8], "....#...");
    assert_eq!(bottom_right[9], "#####...");
    assert_eq!(bottom_right[10], "........");
}

#[test]
fn shades_are_one_two_and_three_in_four_checkerboards() {
    let lit = |character: char| -> u32 {
        rows(glyph_id(character as u32))
            .iter()
            .map(|row| row.count_ones())
            .sum()
    };
    assert_eq!(lit('░'), 32);
    assert_eq!(lit('▒'), 64);
    assert_eq!(lit('▓'), 96);
    assert_eq!(lit('█'), 128);
    assert_eq!(lit('▀'), 64);
    assert_eq!(lit('▄'), 64);
    assert_eq!(rows(glyph_id('▀' as u32))[7], 0xff);
    assert_eq!(rows(glyph_id('▄' as u32))[8], 0xff);
}

#[test]
fn ids_are_ascii_then_the_extras_in_the_design_s_order_else_the_question_mark() {
    assert_eq!(glyph_id('A' as u32), b'A');
    assert_eq!(glyph_id(' ' as u32), b' ');
    assert_eq!(glyph_id('~' as u32), b'~');
    assert_eq!(glyph_id('─' as u32), 0x80);
    assert_eq!(glyph_id('┼' as u32), 0x8a);
    assert_eq!(glyph_id('═' as u32), 0x8b);
    assert_eq!(glyph_id('█' as u32), 0x91);
    assert_eq!(glyph_id('▪' as u32), 0x97);
    assert_eq!(glyph_id('→' as u32), 0x9b);
    assert_eq!(glyph_id('↓' as u32), 0x9e);
    assert_eq!(glyph_id('▲' as u32), UP_TRIANGLE);
    for unknown in [0x7f, 0x9f, 0xe9, 0x2501, 0x1f600, 0x0a] {
        assert_eq!(glyph_id(unknown), REPLACEMENT, "U+{unknown:04X}");
    }
    assert_eq!(rows(0x7f), rows(b'?'));
    assert_eq!(rows(0xff), rows(b'?'));
    assert_eq!(rows(0x00), rows(b'?'));
    assert_ne!(rows(UP_TRIANGLE), rows(b'?'));
    for (index, &extra) in EXTRAS.iter().enumerate() {
        assert_eq!(glyph_id(extra as u32), FIRST_EXTRA + index as u8);
        assert!(rows(FIRST_EXTRA + index as u8).iter().any(|&row| row != 0));
    }
}

#[test]
fn a_glyph_is_drawn_into_its_cell_alone_with_its_underline() {
    let stride = 20;
    let mut pixels = vec![7u32; stride * 18];
    let at = stride + 2;
    draw_glyph(&mut pixels, stride, at, b'I', 0xff, 0x11, true);
    let rows_of_i = rows(b'I');
    for y in 0..18 {
        for x in 0..stride {
            let pixel = pixels[y * stride + x];
            let inside = (1..17).contains(&y) && (2..10).contains(&x);
            if !inside {
                assert_eq!(pixel, 7, "({x}, {y})");
                continue;
            }
            let (row, column) = (y - 1, x - 2);
            let ink = row == UNDERLINE_ROW || rows_of_i[row] & (0x80 >> column) != 0;
            assert_eq!(pixel, if ink { 0xff } else { 0x11 }, "({x}, {y})");
        }
    }
}

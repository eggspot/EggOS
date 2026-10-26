use super::*;
use crate::font::rows as glyph_rows;

fn terminal(columns: u16, rows: u16) -> Terminal {
    Terminal::new(columns, rows).unwrap()
}

fn fed(columns: u16, rows: u16, bytes: &[u8]) -> Terminal {
    let mut terminal = terminal(columns, rows);
    terminal.feed(bytes);
    terminal
}

fn text(cells: &[Cell]) -> String {
    cells.iter().map(|cell| cell.glyph as char).collect()
}

fn row_text(terminal: &Terminal, row: u16) -> String {
    text(terminal.line(row)).trim_end().to_string()
}

fn screen_text(terminal: &Terminal) -> Vec<String> {
    (0..terminal.size().1)
        .map(|row| row_text(terminal, row))
        .collect()
}

#[test]
fn printable_bytes_fill_cells_and_move_the_cursor() {
    let terminal = fed(10, 3, b"EggOS");
    assert_eq!(row_text(&terminal, 0), "EggOS");
    assert_eq!(terminal.cursor(), (0, 5));
    assert_eq!(
        terminal.line(0)[0],
        Cell {
            glyph: b'E',
            ..Cell::BLANK
        }
    );
}

#[test]
fn backspace_tab_carriage_return_and_line_feeds() {
    let mut terminal = fed(20, 4, b"ab\x08\x08\x08c");
    assert_eq!(row_text(&terminal, 0), "cb");
    assert_eq!(terminal.cursor(), (0, 1));
    terminal.feed(b"\tx\ty");
    assert_eq!(row_text(&terminal, 0), "cb      x       y");
    terminal.feed(b"\t\t\tz");
    assert_eq!(terminal.cursor(), (0, 19));
    assert_eq!(terminal.line(0)[19].glyph, b'z');
    terminal.feed(b"\r");
    assert_eq!(terminal.cursor(), (0, 0));
    terminal.feed(b"\n\x0b\x0c");
    assert_eq!(terminal.cursor(), (3, 0));
    terminal.feed(b"q\n");
    assert_eq!(terminal.cursor(), (3, 1));
    terminal.set_onlcr(true);
    terminal.feed(b"\n");
    assert_eq!(terminal.cursor(), (3, 0));
    terminal.feed(b"\x07\x00\x7f");
    assert_eq!(terminal.cursor(), (3, 0));
}

#[test]
fn autowrap_waits_for_the_next_glyph_so_a_full_line_adds_no_empty_one() {
    let mut terminal = fed(4, 3, b"abcd");
    assert_eq!(terminal.cursor(), (0, 3));
    terminal.feed(b"\r\n");
    assert_eq!(terminal.cursor(), (1, 0));
    terminal.feed(b"wxyz12");
    assert_eq!(screen_text(&terminal), ["abcd", "wxyz", "12"]);
    terminal.feed(b"\x1b[?7l\x1b[3;1Habcdef");
    assert_eq!(row_text(&terminal, 2), "abcf");
    assert_eq!(terminal.cursor(), (2, 3));
    terminal.feed(b"\x1b[?7h\x1b[1;1Habcd\x08e");
    assert_eq!(row_text(&terminal, 0), "abed");
}

#[test]
fn line_feed_at_the_bottom_scrolls_into_the_scrollback() {
    let mut terminal = fed(5, 2, b"one\r\ntwo\r\nthree\r\nfour");
    assert_eq!(screen_text(&terminal), ["three", "four"]);
    assert_eq!(terminal.history_length(), 2);
    assert_eq!(text(terminal.history_line(0)).trim_end(), "one");
    assert_eq!(text(terminal.history_line(1)).trim_end(), "two");
    terminal.feed(b"\x1b[3J");
    assert_eq!(terminal.history_length(), 0);
    assert_eq!(screen_text(&terminal), ["", ""]);
}

#[test]
fn the_scrollback_ring_keeps_the_newest_lines() {
    let mut terminal = terminal(6, 1);
    for line in 0..HISTORY_LINES + 10 {
        terminal.feed(format!("\r\n{line}").as_bytes());
    }
    assert_eq!(terminal.history_length(), HISTORY_LINES);
    assert_eq!(text(terminal.history_line(0)).trim_end(), "9");
    assert_eq!(
        text(terminal.history_line(HISTORY_LINES - 1)).trim_end(),
        (HISTORY_LINES + 8).to_string()
    );
    assert_eq!(row_text(&terminal, 0), (HISTORY_LINES + 9).to_string());
}

#[test]
fn save_and_restore_cursor_keep_position_attributes_and_pending_wrap() {
    let mut terminal = fed(4, 3, b"\x1b[2;3H\x1b[31;1m\x1b7\x1b[H\x1b[0mx\x1b8y");
    assert_eq!(terminal.line(1)[2].glyph, b'y');
    assert_eq!(terminal.line(1)[2].foreground, 1);
    assert_eq!(terminal.line(1)[2].flags, cell_flags::BOLD);
    assert_eq!(terminal.line(0)[0].foreground, DEFAULT_COLOUR);
    terminal.feed(b"\x1b[1;1Habcd\x1b7\x1b[3;1H\x1b8e");
    assert_eq!(row_text(&terminal, 1), "e y");
    terminal.feed(b"\x1b[3;2H\x1b[s\x1b[1;1H\x1b[uZ");
    assert_eq!(terminal.line(2)[1].glyph, b'Z');
}

#[test]
fn index_reverse_index_and_next_line() {
    let mut terminal = fed(3, 3, b"a\x1bDb\x1bEc");
    assert_eq!(screen_text(&terminal), ["a", " b", "c"]);
    terminal.feed(b"\x1bD\x1bD");
    assert_eq!(screen_text(&terminal), ["c", "", ""]);
    terminal.feed(b"\x1b[1;1H\x1bM");
    assert_eq!(screen_text(&terminal), ["", "c", ""]);
    assert_eq!(terminal.cursor(), (0, 0));
}

#[test]
fn reset_clears_the_screen_modes_and_attributes_but_not_the_scrollback() {
    let mut terminal = fed(
        4,
        2,
        b"a\r\nb\r\nc\x1b[2;2r\x1b[?25l\x1b[?1h\x1b[7m\x1b[?1049h",
    );
    terminal.feed(b"\x1bc");
    assert!(!terminal.on_alternate_screen());
    assert_eq!(screen_text(&terminal), ["", ""]);
    assert_eq!(terminal.cursor(), (0, 0));
    assert!(terminal.cursor_visible());
    assert!(!terminal.application_cursor_keys());
    terminal.feed(b"x\r\n\r\n");
    assert_eq!(terminal.line(0)[0].flags, 0);
    assert_eq!(terminal.history_length(), 2);
}

#[test]
fn charset_and_keypad_selections_are_accepted_without_effect() {
    let terminal = fed(10, 1, b"\x1b(B\x1b)0\x1b=\x1b>\x1b#8\x1b Fok");
    assert_eq!(row_text(&terminal, 0), "ok");
}

#[test]
fn cursor_movement_is_clamped_to_the_grid_and_the_scroll_region() {
    let mut terminal = fed(10, 10, b"\x1b[5;5H");
    terminal.feed(b"\x1b[2A");
    assert_eq!(terminal.cursor(), (2, 4));
    terminal.feed(b"\x1b[B");
    assert_eq!(terminal.cursor(), (3, 4));
    terminal.feed(b"\x1b[3C");
    assert_eq!(terminal.cursor(), (3, 7));
    terminal.feed(b"\x1b[99C");
    assert_eq!(terminal.cursor(), (3, 9));
    terminal.feed(b"\x1b[0D");
    assert_eq!(terminal.cursor(), (3, 8));
    terminal.feed(b"\x1b[99D\x1b[99A");
    assert_eq!(terminal.cursor(), (0, 0));
    terminal.feed(b"\x1b[3;6r\x1b[4;1H\x1b[9A");
    assert_eq!(terminal.cursor(), (2, 0));
    terminal.feed(b"\x1b[9B");
    assert_eq!(terminal.cursor(), (5, 0));
    terminal.feed(b"\x1b[8;1H\x1b[9B");
    assert_eq!(terminal.cursor(), (9, 0));
    terminal.feed(b"\x1b[1;1H\x1b[2E");
    assert_eq!(terminal.cursor(), (2, 0));
    terminal.feed(b"\x1b[4;4H\x1b[F");
    assert_eq!(terminal.cursor(), (2, 0));
    terminal.feed(b"\x1b[7G");
    assert_eq!(terminal.cursor(), (2, 6));
    terminal.feed(b"\x1b[8d");
    assert_eq!(terminal.cursor(), (7, 6));
    terminal.feed(b"\x1b[99;99f");
    assert_eq!(terminal.cursor(), (9, 9));
    terminal.feed(b"\x1b[H");
    assert_eq!(terminal.cursor(), (0, 0));
}

#[test]
fn origin_mode_positions_the_cursor_inside_the_scroll_region() {
    let mut terminal = fed(10, 10, b"\x1b[3;6r\x1b[?6h");
    assert_eq!(terminal.cursor(), (2, 0));
    terminal.feed(b"\x1b[2;3H");
    assert_eq!(terminal.cursor(), (3, 2));
    terminal.feed(b"\x1b[9;1H");
    assert_eq!(terminal.cursor(), (5, 0));
    terminal.feed(b"\x1b[6n");
    assert_eq!(terminal.replies(), b"\x1b[4;1R");
    terminal.feed(b"\x1b[?6l");
    assert_eq!(terminal.cursor(), (0, 0));
}

#[test]
fn erase_in_display_and_line_take_the_current_background() {
    let filled = || fed(4, 3, b"abcd\r\nefgh\r\nijkl\x1b[2;2H\x1b[44m");
    let mut terminal = filled();
    terminal.feed(b"\x1b[J");
    assert_eq!(screen_text(&terminal), ["abcd", "e", ""]);
    assert_eq!(terminal.line(1)[1].background, 4);
    assert_eq!(terminal.line(2)[3].background, 4);
    assert_eq!(terminal.line(1)[0].background, DEFAULT_COLOUR);
    let mut terminal = filled();
    terminal.feed(b"\x1b[1J");
    assert_eq!(screen_text(&terminal), ["", "  gh", "ijkl"]);
    let mut terminal = filled();
    terminal.feed(b"\x1b[2J");
    assert_eq!(screen_text(&terminal), ["", "", ""]);
    assert_eq!(terminal.cursor(), (1, 1));
    let mut terminal = filled();
    terminal.feed(b"\x1b[K");
    assert_eq!(screen_text(&terminal), ["abcd", "e", "ijkl"]);
    let mut terminal = filled();
    terminal.feed(b"\x1b[1K");
    assert_eq!(screen_text(&terminal), ["abcd", "  gh", "ijkl"]);
    let mut terminal = filled();
    terminal.feed(b"\x1b[2K");
    assert_eq!(screen_text(&terminal), ["abcd", "", "ijkl"]);
    assert_eq!(terminal.line(1)[3].background, 4);
}

#[test]
fn insert_and_delete_lines_stay_inside_the_scroll_region() {
    let lines = b"1\r\n2\r\n3\r\n4\r\n5";
    let mut terminal = fed(3, 5, lines);
    terminal.feed(b"\x1b[2;4r\x1b[3;2H\x1b[L");
    assert_eq!(screen_text(&terminal), ["1", "2", "", "3", "5"]);
    assert_eq!(terminal.cursor(), (2, 0));
    terminal.feed(b"\x1b[2M");
    assert_eq!(screen_text(&terminal), ["1", "2", "", "", "5"]);
    let mut terminal = fed(3, 5, lines);
    terminal.feed(b"\x1b[2;4r\x1b[2;1H\x1b[M");
    assert_eq!(screen_text(&terminal), ["1", "3", "4", "", "5"]);
    terminal.feed(b"\x1b[5;1H\x1b[L");
    assert_eq!(screen_text(&terminal), ["1", "3", "4", "", "5"]);
    assert_eq!(terminal.history_length(), 0);
}

#[test]
fn insert_delete_and_erase_characters_in_the_cursor_s_line() {
    let mut terminal = fed(6, 1, b"abcdef\x1b[1;2H\x1b[2@");
    assert_eq!(row_text(&terminal, 0), "a  bcd");
    terminal.feed(b"\x1b[3P");
    assert_eq!(row_text(&terminal, 0), "acd");
    terminal.feed(b"\x1b[1;1Hxyzuvw\x1b[1;3H\x1b[2X");
    assert_eq!(row_text(&terminal, 0), "xy  vw");
    terminal.feed(b"\x1b[99X");
    assert_eq!(row_text(&terminal, 0), "xy");
    terminal.feed(b"\x1b[1;1H\x1b[99@");
    assert_eq!(row_text(&terminal, 0), "");
}

#[test]
fn scroll_up_and_down_move_the_region() {
    let mut terminal = fed(3, 4, b"1\r\n2\r\n3\r\n4");
    terminal.feed(b"\x1b[2;3r\x1b[S");
    assert_eq!(screen_text(&terminal), ["1", "3", "", "4"]);
    terminal.feed(b"\x1b[2T");
    assert_eq!(screen_text(&terminal), ["1", "", "", "4"]);
    terminal.feed(b"\x1b[r\x1b[S");
    assert_eq!(screen_text(&terminal), ["", "", "4", ""]);
    assert_eq!(terminal.history_length(), 1);
    terminal.feed(b"\x1b[1;2;3;4;5T");
    assert_eq!(screen_text(&terminal), ["", "", "4", ""]);
}

#[test]
fn a_scroll_region_sends_the_cursor_home_and_bad_ones_are_ignored() {
    let mut terminal = fed(5, 5, b"\x1b[3;3H\x1b[2;4r");
    assert_eq!(terminal.cursor(), (0, 0));
    terminal.feed(b"\x1b[4;4H\x1b[4;2r");
    assert_eq!(terminal.cursor(), (3, 3));
    terminal.feed(b"\x1b[2;9r");
    assert_eq!(terminal.cursor(), (3, 3));
    terminal.feed(b"\x1b[2;4r\x1b[4;1H\n\n");
    assert_eq!(terminal.cursor(), (3, 0));
}

#[test]
fn device_status_and_attributes_reply_into_the_input() {
    let mut terminal = fed(80, 24, b"\x1b[12;40H\x1b[6n");
    assert_eq!(terminal.replies(), b"\x1b[12;40R");
    terminal.clear_replies();
    terminal.feed(b"\x1b[c\x1b[>c\x1b[0c\x1b[5n");
    assert_eq!(terminal.replies(), b"\x1b[?1;2c\x1b[>0;10;0c\x1b[?1;2c");
    terminal.clear_replies();
    for _ in 0..REPLY_CAPACITY {
        terminal.feed(b"\x1b[6n");
    }
    assert!(terminal.replies().len() <= REPLY_CAPACITY);
    assert!(terminal.replies().ends_with(b"R"));
}

#[test]
fn private_modes_switch_the_cursor_keys_and_the_cursor() {
    let mut terminal = fed(4, 2, b"\x1b[?1h\x1b[?25l\x1b[?12l");
    assert!(terminal.application_cursor_keys());
    assert!(!terminal.cursor_visible());
    assert!(!terminal.cursor_blinking());
    terminal.feed(b"\x1b[?1;25;12h");
    assert!(terminal.application_cursor_keys());
    assert!(terminal.cursor_visible());
    assert!(terminal.cursor_blinking());
    terminal.feed(b"\x1b[?1l\x1b[?2004h\x1b[?1000h\x1b[?1006l\x1b[4hok");
    assert!(!terminal.application_cursor_keys());
    assert_eq!(row_text(&terminal, 0), "ok");
}

#[test]
fn the_alternate_screen_starts_cleared_and_gives_the_main_one_back() {
    let mut terminal = fed(5, 2, b"main\x1b[1;3H\x1b[?1049h");
    assert!(terminal.on_alternate_screen());
    assert_eq!(screen_text(&terminal), ["", ""]);
    terminal.feed(b"\x1b[2;1Halt\n\n\n");
    assert_eq!(terminal.history_length(), 0);
    terminal.feed(b"\x1b[?1049l");
    assert!(!terminal.on_alternate_screen());
    assert_eq!(screen_text(&terminal), ["main", ""]);
    assert_eq!(terminal.cursor(), (0, 2));
    terminal.feed(b"\x1b[?47hx\x1b[?47l\x1b[?1047h");
    assert_eq!(screen_text(&terminal), ["", ""]);
    terminal.feed(b"\x1b[?1047l");
    assert_eq!(screen_text(&terminal), ["main", ""]);
}

#[test]
fn sgr_sets_the_sixteen_colours_bold_underline_and_inverse() {
    for colour in 0..8u8 {
        let terminal = fed(
            4,
            1,
            format!(
                "\x1b[{};{}mx\x1b[{};{}my",
                30 + colour,
                40 + colour,
                90 + colour,
                100 + colour
            )
            .as_bytes(),
        );
        let (normal, bright) = (terminal.line(0)[0], terminal.line(0)[1]);
        assert_eq!((normal.foreground, normal.background), (colour, colour));
        assert_eq!(
            (bright.foreground, bright.background),
            (colour + 8, colour + 8)
        );
        assert_eq!(
            normal.colours(),
            (
                ANSI_COLOURS[usize::from(colour)],
                ANSI_COLOURS[usize::from(colour)]
            )
        );
    }
    let terminal = fed(
        8,
        1,
        b"\x1b[1;4;7;31ma\x1b[22mb\x1b[24;27mc\x1b[39;49md\x1b[2me\x1b[mf",
    );
    let cells = terminal.line(0);
    assert_eq!(
        cells[0].flags,
        cell_flags::BOLD | cell_flags::UNDERLINE | cell_flags::INVERSE
    );
    assert_eq!(cells[0].colours(), (TERM_BACKGROUND, ANSI_COLOURS[9]));
    assert_eq!(cells[1].flags, cell_flags::UNDERLINE | cell_flags::INVERSE);
    assert_eq!(cells[2].flags, 0);
    assert_eq!(cells[2].foreground, 1);
    assert_eq!(
        (cells[3].foreground, cells[3].background),
        (DEFAULT_COLOUR, DEFAULT_COLOUR)
    );
    assert_eq!(cells[4].flags, 0);
    assert_eq!(
        cells[5],
        Cell {
            glyph: b'f',
            ..Cell::BLANK
        }
    );
    let bold_default = Cell {
        flags: cell_flags::BOLD,
        ..Cell::BLANK
    };
    assert_eq!(bold_default.colours(), (TERM_FOREGROUND, TERM_BACKGROUND));
    let bold_bright = Cell {
        foreground: 12,
        flags: cell_flags::BOLD,
        ..Cell::BLANK
    };
    assert_eq!(bold_bright.colours().0, ANSI_COLOURS[12]);
}

#[test]
fn extended_colours_map_to_the_nearest_of_the_sixteen() {
    let colour = |sequence: &str| {
        let cell = fed(2, 1, format!("\x1b[{sequence}mx").as_bytes()).line(0)[0];
        (cell.foreground, cell.background)
    };
    assert_eq!(colour("38;5;3"), (3, DEFAULT_COLOUR));
    assert_eq!(colour("48;5;12"), (DEFAULT_COLOUR, 12));
    assert_eq!(colour("38;5;16"), (0, DEFAULT_COLOUR));
    assert_eq!(colour("38;5;52"), (1, DEFAULT_COLOUR));
    assert_eq!(colour("38;5;196"), (9, DEFAULT_COLOUR));
    assert_eq!(colour("38;5;21"), (12, DEFAULT_COLOUR));
    assert_eq!(colour("38;5;231"), (15, DEFAULT_COLOUR));
    assert_eq!(colour("38;5;232"), (0, DEFAULT_COLOUR));
    assert_eq!(colour("38;5;240"), (8, DEFAULT_COLOUR));
    assert_eq!(colour("38;5;245"), (7, DEFAULT_COLOUR));
    assert_eq!(colour("38;5;255"), (15, DEFAULT_COLOUR));
    assert_eq!(colour("38;2;255;0;0"), (9, DEFAULT_COLOUR));
    assert_eq!(colour("48;2;0;100;0"), (DEFAULT_COLOUR, 2));
    assert_eq!(colour("38;2;10;10;10"), (0, DEFAULT_COLOUR));
    assert_eq!(colour("38;2;200;200;200;44"), (15, 4));
    assert_eq!(colour("38;7;31"), (1, DEFAULT_COLOUR));
}

#[test]
fn unknown_and_overlong_sequences_are_swallowed_whole() {
    let terminal = fed(
        20,
        1,
        b"a\x1b[?99h\x1b[1;2;3;4;5;6;7;8;9;10;11;12;13;14;15;16;17mb\x1b[1:2mc\x1b[>5;6z\x1b[ qd\x1bZe",
    );
    assert_eq!(row_text(&terminal, 0), "abcde");
    assert_eq!(terminal.line(0)[1].flags, 0);
    let terminal = fed(
        20,
        1,
        b"\x1b]0;a title\x07x\x1b]2;t\x1b\\y\x1bP1$r\x1b\\z\x1b_apc\x1b\\w",
    );
    assert_eq!(row_text(&terminal, 0), "xyzw");
    let mut long_title = b"\x1b]0;".to_vec();
    long_title.extend(core::iter::repeat_n(b'.', 2000));
    let terminal = fed(4, 1, &long_title);
    assert!(row_text(&terminal, 0).bytes().all(|byte| byte == b'.'));
    let terminal = fed(10, 1, b"\x1b[31\x18x\x1b]0;t\x1a y\x1b[1\x1b[32mz");
    assert_eq!(row_text(&terminal, 0), "x yz");
    assert_eq!(terminal.line(0)[3].foreground, 2);
    let terminal = fed(10, 1, b"\x1b[2\nC");
    assert_eq!(terminal.cursor(), (0, 2));
    assert_eq!(row_text(&terminal, 0), "");
}

#[test]
fn utf8_decodes_to_glyph_ids_and_bad_bytes_draw_one_question_mark_each() {
    let terminal = fed(20, 1, "┌─┐│é→".as_bytes());
    let ids: Vec<u8> = terminal.line(0)[..6]
        .iter()
        .map(|cell| cell.glyph)
        .collect();
    assert_eq!(
        ids,
        [
            font::glyph_id('┌' as u32),
            font::glyph_id('─' as u32),
            font::glyph_id('┐' as u32),
            font::glyph_id('│' as u32),
            b'?',
            font::glyph_id('→' as u32)
        ]
    );
    let terminal = fed(
        20,
        1,
        b"a\x80b\xc0\xafc\xe2\x94d\xff\xf0\x9f\x98\x80e\xed\xa0\x80f",
    );
    assert_eq!(row_text(&terminal, 0), "a?b??c??d??e???f");
    let terminal = fed(20, 1, b"\xe2\x94\x1b[1Cx");
    assert_eq!(row_text(&terminal, 0), "?? x");
}

fn pixels_of(terminal: &mut Terminal, width: usize, height: usize) -> Vec<u32> {
    let mut pixels = vec![0x00ab_cdef; width * height];
    let mut damage = Damage::new();
    terminal.render(&mut pixels, width, PADDING, PADDING, &mut damage);
    pixels
}

#[test]
fn eggos_renders_as_the_font_s_glyphs_in_the_terminal_s_colours() {
    let mut terminal = fed(8, 2, b"EggOS");
    let width = 8 * 8 + 16;
    let pixels = pixels_of(&mut terminal, width, 2 * 16 + 16);
    for (index, &glyph) in b"EggOS".iter().enumerate() {
        for (row, bits) in glyph_rows(glyph).iter().enumerate() {
            for column in 0..8 {
                let x = 8 + index * 8 + column;
                let y = 8 + row;
                let ink = bits & (0x80 >> column) != 0;
                let expected = if ink {
                    TERM_FOREGROUND
                } else {
                    TERM_BACKGROUND
                };
                assert_eq!(
                    pixels[y * width + x],
                    expected,
                    "{} ({x}, {y})",
                    glyph as char
                );
            }
        }
    }
    let cursor_pixel = pixels[8 * width + 8 + 5 * 8];
    assert_eq!(cursor_pixel, TERM_CURSOR);
    assert_eq!(pixels[0], 0x00ab_cdef);
    assert_eq!(pixels[(8 + 32) * width + 4], 0x00ab_cdef);
}

#[test]
fn the_cursor_is_a_block_with_the_glyph_in_the_background_colour() {
    let mut terminal = fed(4, 1, b"ab\x1b[1;1H");
    let width = 4 * 8 + 16;
    let pixels = pixels_of(&mut terminal, width, 32);
    for (row, bits) in glyph_rows(b'a').iter().enumerate() {
        for column in 0..8 {
            let ink = bits & (0x80 >> column) != 0;
            let expected = if ink { TERM_BACKGROUND } else { TERM_CURSOR };
            assert_eq!(pixels[(8 + row) * width + 8 + column], expected);
        }
    }
    let mut hidden = fed(4, 1, b"ab\x1b[1;1H\x1b[?25l");
    let pixels = pixels_of(&mut hidden, width, 32);
    assert_eq!(pixels[8 * width + 8], TERM_BACKGROUND);
}

#[test]
fn a_render_draws_only_the_damaged_cells_and_reports_them() {
    let mut terminal = terminal(10, 4);
    let (width, height) = (10 * 8 + 16, 4 * 16 + 16);
    let mut pixels = vec![0; width * height];
    let mut damage = Damage::new();
    terminal.render(&mut pixels, width, 8, 8, &mut damage);
    assert_eq!(damage.rects(), &[Rect::new(8, 8, 80, 64)]);
    damage.clear();
    terminal.render(&mut pixels, width, 8, 8, &mut damage);
    assert_eq!(damage.rects(), &[Rect::new(8, 8, 8, 16)]);
    assert!(!terminal.needs_render());
    damage.clear();
    terminal.render(&mut pixels, width, 8, 8, &mut damage);
    assert_eq!(damage.rects(), &[Rect::new(8, 8, 8, 16)]);
    terminal.feed(b"\x1b[?25l\x1b[3;4Hxy");
    damage.clear();
    terminal.render(&mut pixels, width, 8, 8, &mut damage);
    assert_eq!(damage.rects().len(), 2);
    assert!(damage.rects().contains(&Rect::new(8, 8, 8, 16)));
    assert!(damage.rects().contains(&Rect::new(8 + 24, 8 + 32, 16, 16)));
}

#[test]
fn a_grid_assumed_drawn_blank_draws_only_the_cursor_and_what_changed() {
    let mut terminal = terminal(10, 4);
    terminal.assume_drawn_blank();
    let (width, height) = (10 * 8 + 16, 4 * 16 + 16);
    let mut pixels = vec![TERM_BACKGROUND; width * height];
    let mut damage = Damage::new();
    terminal.feed(b"\x1b[2;3Hab");
    terminal.render(&mut pixels, width, 8, 8, &mut damage);
    assert_eq!(damage.rects(), &[Rect::new(8 + 16, 8 + 16, 24, 16)]);
    let mut fresh = fed(10, 4, b"\x1b[2;3Hab");
    let mut expected = vec![TERM_BACKGROUND; width * height];
    fresh.render(&mut expected, width, 8, 8, &mut Damage::new());
    assert!(pixels == expected);
}

#[test]
fn a_grid_that_does_not_fit_draws_nothing() {
    let mut terminal = terminal(10, 4);
    let mut pixels = vec![1u32; 50 * 50];
    let mut damage = Damage::new();
    terminal.render(&mut pixels, 50, 8, 8, &mut damage);
    assert!(damage.is_empty());
    assert!(pixels.iter().all(|&pixel| pixel == 1));
}

#[test]
fn a_scroll_moves_pixels_and_draws_only_the_new_line() {
    let mut terminal = fed(10, 4, b"1\r\n2\r\n3\r\n4");
    let (width, height) = (10 * 8 + 16, 4 * 16 + 16);
    let mut pixels = vec![0; width * height];
    let mut damage = Damage::new();
    terminal.render(&mut pixels, width, 8, 8, &mut damage);
    terminal.feed(b"\r\n5");
    damage.clear();
    terminal.render(&mut pixels, width, 8, 8, &mut damage);
    assert_eq!(damage.rects(), &[Rect::new(8, 8, 80, 64)]);
    let mut fresh = fed(10, 4, b"1\r\n2\r\n3\r\n4\r\n5");
    assert_eq!(pixels, {
        let mut expected = vec![0; width * height];
        fresh.render(&mut expected, width, 8, 8, &mut Damage::new());
        expected
    });
}

#[test]
fn the_scrollback_view_shows_older_lines_with_its_mark_and_no_cursor() {
    let mut terminal = fed(4, 2, b"a\r\nb\r\nc\r\nd");
    terminal.move_view(ViewMove::Back(1));
    assert_eq!(terminal.view_back(), 1);
    let width = 4 * 8 + 16;
    let pixels = pixels_of(&mut terminal, width, 2 * 16 + 16);
    let mut expected_rows = glyph_rows(b'b').to_vec();
    expected_rows.extend(glyph_rows(b'c'));
    for (row, bits) in expected_rows.iter().enumerate() {
        let ink = bits & 0x80 != 0;
        let pixel = pixels[(8 + row) * width + 8];
        assert_eq!(
            pixel,
            if ink {
                TERM_FOREGROUND
            } else {
                TERM_BACKGROUND
            }
        );
    }
    let mark = glyph_rows(font::UP_TRIANGLE);
    let (mark_row, _) = mark
        .iter()
        .enumerate()
        .find(|(_, bits)| **bits & 0x80 != 0)
        .unwrap();
    assert_eq!(pixels[(8 + mark_row) * width + 8 + 3 * 8], ACCENT);
    assert_eq!(pixels[(8 + 16) * width + 8 + 8], TERM_BACKGROUND);
    terminal.move_view(ViewMove::PageBack);
    assert_eq!(terminal.view_back(), 2);
    terminal.move_view(ViewMove::Oldest);
    assert_eq!(terminal.view_back(), 2);
    terminal.move_view(ViewMove::Forward(1));
    assert_eq!(terminal.view_back(), 1);
    terminal.feed(b"\r\ne");
    assert_eq!(terminal.view_back(), 2);
    terminal.move_view(ViewMove::PageForward);
    assert_eq!(terminal.view_back(), 1);
    terminal.move_view(ViewMove::Live);
    assert_eq!(terminal.view_back(), 0);
    terminal.feed(b"\x1b[?1049h");
    terminal.move_view(ViewMove::Back(1));
    assert_eq!(terminal.view_back(), 0);
}

#[test]
fn the_full_screen_grid_follows_the_display_s_size() {
    assert_eq!(
        TextArea::full_screen(1280, 800),
        Some(TextArea {
            x: 8,
            y: 8,
            columns: 158,
            rows: 49
        })
    );
    assert_eq!(
        TextArea::full_screen(1024, 768),
        Some(TextArea {
            x: 8,
            y: 8,
            columns: 126,
            rows: 47
        })
    );
    assert_eq!(TextArea::full_screen(23, 31), None);
    assert_eq!(
        TextArea::full_screen(24, 32).map(|area| (area.columns, area.rows)),
        Some((1, 1))
    );
    assert_eq!(
        TextArea::full_screen(1280, 800).unwrap().rect(),
        Rect::new(8, 8, 1264, 784)
    );
    assert!(Terminal::new(0, 5).is_none());
}

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn below(&mut self, limit: usize) -> usize {
        (self.next() % limit as u64) as usize
    }
}

const PIECES: [&[u8]; 34] = [
    b"hello",
    b"x",
    b"\r\n",
    b"\n",
    b"\r",
    b"\x08",
    b"\t",
    b"\x1b[31m",
    b"\x1b[1;44m",
    b"\x1b[7m",
    b"\x1b[0m",
    b"\x1b[4m",
    b"\x1b[H",
    b"\x1b[3;5H",
    b"\x1b[2J",
    b"\x1b[K",
    b"\x1b[1J",
    b"\x1b[2;5r",
    b"\x1b[r",
    b"\x1b[3L",
    b"\x1b[2M",
    b"\x1b[2S",
    b"\x1b[T",
    b"\x1bM",
    b"\x1bD",
    b"\x1b[3@",
    b"\x1b[2P",
    b"\x1b[?25l",
    b"\x1b[?25h",
    b"\x1b[?1049h",
    b"\x1b[?1049l",
    "─│é".as_bytes(),
    b"\x1b[5A\x1b[9C",
    b"0123456789abcdefghijklmnopqrstuvwxyz",
];

fn assert_damage_covers_changes(before: &[u32], after: &[u32], width: usize, damage: &Damage) {
    for (index, (old, new)) in before.iter().zip(after).enumerate() {
        if old == new {
            continue;
        }
        let (x, y) = ((index % width) as u32, (index / width) as u32);
        let covered = damage.rects().iter().any(|rect| {
            (rect.x..rect.right()).contains(&x) && (rect.y..rect.bottom()).contains(&y)
        });
        assert!(covered, "pixel ({x}, {y}) changed outside the damage");
    }
}

#[test]
fn rendering_after_any_mix_of_sequences_matches_a_fresh_render() {
    for seed in 0..40 {
        let mut random = Lcg(seed);
        let (columns, rows) = (12, 7);
        let (width, height) = (usize::from(columns) * 8 + 16, usize::from(rows) * 16 + 16);
        let mut terminal = terminal(columns, rows);
        let mut fed_bytes = Vec::new();
        let mut pixels = vec![TERM_BACKGROUND; width * height];
        terminal.render(&mut pixels, width, 8, 8, &mut Damage::new());
        for step in 0..60 {
            let piece = PIECES[random.below(PIECES.len())];
            terminal.feed(piece);
            fed_bytes.extend_from_slice(piece);
            if random.below(3) == 0 || step == 59 {
                if random.below(4) == 0 {
                    terminal.move_view(if random.below(2) == 0 {
                        ViewMove::Back(random.below(4))
                    } else {
                        ViewMove::Live
                    });
                }
                let before = pixels.clone();
                let mut damage = Damage::new();
                terminal.render(&mut pixels, width, 8, 8, &mut damage);
                assert_damage_covers_changes(&before, &pixels, width, &damage);
            }
        }
        terminal.move_view(ViewMove::Live);
        terminal.render(&mut pixels, width, 8, 8, &mut Damage::new());
        let mut fresh = fed(columns, rows, &fed_bytes);
        let mut expected = vec![TERM_BACKGROUND; width * height];
        fresh.render(&mut expected, width, 8, 8, &mut Damage::new());
        assert!(
            pixels == expected,
            "seed {seed}: {:?}",
            String::from_utf8_lossy(&fed_bytes)
        );
    }
}

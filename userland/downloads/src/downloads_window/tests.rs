use super::*;
use eggos_download_list::{EPERM, ESRCH, TERMINATE};
use eggos_toolkit::geometry::Rect;
use eggos_toolkit::input::{Modifiers, MouseButton};
use eggos_toolkit::tree::element_of;
use eggos_toolkit::widget::{TABLE_CELL_PADDING, TABLE_FIRST_TEXT_X, TABLE_MARK_X};
use std::collections::BTreeMap;

const NOW: u32 = 5000;
const EGGOS_NAME: &str = "eggos-0.3.0-x86_64.elf";
const EGGOS_PATH: &str = "/home/root/Downloads/eggos-0.3.0-x86_64.elf";
const EGGOS_RECEIVED: u64 = 3_565_159;
const PROCESSES: &str = "1 0 1 1 R 10 init\n61 7 61 7 R 3 fetch\n";
const ADDRESS: &str = "https://example.com/eggos.elf";
const SAVING_EGGOS: &str = "Saving to /home/root/Downloads/eggos-0.3.0-x86_64.elf";
const SAVED_NOTES: &str = "Saved to /home/root/Downloads/notes.txt";
const FAILED_ZIP: &str = "Failed: certificate not trusted";
const CANCELLED_LINE: &str = "Cancelled: the partial file was removed.";
const ENDED_LINE: &str = "Failed: fetch ended without finishing";
const CANCELLING_EGGOS: &str = "Cancelling eggos-0.3.0-x86_64.elf...";
const ALREADY_ENDED: &str = "eggos-0.3.0-x86_64.elf has already ended.";
const ROWS: [&str; 4] = [
    "eggos-0.3.0-x86_64.elf\t3.4 MB of 12 MB\tDownloading, 28 %\texample.com",
    "notes.txt\t512 B\tDone\t10.0.2.2:8000",
    "a.zip\t0 B\tFailed: certificate not trusted\tbad.example.net",
    "big.iso\t1.0 MB of 700 MB\tCancelled\texample.org",
];

fn file(record: Record<'_>) -> Vec<u8> {
    let mut bytes = vec![0u8; MAX_RECORD_BYTES];
    let length = record.write(&mut bytes).expect("a record fits");
    bytes.truncate(length);
    bytes
}

fn eggos(received: u64, state: State) -> Vec<u8> {
    file(Record {
        url: b"https://example.com/eggos-0.3.0-x86_64.elf",
        path: EGGOS_PATH.as_bytes(),
        received,
        total: Some(12_582_912),
        state,
        reason: b"",
        started: 1_793_282_650,
        ended: state.is_over().then_some(1_793_282_660),
    })
}

fn running(path: &str, started: u64) -> Vec<u8> {
    file(Record {
        url: b"https://example.com/new.bin",
        path: path.as_bytes(),
        received: 0,
        total: None,
        state: State::Downloading,
        reason: b"",
        started,
        ended: None,
    })
}

fn over_records() -> [(u32, Vec<u8>); 3] {
    let notes = Record {
        url: b"http://10.0.2.2:8000/notes.txt",
        path: b"/home/root/Downloads/notes.txt",
        received: 512,
        total: Some(512),
        state: State::Done,
        reason: b"",
        started: 1_793_282_600,
        ended: Some(1_793_282_601),
    };
    let zip = Record {
        url: b"https://bad.example.net/a.zip",
        path: b"/home/root/Downloads/a.zip",
        received: 0,
        total: None,
        state: State::Failed,
        reason: b"certificate not trusted",
        started: 1_793_282_590,
        ended: Some(1_793_282_591),
    };
    let iso = Record {
        url: b"http://example.org/big.iso",
        path: b"/home/root/Downloads/big.iso",
        received: 1_048_576,
        total: Some(734_003_200),
        state: State::Cancelled,
        reason: b"",
        started: 1_793_282_500,
        ended: Some(1_793_282_530),
    };
    [(58, file(notes)), (55, file(zip)), (50, file(iso))]
}

#[derive(Debug)]
struct FakeSystem {
    records: BTreeMap<u32, Vec<u8>>,
    list_error: Option<Errno>,
    processes: String,
    kills: Vec<(u32, u32)>,
    kill_answer: Result<(), i32>,
    removed: Vec<u32>,
    existing: Vec<String>,
    folders: Vec<String>,
    fetches: Vec<(String, String)>,
}

impl FakeSystem {
    fn example() -> Self {
        let mut records = BTreeMap::from(over_records());
        let downloading = eggos(EGGOS_RECEIVED, State::Downloading);
        records.insert(61, downloading);
        Self {
            records,
            list_error: None,
            processes: String::from(PROCESSES),
            kills: Vec::new(),
            kill_answer: Ok(()),
            removed: Vec::new(),
            existing: Vec::new(),
            folders: Vec::new(),
            fetches: Vec::new(),
        }
    }
}

fn copied(from: &[u8], into: &mut [u8]) -> usize {
    let length = from.len().min(into.len());
    into[..length].copy_from_slice(&from[..length]);
    length
}

impl DownloadSystem for FakeSystem {
    fn record_ids(&mut self, ids: &mut Vec<u32>) -> Result<(), Errno> {
        if let Some(error) = self.list_error {
            return Err(error);
        }
        ids.extend(self.records.keys().copied());
        Ok(())
    }

    fn read_record(&mut self, id: u32, text: &mut [u8]) -> Option<usize> {
        let record = self.records.get(&id)?;
        Some(copied(record, text))
    }

    fn read_processes(&mut self, text: &mut [u8]) -> Option<usize> {
        Some(copied(self.processes.as_bytes(), text))
    }

    fn send_signal(&mut self, pid: u32, signal: u32) -> Result<(), i32> {
        self.kills.push((pid, signal));
        self.kill_answer
    }

    fn remove_record(&mut self, id: u32) -> Result<(), Errno> {
        self.removed.push(id);
        match self.records.remove(&id) {
            Some(_) => Ok(()),
            None => Err(Errno::ENOENT),
        }
    }

    fn exists(&mut self, path: &str) -> bool {
        self.existing.iter().any(|known| known == path)
    }

    fn make_folder(&mut self, path: &str) -> Result<(), Errno> {
        self.folders.push(String::from(path));
        Ok(())
    }

    fn start_fetch(&mut self, url: &str, path: &str) -> Result<(), Errno> {
        self.fetches.push(fetch(url, path));
        Ok(())
    }
}

fn fetch(url: &str, path: &str) -> (String, String) {
    (String::from(url), String::from(path))
}

fn saved_paths(system: &FakeSystem) -> Vec<&str> {
    let fetches = system.fetches.iter();
    fetches.map(|(_, path)| path.as_str()).collect()
}

fn opened(system: &mut FakeSystem) -> DownloadsWindow {
    DownloadsWindow::new(system, NOW)
}

fn place(window: &DownloadsWindow, id: WidgetId) -> (i32, i32, i32, i32) {
    let rect = window.widgets().rect(id).unwrap_or_default();
    (rect.x, rect.y, rect.width, rect.height)
}

fn enabled(window: &DownloadsWindow, id: WidgetId) -> bool {
    window.widgets().is_enabled(id)
}

fn is_primary(window: &DownloadsWindow, id: WidgetId) -> bool {
    match window.widgets().kind(id) {
        Some(Kind::Button { primary, .. }) => *primary,
        _ => false,
    }
}

fn placeholder_of(window: &DownloadsWindow) -> &'static str {
    match window.widgets().kind(window.widgets.field) {
        Some(Kind::TextField(field)) => field.placeholder,
        _ => "",
    }
}

fn field_text(window: &DownloadsWindow) -> &str {
    let field = window.widgets.field;
    window.widgets().text_of(field).unwrap_or("")
}

fn table_of(window: &DownloadsWindow) -> Table {
    match window.widgets().kind(window.widgets.table) {
        Some(Kind::Table(table)) => *table,
        _ => panic!("the window has its table"),
    }
}

fn titles_of(window: &DownloadsWindow) -> Vec<&'static str> {
    let table = table_of(window);
    let columns = table.columns();
    columns.iter().map(|column| column.title).collect()
}

fn widths_of(window: &DownloadsWindow) -> Vec<i32> {
    let table = table_of(window);
    let columns = table.columns();
    columns.iter().map(|column| column.width).collect()
}

fn cells_of(window: &DownloadsWindow, row: usize) -> Vec<String> {
    let table = table_of(window);
    (0..table.column_count)
        .map(|column| String::from(table.cell_text(row, column).as_str()))
        .collect()
}

fn row_text(window: &DownloadsWindow, row: usize) -> String {
    cells_of(window, row).join("\t")
}

fn ids(window: &DownloadsWindow) -> Vec<u32> {
    window.rows.iter().map(|row| row.id).collect()
}

fn bar_of(window: &DownloadsWindow) -> Option<u16> {
    let progress = window.widgets.progress;
    if !window.widgets().is_shown(progress) {
        return None;
    }
    match window.widgets().kind(progress) {
        Some(Kind::Progress { permille }) => Some(*permille),
        _ => None,
    }
}

fn line_of(window: &DownloadsWindow) -> Option<(String, Token)> {
    let label = window.widgets.line;
    if !window.widgets().is_shown(label) {
        return None;
    }
    match window.widgets().kind(label) {
        Some(Kind::Label { text, colour, .. }) => Some((String::from(text.as_str()), *colour)),
        _ => None,
    }
}

fn detail(words: &str) -> Option<(String, Token)> {
    Some((String::from(words), Token::TextDim))
}

fn status(words: &str) -> Option<(String, Token)> {
    Some((String::from(words), Token::Text))
}

fn key_input(code: u16, character: u32, pressed: bool) -> Input {
    Input::Key {
        code,
        pressed,
        modifiers: Modifiers::default(),
        character,
    }
}

fn press(window: &mut DownloadsWindow, system: &mut FakeSystem, code: u16, character: u32) {
    for pressed in [true, false] {
        let input = key_input(code, character, pressed);
        window.handle(system, input, NOW);
    }
}

fn type_text(window: &mut DownloadsWindow, system: &mut FakeSystem, words: &str) {
    for character in words.chars() {
        press(window, system, key::A, character as u32);
    }
}

fn click(window: &mut DownloadsWindow, system: &mut FakeSystem, x: i32, y: i32) {
    let moved = Input::PointerMove { x, y };
    window.handle(system, moved, NOW);
    for pressed in [true, false] {
        let input = Input::Button {
            x,
            y,
            button: MouseButton::Left,
            pressed,
        };
        window.handle(system, input, NOW);
    }
}

fn focus_field(window: &mut DownloadsWindow) {
    let field = window.widgets.field;
    window.tree().set_focus(Some(field), NOW);
}

fn invoke(window: &mut DownloadsWindow, system: &mut FakeSystem, id: WidgetId) -> Outcome {
    let element = element_of(id);
    window.act(system, element, Action::Invoke, "", NOW)
}

fn within(inner: &Rect, outer: Rect) -> bool {
    let (left, top) = (outer.x - 5, outer.y - 5);
    let (right, bottom) = (outer.right() + 5, outer.bottom() + 5);
    inner.x >= left && inner.y >= top && inner.right() <= right && inner.bottom() <= bottom
}

#[test]
fn the_window_opens_with_the_designed_places_and_the_newest_download_selected() {
    let mut system = FakeSystem::example();
    let window = opened(&mut system);
    let w = window.widgets;
    assert_eq!(place(&window, w.field), (16, 16, 504, 28));
    assert_eq!(placeholder_of(&window), PLACEHOLDER);
    assert_eq!(place(&window, w.download), (528, 16, 96, 28));
    assert!(is_primary(&window, w.download));
    assert!(!enabled(&window, w.download));
    assert_eq!(place(&window, w.table), (16, 52, 608, 196));
    assert_eq!(window.widgets().focus(), Some(w.table));
    let table = table_of(&window);
    assert_eq!(table.name, "Downloads");
    assert_eq!(titles_of(&window), ["Name", "Size", "State", "From"]);
    assert_eq!(widths_of(&window), [232, 152, 112, 112]);
    assert!(table.columns().iter().all(|column| !column.sortable));
    assert_eq!(ids(&window), [61, 58, 55, 50]);
    assert_eq!(window.widgets().selected(w.table), Some(0));
    for (row, expected) in ROWS.into_iter().enumerate() {
        assert_eq!(row_text(&window, row), expected);
    }
    let at = Rect::new(16, 52, 608, 196);
    let first = table.row_rect(at, 0);
    assert_eq!((first.x + TABLE_MARK_X, first.y), (24, 80));
    assert_eq!(first.x + TABLE_FIRST_TEXT_X, 40);
    assert_eq!(table.column_x(at, 2) - TABLE_CELL_PADDING, 392);
    assert_eq!(table.column_x(at, 2) + TABLE_CELL_PADDING, 408);
    assert_eq!(table.column_x(at, 3) + TABLE_CELL_PADDING, 520);
    assert_eq!(place(&window, w.progress), (16, 260, 608, 8));
    assert_eq!(bar_of(&window), Some(283));
    assert_eq!(place(&window, w.line), (16, 294, 392, 16));
    assert_eq!(line_of(&window), detail(SAVING_EGGOS));
    assert_eq!(place(&window, w.clear), (424, 288, 96, 28));
    assert!(enabled(&window, w.clear));
    assert_eq!(place(&window, w.cancel), (528, 288, 96, 28));
    assert!(enabled(&window, w.cancel));
}

#[test]
fn down_to_each_download_that_is_over_shows_its_detail_hides_the_bar_and_disables_cancel() {
    let mut system = FakeSystem::example();
    let mut window = opened(&mut system);
    let w = window.widgets;
    press(&mut window, &mut system, key::DOWN, 0);
    assert_eq!(window.widgets().selected(w.table), Some(1));
    assert_eq!(line_of(&window), detail(SAVED_NOTES));
    assert_eq!(bar_of(&window), None);
    assert!(!enabled(&window, w.cancel));
    assert!(enabled(&window, w.clear));
    press(&mut window, &mut system, key::DOWN, 0);
    assert_eq!(line_of(&window), detail(FAILED_ZIP));
    press(&mut window, &mut system, key::DOWN, 0);
    assert_eq!(line_of(&window), detail(CANCELLED_LINE));
    assert_eq!(cells_of(&window, 3)[1], "1.0 MB of 700 MB");
    assert_eq!(bar_of(&window), None);
    assert!(!enabled(&window, w.cancel));
    press(&mut window, &mut system, key::HOME, 0);
    assert_eq!(line_of(&window), detail(SAVING_EGGOS));
    assert_eq!(bar_of(&window), Some(283));
    assert!(enabled(&window, w.cancel));
}

#[test]
fn a_refresh_a_second_later_shows_what_arrived_and_damages_only_the_table_and_the_bar() {
    let mut system = FakeSystem::example();
    let mut window = opened(&mut system);
    let w = window.widgets;
    assert_eq!(window.deadline(NOW), Some(NOW + REFRESH_MS));
    let grown = eggos(6_291_456, State::Downloading);
    system.records.insert(61, grown);
    window.tick(&mut system, NOW + REFRESH_MS - 1);
    assert_eq!(cells_of(&window, 0)[1], "3.4 MB of 12 MB");
    window.tree().take_damage();
    window.tick(&mut system, NOW + REFRESH_MS);
    assert_eq!(cells_of(&window, 0)[1], "6.0 MB of 12 MB");
    assert_eq!(cells_of(&window, 0)[2], "Downloading, 50 %");
    assert_eq!(bar_of(&window), Some(500));
    let damage = window.tree().take_damage();
    let table = window.widgets().rect(w.table).unwrap_or_default();
    let bar = window.widgets().rect(w.progress).unwrap_or_default();
    assert!(!damage.is_empty());
    for rect in damage.rects() {
        assert!(within(rect, table) || within(rect, bar), "{rect:?}");
    }
}

#[test]
fn a_refresh_with_nothing_new_damages_nothing() {
    let mut system = FakeSystem::example();
    let mut window = opened(&mut system);
    window.tree().take_damage();
    let later = NOW + REFRESH_MS;
    window.tick(&mut system, later);
    assert!(window.tree().take_damage().is_empty());
    assert_eq!(window.deadline(later), Some(later + REFRESH_MS));
}

#[test]
fn a_new_download_goes_on_top_and_the_selection_stays_on_the_same_download() {
    let mut system = FakeSystem::example();
    let mut window = opened(&mut system);
    let w = window.widgets;
    let path = "/home/root/Downloads/new.bin";
    let newer = running(path, 1_793_282_700);
    system.records.insert(70, newer);
    system.processes.push_str("70 7 70 7 R 0 fetch\n");
    press(&mut window, &mut system, F5, 0);
    assert_eq!(ids(&window), [70, 61, 58, 55, 50]);
    assert_eq!(cells_of(&window, 0)[0], "new.bin");
    assert_eq!(cells_of(&window, 0)[2], "Downloading");
    assert_eq!(window.widgets().selected(w.table), Some(1));
    assert_eq!(cells_of(&window, 1)[0], EGGOS_NAME);
    assert_eq!(line_of(&window), detail(SAVING_EGGOS));
    assert!(enabled(&window, w.cancel));
}

#[test]
fn a_running_record_whose_fetch_is_gone_reads_failed_without_finishing() {
    let mut system = FakeSystem::example();
    let mut window = opened(&mut system);
    let w = window.widgets;
    system.processes = String::from("1 0 1 1 R 10 init\n");
    press(&mut window, &mut system, F5, 0);
    assert_eq!(cells_of(&window, 0)[2], ENDED_LINE);
    assert_eq!(line_of(&window), detail(ENDED_LINE));
    assert_eq!(bar_of(&window), None);
    assert!(!enabled(&window, w.cancel));
    press(&mut window, &mut system, key::DELETE, 0);
    assert!(system.kills.is_empty());
}

#[test]
fn delete_cancels_the_selected_download_and_says_so_until_the_refresh_shows_it_cancelled() {
    let mut system = FakeSystem::example();
    let mut window = opened(&mut system);
    let w = window.widgets;
    press(&mut window, &mut system, key::DELETE, 0);
    assert_eq!(system.kills, [(61, TERMINATE)]);
    assert_eq!(line_of(&window), status(CANCELLING_EGGOS));
    press(&mut window, &mut system, F5, 0);
    assert_eq!(line_of(&window), status(CANCELLING_EGGOS));
    let cancelled = eggos(EGGOS_RECEIVED, State::Cancelled);
    system.records.insert(61, cancelled);
    press(&mut window, &mut system, F5, 0);
    assert_eq!(cells_of(&window, 0)[2], "Cancelled");
    assert_eq!(line_of(&window), detail(CANCELLED_LINE));
    assert_eq!(bar_of(&window), None);
    assert!(!enabled(&window, w.cancel));
}

#[test]
fn a_cancel_that_finds_the_download_over_or_another_user_s_says_why() {
    let mut system = FakeSystem {
        kill_answer: Err(ESRCH),
        ..FakeSystem::example()
    };
    let mut window = opened(&mut system);
    press(&mut window, &mut system, key::DELETE, 0);
    assert_eq!(system.kills, [(61, TERMINATE)]);
    assert_eq!(line_of(&window), status(ALREADY_ENDED));
    let mut system = FakeSystem {
        kill_answer: Err(EPERM),
        ..FakeSystem::example()
    };
    let mut window = opened(&mut system);
    press(&mut window, &mut system, key::DELETE, 0);
    assert_eq!(line_of(&window), status(NOT_ALLOWED));
    let mut system = FakeSystem::example();
    let mut window = opened(&mut system);
    let done = eggos(EGGOS_RECEIVED, State::Done);
    system.records.insert(61, done);
    press(&mut window, &mut system, key::DELETE, 0);
    assert!(system.kills.is_empty());
    assert_eq!(line_of(&window), status(ALREADY_ENDED));
}

#[test]
fn a_click_on_cancel_does_what_delete_does() {
    let mut system = FakeSystem::example();
    let mut window = opened(&mut system);
    click(&mut window, &mut system, 576, 302);
    assert_eq!(system.kills, [(61, TERMINATE)]);
    assert_eq!(line_of(&window), status(CANCELLING_EGGOS));
}

#[test]
fn clear_removes_the_records_of_downloads_that_are_over_and_keeps_the_running_one() {
    let mut system = FakeSystem::example();
    let mut window = opened(&mut system);
    let w = window.widgets;
    press(&mut window, &mut system, key::DOWN, 0);
    click(&mut window, &mut system, 472, 302);
    system.removed.sort_unstable();
    assert_eq!(system.removed, [50, 55, 58]);
    assert_eq!(ids(&window), [61]);
    assert_eq!(window.widgets().selected(w.table), Some(0));
    assert_eq!(line_of(&window), status("Cleared 3 downloads."));
    assert!(!enabled(&window, w.clear));
    assert!(enabled(&window, w.cancel));
    assert_eq!(bar_of(&window), Some(283));
}

#[test]
fn clearing_every_download_shows_the_empty_text_and_focuses_the_field() {
    let mut system = FakeSystem::example();
    let mut window = opened(&mut system);
    let w = window.widgets;
    let cancelled = eggos(EGGOS_RECEIVED, State::Cancelled);
    system.records.insert(61, cancelled);
    click(&mut window, &mut system, 472, 302);
    assert_eq!(system.removed.len(), 4);
    assert!(system.records.is_empty());
    assert_eq!(line_of(&window), status("Cleared 4 downloads."));
    assert_eq!(table_of(&window).rows, 0);
    assert_eq!(table_of(&window).empty_text, EMPTY);
    assert_eq!(bar_of(&window), None);
    assert!(!enabled(&window, w.clear));
    assert!(!enabled(&window, w.cancel));
    assert_eq!(window.widgets().focus(), Some(w.field));
}

#[test]
fn typing_a_web_address_enables_download_and_enter_starts_fetch_into_downloads() {
    let mut system = FakeSystem::example();
    let mut window = opened(&mut system);
    let w = window.widgets;
    focus_field(&mut window);
    assert!(!enabled(&window, w.download));
    type_text(&mut window, &mut system, ADDRESS);
    assert!(enabled(&window, w.download));
    press(&mut window, &mut system, key::ENTER, 0);
    let path = "/home/root/Downloads/eggos.elf";
    assert_eq!(system.fetches, [fetch(ADDRESS, path)]);
    assert_eq!(system.folders, ["/home/root", SAVE_FOLDER]);
    assert_eq!(field_text(&window), "");
    assert!(!enabled(&window, w.download));
    assert_eq!(window.widgets().focus(), Some(w.field));
    assert_eq!(line_of(&window), status("Starting eggos.elf..."));
    let started = running(path, 1_793_282_700);
    system.records.insert(80, started);
    system.processes.push_str("80 7 80 7 R 0 fetch\n");
    press(&mut window, &mut system, F5, 0);
    assert_eq!(ids(&window)[0], 80);
    assert_eq!(window.widgets().selected(w.table), Some(0));
    assert_eq!(
        line_of(&window),
        detail("Saving to /home/root/Downloads/eggos.elf")
    );
}

#[test]
fn a_name_already_in_the_folder_saves_as_the_first_free_numbered_copy() {
    let mut system = FakeSystem {
        existing: vec![String::from("/home/root/Downloads/eggos.elf")],
        ..FakeSystem::example()
    };
    let mut window = opened(&mut system);
    focus_field(&mut window);
    type_text(&mut window, &mut system, ADDRESS);
    press(&mut window, &mut system, key::ENTER, 0);
    let path = "/home/root/Downloads/eggos (2).elf";
    assert_eq!(system.fetches, [fetch(ADDRESS, path)]);
    let starting = "Starting eggos (2).elf...";
    assert_eq!(line_of(&window), status(starting));
}

#[test]
fn the_saved_name_is_the_address_s_last_segment_or_index_html() {
    let mut system = FakeSystem::example();
    let mut window = opened(&mut system);
    focus_field(&mut window);
    for address in ["https://example.com/", "https://example.com/a/b?x=1"] {
        type_text(&mut window, &mut system, address);
        press(&mut window, &mut system, key::ENTER, 0);
    }
    let saved = saved_paths(&system);
    assert_eq!(saved.len(), 2);
    assert_eq!(saved[0], "/home/root/Downloads/index.html");
    assert_eq!(saved[1], "/home/root/Downloads/b");
}

#[test]
fn an_address_without_http_or_https_is_refused_and_kept_and_esc_empties_the_field() {
    let mut system = FakeSystem::example();
    let mut window = opened(&mut system);
    let w = window.widgets;
    focus_field(&mut window);
    type_text(&mut window, &mut system, "example.com/x");
    assert!(enabled(&window, w.download));
    press(&mut window, &mut system, key::ENTER, 0);
    assert!(system.fetches.is_empty());
    assert_eq!(line_of(&window), status(NOT_A_WEB_ADDRESS));
    assert_eq!(field_text(&window), "example.com/x");
    press(&mut window, &mut system, key::ESC, 0);
    assert_eq!(field_text(&window), "");
    assert!(!enabled(&window, w.download));
}

#[test]
fn a_phone_s_360_wide_window_stacks_full_width_controls_over_two_columns() {
    let mut system = FakeSystem::example();
    let mut window = opened(&mut system);
    let w = window.widgets;
    window.resize(360, 640);
    assert_eq!(place(&window, w.field), (16, 16, 328, 28));
    assert_eq!(place(&window, w.download), (16, 52, 328, 40));
    assert_eq!(place(&window, w.table), (16, 100, 328, 384));
    assert_eq!(place(&window, w.progress), (16, 496, 328, 8));
    assert_eq!(place(&window, w.line), (16, 512, 328, 16));
    assert_eq!(place(&window, w.clear), (16, 536, 328, 40));
    assert_eq!(place(&window, w.cancel), (16, 584, 328, 40));
    assert_eq!(titles_of(&window), ["Name", "State"]);
    assert_eq!(widths_of(&window), [208, 120]);
    assert_eq!(cells_of(&window, 0), [EGGOS_NAME, "28 %"]);
    assert_eq!(cells_of(&window, 1), ["notes.txt", "Done"]);
    assert_eq!(cells_of(&window, 2)[1], "Failed");
    let (words, ink) = line_of(&window).expect("the detail line shows");
    let prefix = "3.4 MB of 12 MB \u{b7} Saving to /home/root/Downloads/";
    assert!(words.starts_with(prefix));
    assert!(words.ends_with("..."));
    assert_eq!(words.len(), 64);
    assert_eq!(ink, Token::TextDim);
    window.resize(640, 332);
    assert_eq!(widths_of(&window), [232, 152, 112, 112]);
    assert_eq!(cells_of(&window, 0)[2], "Downloading, 28 %");
    assert_eq!(line_of(&window), detail(SAVING_EGGOS));
}

#[test]
fn agents_cannot_press_download_or_cancel_but_may_select_rows_and_clear() {
    let mut system = FakeSystem::example();
    let mut window = opened(&mut system);
    let w = window.widgets;
    assert!(window.widgets().is_person_only(w.download));
    assert!(window.widgets().is_person_only(w.cancel));
    assert!(!window.widgets().is_person_only(w.clear));
    for refused in [w.download, w.cancel] {
        let outcome = invoke(&mut window, &mut system, refused);
        assert_eq!(outcome, Outcome::NotSupported);
    }
    assert!(system.kills.is_empty());
    assert!(system.fetches.is_empty());
    let table = element_of(w.table);
    let select = Action::SetText;
    let outcome = window.act(&mut system, table, select, "2", NOW);
    assert_eq!(outcome, Outcome::Done);
    assert_eq!(window.widgets().selected(w.table), Some(1));
    assert_eq!(line_of(&window), detail(SAVED_NOTES));
    let cleared = invoke(&mut window, &mut system, w.clear);
    assert_eq!(cleared, Outcome::Done);
    assert_eq!(ids(&window), [61]);
}

#[test]
fn a_minimised_window_stops_refreshing_and_refreshes_at_once_when_restored() {
    let mut system = FakeSystem::example();
    let mut window = opened(&mut system);
    let minimised = Input::StateChanged { state: 1 };
    window.handle(&mut system, minimised, NOW);
    assert_eq!(window.deadline(NOW), None);
    let grown = eggos(6_291_456, State::Downloading);
    system.records.insert(61, grown);
    let later = NOW + 5 * REFRESH_MS;
    window.tick(&mut system, later);
    assert_eq!(cells_of(&window, 0)[1], "3.4 MB of 12 MB");
    let restored = Input::StateChanged { state: 0 };
    window.handle(&mut system, restored, later);
    assert_eq!(window.deadline(later), Some(later));
    window.tick(&mut system, later);
    assert_eq!(cells_of(&window, 0)[1], "6.0 MB of 12 MB");
    assert_eq!(window.deadline(later), Some(later + REFRESH_MS));
}

#[test]
fn an_unreadable_download_list_says_so_and_still_takes_an_address() {
    let mut system = FakeSystem {
        list_error: Some(Errno::EACCES),
        ..FakeSystem::example()
    };
    let mut window = opened(&mut system);
    let w = window.widgets;
    assert_eq!(table_of(&window).rows, 0);
    assert_eq!(table_of(&window).empty_text, UNREADABLE);
    assert_eq!(window.widgets().focus(), Some(w.field));
    assert_eq!(line_of(&window), None);
    assert!(!enabled(&window, w.clear));
    assert!(!enabled(&window, w.cancel));
    type_text(&mut window, &mut system, ADDRESS);
    press(&mut window, &mut system, key::ENTER, 0);
    assert_eq!(system.fetches.len(), 1);
}

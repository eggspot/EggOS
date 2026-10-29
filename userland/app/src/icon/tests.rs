use super::bitmap::samples::bitmap;
use super::icns::samples::icns;
use super::pe::samples::program_with_icons;
use super::*;
use crate::fake::FakeFiles;
use crate::reader::Bytes;
use eggos_png::{Encoder, EncoderWorkspace, PngSink};

struct PngBytes(Vec<u8>);

impl PngSink for PngBytes {
    type Error = ();

    fn write(&mut self, bytes: &[u8]) -> Result<(), ()> {
        self.0.extend_from_slice(bytes);
        Ok(())
    }
}

pub(crate) fn solid_png(side: u32, colour: u32) -> Vec<u8> {
    let mut work = Box::new(EncoderWorkspace::new());
    let mut encoder = Encoder::new(side, side, &mut work, PngBytes(Vec::new())).unwrap();
    for _ in 0..side {
        encoder.write_row(&vec![colour; side as usize]).unwrap();
    }
    encoder.finish().unwrap().0
}

fn candidate(side: u32, rank: u8, format: Format) -> Candidate {
    Candidate {
        side,
        rank,
        offset: 0,
        length: 0,
        format,
    }
}

fn sides_in_order(list: &Candidates, target: u32) -> Vec<(u32, u8)> {
    let (order, count) = order(list, target);
    order[..count]
        .iter()
        .map(|&index| {
            let chosen = list.iter().nth(index).unwrap();
            (chosen.side, chosen.rank)
        })
        .collect()
}

#[test]
fn the_exact_size_comes_first_then_the_smallest_larger_then_the_largest_smaller() {
    let mut list = Candidates::new();
    for side in [16, 32, 256] {
        list.push(candidate(side, 0, Format::Png));
    }
    assert_eq!(sides_in_order(&list, 32)[0], (32, 0));
    assert_eq!(sides_in_order(&list, 48), [(256, 0), (32, 0), (16, 0)]);
    let mut list = Candidates::new();
    list.push(candidate(16, 0, Format::Png));
    list.push(candidate(32, 0, Format::Png));
    assert_eq!(sides_in_order(&list, 48)[0], (32, 0));
    let mut list = Candidates::new();
    list.push(candidate(32, 1, Format::Argb));
    list.push(candidate(32, 2, Format::Planes { mask: None }));
    list.push(candidate(32, 0, Format::Png));
    list.push(candidate(48, 2, Format::Planes { mask: None }));
    list.push(candidate(128, 0, Format::Png));
    assert_eq!(sides_in_order(&list, 32)[..3], [(32, 0), (32, 1), (32, 2)]);
    assert_eq!(sides_in_order(&list, 48)[0], (48, 2));
    let mut list = Candidates::new();
    list.push(candidate(0, 0, Format::Png));
    list.push(candidate(MAX_SIDE + 1, 0, Format::Png));
    assert!(list.is_empty());
}

#[test]
fn a_png_larger_than_both_sizes_is_averaged_down() {
    let png = solid_png(64, 0x0012_3456);
    let mut work = Box::new(Workspace::new());
    let mut icon = OwnIcon::new();
    assert!(from_png(&mut Bytes(&png), &mut work, &mut icon));
    assert!(icon.large().iter().all(|&pixel| pixel == 0xff12_3456));
    assert!(icon.small().iter().all(|&pixel| pixel == 0xff12_3456));
    let file = icon.file();
    assert_eq!(file.len(), 13_312);
    assert_eq!(file[..4], [0x56, 0x34, 0x12, 0xff]);
    let mut none = OwnIcon::new();
    assert!(!from_png(&mut Bytes(b"not a png"), &mut work, &mut none));
    let mut huge = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    huge.extend_from_slice(&2000u32.to_be_bytes());
    huge.extend_from_slice(&2000u32.to_be_bytes());
    assert!(!from_png(&mut Bytes(&huge), &mut work, &mut none));
}

#[test]
fn a_windows_program_s_bitmaps_give_both_sizes() {
    let opaque_row = |side: usize, bgra: [u8; 4]| bgra.repeat(side);
    let large = bitmap(
        48,
        32,
        &[],
        &vec![opaque_row(48, [0x30, 0x20, 0x10, 0xff]); 48],
        &vec![vec![0; 8]; 48],
    );
    let tiny = bitmap(
        16,
        32,
        &[],
        &vec![opaque_row(16, [0, 0, 0xff, 0xff]); 16],
        &vec![vec![0; 4]; 16],
    );
    let program = program_with_icons(&[(1, tiny), (2, large)]);
    let mut work = Box::new(Workspace::new());
    let mut icon = OwnIcon::new();
    assert!(from_exe(&mut Bytes(&program), &mut work, &mut icon));
    assert!(icon.large().iter().all(|&pixel| pixel == 0xff10_2030));
    assert!(icon.small().iter().all(|&pixel| pixel == 0xff10_2030));
    let mut none = OwnIcon::new();
    let plain = crate::program::samples::windows_program(0x8664);
    assert!(!from_exe(&mut Bytes(&plain), &mut work, &mut none));
}

#[test]
fn an_icns_png_entry_is_scaled_and_a_small_planar_one_grows() {
    let file = icns(&[(b"ic07", solid_png(128, 0x00ff_0000)), (b"is32", vec![0x40; 3 * 256])]);
    let mut work = Box::new(Workspace::new());
    let mut icon = OwnIcon::new();
    assert!(from_icns(&mut Bytes(&file), &mut work, &mut icon));
    assert!(icon.large().iter().all(|&pixel| pixel == 0xffff_0000));
    let only_small = icns(&[(b"is32", vec![0x40; 3 * 256])]);
    assert!(from_icns(&mut Bytes(&only_small), &mut work, &mut icon));
    assert!(icon.large().iter().all(|&pixel| pixel == 0xff40_4040));
    assert!(icon.small().iter().all(|&pixel| pixel == 0xff40_4040));
}

const PLIST: &[u8] = b"<?xml version=\"1.0\"?>\n<plist version=\"1.0\">\n<dict>\n\t<key>CFBundleExecutable</key>\n\t<string>Paint</string>\n\t<key>CFBundleIconFile</key>\n\t<string> Paint </string>\n</dict>\n</plist>\n";

#[test]
fn info_plists_bundles_and_pngs_beside_are_found_by_their_paths() {
    assert_eq!(plist_icon_file(PLIST), Some(&b"Paint"[..]));
    assert_eq!(
        plist_icon_file(b"<key>CFBundleIconFile</key><string>../x</string>"),
        None
    );
    assert_eq!(
        plist_icon_file(b"<key>CFBundleIconFile</key><integer>1</integer>"),
        None
    );
    assert_eq!(plist_icon_file(b"<dict/>"), None);
    assert_eq!(
        bundle_of(b"/Applications/Paint/Paint.app/Contents/MacOS/Paint"),
        Some(&b"/Applications/Paint/Paint.app"[..])
    );
    assert_eq!(bundle_of(b"/Applications/x/x.exe"), None);
    assert_eq!(bundle_of(b"/a/Folder/Contents/MacOS/x"), None);
    assert_eq!(
        png_beside(b"/media/host/win-paint.exe").unwrap().as_bytes(),
        b"/media/host/win-paint.png"
    );
    assert_eq!(
        png_beside(b"/media/host/Paint.app").unwrap().as_bytes(),
        b"/media/host/Paint.png"
    );
}

fn bundle_files(icon_name: &str, plist: &[u8]) -> FakeFiles {
    let mut files = FakeFiles::new();
    for folder in [
        "/Applications/Paint",
        "/Applications/Paint/Paint.app",
        "/Applications/Paint/Paint.app/Contents",
        "/Applications/Paint/Paint.app/Contents/MacOS",
        "/Applications/Paint/Paint.app/Contents/Resources",
    ] {
        files.directory(folder);
    }
    files.file("/Applications/Paint/Paint.app/Contents/Info.plist", plist);
    files.file(
        "/Applications/Paint/Paint.app/Contents/MacOS/Paint",
        &[0xcf, 0xfa, 0xed, 0xfe],
    );
    files.file(
        &format!("/Applications/Paint/Paint.app/Contents/Resources/{icon_name}"),
        &icns(&[(b"ic07", solid_png(128, 0x0000_ff00))]),
    );
    files
}

#[test]
fn a_bundle_s_icon_is_the_one_its_info_plist_names_else_the_only_one() {
    const PROGRAM: &[u8] = b"/Applications/Paint/Paint.app/Contents/MacOS/Paint";
    let mut work = Box::new(Workspace::new());
    for (icon_name, plist) in [
        ("Paint.icns", PLIST),
        ("AppIcon.icns", &b"<dict/>"[..]),
        ("Other.icns", &b"<dict/>"[..]),
    ] {
        let mut files = bundle_files(icon_name, plist);
        let mut icon = OwnIcon::new();
        assert!(find(&mut files, PROGRAM, None, &mut work, &mut icon), "{icon_name}");
        assert!(icon.small().iter().all(|&pixel| pixel == 0xff00_ff00), "{icon_name}");
        assert_eq!(files.open_count(), 0);
    }
    let mut files = bundle_files("Paint.icns", PLIST);
    files.file(
        "/Applications/Paint/Paint.app/Contents/Resources/Second.icns",
        b"icns",
    );
    files.nodes.retain(|(path, _)| !path.ends_with(b"Info.plist"));
    let mut icon = OwnIcon::new();
    assert!(find(&mut files, PROGRAM, None, &mut work, &mut icon));
}

#[test]
fn a_program_without_an_icon_takes_the_png_beside_it_or_has_none() {
    let mut files = FakeFiles::new();
    files.directory("/Applications/tool");
    files.file("/Applications/tool/tool", b"\x7fELF");
    files.file("/media/host/tool.png", &solid_png(32, 0x0001_0203));
    let mut work = Box::new(Workspace::new());
    let mut icon = OwnIcon::new();
    assert!(!find(&mut files, b"/Applications/tool/tool", None, &mut work, &mut icon));
    assert!(find(
        &mut files,
        b"/Applications/tool/tool",
        Some(b"/media/host/tool"),
        &mut work,
        &mut icon
    ));
    assert_eq!(icon.small()[0], 0xff01_0203);
    assert_eq!(icon.large()[0], 0);
    assert_eq!(icon.large()[8 * 48 + 8], 0xff01_0203);
    files.file("/media/host/broken.png", b"\x89PNG\r\n\x1a\n broken");
    let mut none = OwnIcon::new();
    assert!(!find(
        &mut files,
        b"/Applications/tool/tool",
        Some(b"/media/host/broken"),
        &mut work,
        &mut none
    ));
}

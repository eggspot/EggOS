//! `cargo xtask qt-app` (D179, the Real apps track's third Linux app): a real Qt app, CorePad,
//! installed and used inside EggOS through `eggway`.
//!
//! Everything here is test data, downloaded and never shipped or committed (decided by
//! research). CorePad 5.0.0 (the C Suite's text editor, GPL-3.0-or-later, a Qt Widgets app) and
//! every library it loads come from Alpine Linux `v3.24`'s packages, each checked against its
//! pinned SHA-256: Qt 6.11.1's Core, Gui, Widgets, Network, DBus, Xml and Wayland client
//! libraries, which draw with Qt's raster engine into `wl_shm` buffers and need no GPU, the C
//! Suite's `libcprime`, ICU, GLib, HarfBuzz, fontconfig, FreeType, the network and X11
//! libraries Qt is linked against (loaded, never used on Wayland), Wayland's client libraries,
//! `libxkbcommon` and musl's loader. Alpine's `libQt6Gui` is linked against Mesa's EGL and GL,
//! which bring LLVM and a GPU driver EggOS cannot run, so the app carries EggOS's own
//! `libEGL.so.1`, `libGL.so.1` and `libGLESv2.so.2` ([`crate::gl_libraries`]) instead. They are
//! packed into `corepad.zip`: the editor at `corepad`, musl's loader and every library at
//! `lib/`, Qt's Wayland platform, shell, decoration and input plugins at `plugins/`, ICU's data
//! at `share/icu` and four DejaVu fonts at `share/fonts`, which the installer links into
//! `/Applications/.fonts`, where EggOS's `/etc/fonts/fonts.conf` points fontconfig
//! ([`crate::qemu::qt_app`] boots it).

use crate::gl_libraries::{self, Machine};
use crate::sdl_app::{Package, create, fetch_pinned, fresh, read, untar};
use crate::zip::{self, Member};
use crate::{Arch, build_kernel};
use std::path::{Path, PathBuf};

/// The app's name once installed, and its program's file name in the archive.
pub const APP: &str = "corepad";
/// The archive the host folder holds.
pub const ARCHIVE: &str = "corepad.zip";
/// CorePad's version, as Alpine packages it.
pub const COREPAD_VERSION: &str = "5.0.0";
/// Qt's version, as Alpine packages it.
pub const QT_VERSION: &str = "6.11.1";
/// Where musl's loader and the libraries are inside the app's folder.
pub const LIBRARY_FOLDER: &str = "lib";
/// Where Qt's plugins are inside the app's folder: the folder `QT_PLUGIN_PATH` names.
pub const PLUGIN_FOLDER: &str = "plugins";
/// Where ICU's data is inside the app's folder: the folder `ICU_DATA` names.
pub const ICU_FOLDER: &str = "share/icu";
/// The fonts' folder inside the app's folder, the one the installer links.
pub const FONTS_FOLDER: &str = "share/fonts";
/// The file the check opens and edits.
pub const NOTE: &str = "/tmp/note.txt";
/// The name the check saves the edited note as, in the note's folder, through CorePad's Save
/// As dialog (its Ctrl+S).
pub const SAVED_NAME: &str = "saved.txt";

const PROGRAM_IN_PACKAGE: &str = "usr/bin/corepad";
const PLUGINS_IN_PACKAGE: &str = "usr/lib/qt6/plugins";
const ICU_DATA_IN_PACKAGE: &str = "usr/share/icu/78.1/icudt78l.dat";
const ICU_DATA: &str = "icudt78l.dat";
const FONTS_IN_PACKAGE: &str = "usr/share/fonts/dejavu";
const FONTS: [&str; 4] = [
    "DejaVuSans.ttf",
    "DejaVuSans-Bold.ttf",
    "DejaVuSansMono.ttf",
    "DejaVuSansMono-Bold.ttf",
];
const LIBRARIES_IN_PACKAGE: &str = "usr/lib";
const DIRECTORY_MODE: u32 = 0o040_755;
const FILE_MODE: u32 = 0o100_644;
const PROGRAM_MODE: u32 = 0o100_755;

/// The packages, pinned by research (D179): CorePad and the closure of the libraries its
/// program, its libraries and the plugins it carries name (`DT_NEEDED`) on either
/// architecture, ICU's English data and DejaVu's fonts.
pub const PACKAGES: [Package; 52] = [
    Package {
        repository: "main",
        name: "brotli-libs",
        version: "1.2.1-r0",
        x86_64: "62a7e37e43bbfc1f8525a00ad2125fd5897135dc93b849733884cb78ffd8e26c",
        aarch64: "f802b9d5a329c216f02e73607cb8f662c434767e41686367ac0afcf6d0d45d64",
    },
    Package {
        repository: "main",
        name: "c-ares",
        version: "1.34.9-r0",
        x86_64: "3539a3e11c1ef1516117d8179a546ac882086e94e0d78ad51d687b3fa2b3b5f2",
        aarch64: "1bda7f36597a3f6d616b0c8bd25cd611299bf353ec5ee2ae102e97694885c2f1",
    },
    Package {
        repository: "community",
        name: "corepad",
        version: "5.0.0-r0",
        x86_64: "e23e0dcd7e1112f1f92a1908e99e73caf5bda16a8a6f230ba92f60752f348d45",
        aarch64: "810306ae7c487c8a95ddcb1d950ecc1dc8546b559b577946b63ede74c069e0d2",
    },
    Package {
        repository: "main",
        name: "dbus-libs",
        version: "1.16.2-r2",
        x86_64: "105f6ba17454c587ca9345f9deb348012d1950588a22da42120c954d396c99b6",
        aarch64: "b09b297d622377f870da5f162563b2bb4926dda17dfb2c7b1c84c37b05e8e295",
    },
    Package {
        repository: "community",
        name: "double-conversion",
        version: "3.4.0-r0",
        x86_64: "ca85568760cee11ac02f2951525c1d74271aff2d41389296a69e97efc4d99ae6",
        aarch64: "8bd4428d5e5626005e26d4df2bb21b365f4d72ac6409d86dd7ab15cd9719df9d",
    },
    Package {
        repository: "community",
        name: "duktape-libs",
        version: "2.7.0-r3",
        x86_64: "1f0bd84d8d8f35ddcf12c7fad1d3ed6b30ece496bbfba992d9a6e96cff78c939",
        aarch64: "a2a7b52fa72110ec625107e32278046aef3081fa41d1d6950de6b66f2819eb12",
    },
    Package {
        repository: "main",
        name: "font-dejavu",
        version: "2.37-r6",
        x86_64: "6e7609e7a1aaed947afe4ddcc02bbdeec571dac5ff4ea1c04af9b8480b7b9dcf",
        aarch64: "786ad1bca0938ebb59e52894102cc31e2e8ef7a940f97d0e0b97d7a0caa5790e",
    },
    Package {
        repository: "main",
        name: "fontconfig",
        version: "2.17.1-r1",
        x86_64: "7634cf2fb1066ec97c8f41ec90ec9f036540e056276b7bf9a10dc4ec9e44d238",
        aarch64: "e4fc46de2fa0dc64ef948aab0798500f784eef15d75c9a2e8952a0e3017dacfc",
    },
    Package {
        repository: "main",
        name: "freetype",
        version: "2.14.3-r0",
        x86_64: "09d76fde675a9ca3cfeb83d72fb09cb2e9ccf79b2937daa05a1af9a37a6b7815",
        aarch64: "8f36a4b4b6c75574f2f51d62c3df27d23ec7054ae1cae97d54adad72e725e027",
    },
    Package {
        repository: "main",
        name: "glib",
        version: "2.88.1-r1",
        x86_64: "2f32203b97789bc9a92e57e2d9b89eb2e6951b69534ba082a683c6e49031cb5d",
        aarch64: "0ae89191a79ea5d5a02515e41f29b62e5e0a0a80fb059804bbbacac2d77cce4e",
    },
    Package {
        repository: "main",
        name: "graphite2",
        version: "1.3.14-r6",
        x86_64: "2caa36d901f79215753ce3b614818ca71596baf2bbb2c21b6eadcf073d81b552",
        aarch64: "07ac746525d14925865451eb20d283406b9a907bbe828e818c32d667dabf48d7",
    },
    Package {
        repository: "main",
        name: "harfbuzz",
        version: "13.2.1-r0",
        x86_64: "01f7862265ef2256bbadd0fc998df3d1243dee606df3bd8284471cf390f86f29",
        aarch64: "0f2f652308dac47cbd2e75e4da9fd8cc3d08c6db562af4a2fbdfe5ebf54ebedd",
    },
    Package {
        repository: "main",
        name: "icu-data-en",
        version: "78.1-r0",
        x86_64: "7fc4d3abd06b6e3301c82e9f872e4555d4a41e2abc28087f4286aba01e5dd5a7",
        aarch64: "3fa1dfb91a081f73c32a46f6ec054196caa37c9fd9569c433ddf6b29415dcf13",
    },
    Package {
        repository: "main",
        name: "icu-libs",
        version: "78.1-r0",
        x86_64: "ef9a3035f23886cf0da5869902c736a775cf3db151ae1ab7f1406e2a0f84bd64",
        aarch64: "2393882dcf1e3f5be98696665c3468dd0d2a85cd568d50abd232729863df9738",
    },
    Package {
        repository: "community",
        name: "libb2",
        version: "0.98.1-r3",
        x86_64: "9136ea15beed833ac05b0cee5812a5fc30ec5851d199579873138d3c58d2a294",
        aarch64: "2b92e1fa391fa53f89075eb637009f530b08841aa110987da9547085d62c5a7a",
    },
    Package {
        repository: "main",
        name: "libblkid",
        version: "2.42.3-r1",
        x86_64: "2512c2bff496ac2b8561a3c814d21d35ce10cda579b3d410f9f96da93a5aaac1",
        aarch64: "9d0f24976c575c53c12526b6e078edf34c980c153bc8e5f1c1053cbece5b8cfa",
    },
    Package {
        repository: "main",
        name: "libbsd",
        version: "0.12.2-r0",
        x86_64: "407597c04b41dbfe8a05b35d45a3453e32fae7df84aed3a06ff48dd4df1338b6",
        aarch64: "1c4129bef0862afa2a7414217e8c3a50609abfe2e264b6f7ba6a5452ed6c3b35",
    },
    Package {
        repository: "main",
        name: "libbz2",
        version: "1.0.8-r6",
        x86_64: "2737e2b23ed08d4911ee62cd0163b5f4bf0379cacf4dc35b23a00209584c98fd",
        aarch64: "28cb76bb6d11d2a55c3061b5fcb2ab84996412a248b2c9fe52bb710d0a2a84e7",
    },
    Package {
        repository: "community",
        name: "libcprime",
        version: "5.0.0-r0",
        x86_64: "2d976268a8ff2f1bb101affc5f8c9b15a5de0db323183a210bb953c41ce93075",
        aarch64: "8431becf795e91309e28877dd259c0602eb269920c320f078d2d6ad421ca4a57",
    },
    Package {
        repository: "main",
        name: "libcrypto3",
        version: "3.5.9-r0",
        x86_64: "6632d758d8f5e9ea3b650fe966f23bbf9a202f8b8dceecac93da135dec5e3689",
        aarch64: "2676a2b0b6e23ea2edccf3ee982b9842a665d52603d047de3d0a185dc316d983",
    },
    Package {
        repository: "main",
        name: "libcurl",
        version: "8.23.0-r0",
        x86_64: "1f70ca404818805a6465b615bbfe84ef861488a963d7e630108ebd7fbaa51665",
        aarch64: "64ae89f6e5d318ed5209778232191806ce49644ce9e09540b7d7830a5330d127",
    },
    Package {
        repository: "main",
        name: "libeconf",
        version: "0.8.3-r0",
        x86_64: "2a791c7957961f830cf31b04f35f814a63afcf633badc9052425ff1cf2f64380",
        aarch64: "cf0899d49ded0a6891e3cdf1b37887f2ecd77293afe1d8f3cb8937763c83f183",
    },
    Package {
        repository: "main",
        name: "libexpat",
        version: "2.9.0-r0",
        x86_64: "32b127f31e0a919110a18a7f0bb3780d8fb66ba4d8d61b2d301d633f0e319d0e",
        aarch64: "7128bfe18b8a315b9cf90ddf0beffeee454648ae72ff7898ff7d7b09803a559d",
    },
    Package {
        repository: "main",
        name: "libffi",
        version: "3.5.2-r1",
        x86_64: "6da1a19313a40c56cac756fb11e2c4d5267831e1c22fc8323f24c943c94da6e0",
        aarch64: "9a67280b0c7c62db658c2d4d32ed1199903bef859c32dd4245e1a7e35c89ab4b",
    },
    Package {
        repository: "main",
        name: "libgcc",
        version: "15.2.0-r5",
        x86_64: "393dcd32629f06d7d85409c272d142d0c082772d10b87ef55ee82f47de3be637",
        aarch64: "369aaa6e9d099a737bad6dd3e6c2fe7bb1547ca26d22b94ee0411228f709b403",
    },
    Package {
        repository: "main",
        name: "libgomp",
        version: "15.2.0-r5",
        x86_64: "e7204aad00e31d544e0a34eac5a7eb7d3586d2b7b22afd6e5907189c4e0337cc",
        aarch64: "90576a8899c560d3983b8ac8442b5b75957a917bc1c4e950e005945080f8682f",
    },
    Package {
        repository: "main",
        name: "libidn2",
        version: "2.3.8-r0",
        x86_64: "7d038ac558dde464964b033ce5126dd2eaf84cc414dc15e77c5424a321d0ebee",
        aarch64: "d02ffda4510652f89c5fb89cd2e7ff721afb0c74e7d75476ad60bebe7ac0ae20",
    },
    Package {
        repository: "main",
        name: "libintl",
        version: "1.0-r0",
        x86_64: "cb03919cba5624b5e8759d04f3b31913b02585355988b914bc31de4746ff5ef6",
        aarch64: "d106a13e3e68103d3a8217406aae6186289baf352d0f3d10fc21938d2dd8adb1",
    },
    Package {
        repository: "main",
        name: "libmd",
        version: "1.2.0-r0",
        x86_64: "f6d68d072fcefc8edd4ffe3afc341ca391f8a9a209130628fb065d98b18a2792",
        aarch64: "cb94430ebc45dbd5dd6882bfd296bb73ea3c337691f469bd1438e696a6e867eb",
    },
    Package {
        repository: "main",
        name: "libmount",
        version: "2.42.3-r1",
        x86_64: "e2ae003260f2c1ec13666c6e108d5e8e3d64974b0667166a02432bd3be263a7a",
        aarch64: "a2a6fc6aab7a59c70ec060f5fb8c950be42d1a017934bf32e5dd6984eaab15cc",
    },
    Package {
        repository: "main",
        name: "libpcre2-16",
        version: "10.49-r0",
        x86_64: "097f71794dfb16e1d9744e30abc6c6728394912547130735cdaee3bb83f1c8ad",
        aarch64: "645e2618496df0907003964b5c6ca4d16494675607e64b9f1f3ad73d4f448ad6",
    },
    Package {
        repository: "main",
        name: "libpng",
        version: "1.6.59-r0",
        x86_64: "d16aeb0597f2bc43e0583ac9ff779388ec160c3e22568b640122e94ba55be6d7",
        aarch64: "bccd127f8c42bc5c21c018b6b1dc977128f3bad64c3412e541c09e96e8b227a4",
    },
    Package {
        repository: "community",
        name: "libproxy",
        version: "0.5.12-r0",
        x86_64: "9656ff4015e4f2654f58e99dde115dd6c243bf6d22b92cfcc5f7b63f3b44d2e8",
        aarch64: "5c4d4e7deedccc48774b2cd547868bf909e0d6bbe42be9b4e5c064a41d94b036",
    },
    Package {
        repository: "main",
        name: "libpsl",
        version: "0.21.5-r3",
        x86_64: "ec665a344b6e887f00f0a47015cc732c3a281b69e84641dc8662ea234fde6cdb",
        aarch64: "b31e91371425d5f80c1db5cba2a849956b65c802add0b49de844b2ca961aa8da",
    },
    Package {
        repository: "main",
        name: "libssl3",
        version: "3.5.9-r0",
        x86_64: "05e3393fb95aa5751ca2f9d242f659f6cff82c1cc7767cc2df4a086f7ad01877",
        aarch64: "20ac252b276d73f2c69c1d25f84537c7fba81caefc026c6be1094b394af2082e",
    },
    Package {
        repository: "main",
        name: "libstdc++",
        version: "15.2.0-r5",
        x86_64: "14c987b556f5385a5db18376e788c75f37d85321b8dc1920d926ea7daac1d6f6",
        aarch64: "2302e766d4e4926038ec166ecb85837ee884576115236ddb565e3a5fca4a11d7",
    },
    Package {
        repository: "main",
        name: "libunistring",
        version: "1.4.2-r0",
        x86_64: "3aa6030e603e34d96192742f327c0509f409c6769ce2e7b1463685dede38d714",
        aarch64: "58abfcbce2459071c505771a572a79953d6043d72b643ed0d5285ca39cdaf85c",
    },
    Package {
        repository: "main",
        name: "libx11",
        version: "1.8.14-r0",
        x86_64: "016efc189c6d566061ffe206ed0780e32686c1a8c832a3917d35d2c66d61a40f",
        aarch64: "d460c111c1fc40e8f84367907b3e50a58a2b19c3e9d54f7c1f0e4aea7a376e3b",
    },
    Package {
        repository: "main",
        name: "libxau",
        version: "1.0.12-r0",
        x86_64: "3506cdba7ed916a6f814a010fbb127dbfdae4147107f64a15f5fa2651c5d8510",
        aarch64: "179a76248d0eea545f3affafc1275358dc671547a2ab76ff3e93edecb612864b",
    },
    Package {
        repository: "main",
        name: "libxcb",
        version: "1.17.0-r2",
        x86_64: "7935d5ad9c7137b7b3f1292d62f55f7e0cc5c919155218c3060ea46503b3f211",
        aarch64: "21dbe6dfbc477baa1bad13b82b8e61b83e34e1c474d3913f33ad5fc5b890afa0",
    },
    Package {
        repository: "main",
        name: "libxdmcp",
        version: "1.1.5-r1",
        x86_64: "2a93f44f3f85c1eda9ed53691c35d0742b5fcd8e0057cd7380934025936cbf6f",
        aarch64: "4f6fe6269deadf99d2cd69b0a77cdaae1cab414738edf59df4ae79f4871c7adb",
    },
    Package {
        repository: "main",
        name: "libxkbcommon",
        version: "1.13.1-r0",
        x86_64: "f52efd26a9069b1c3edc89cab399f7f81bdc47c9e88a54e017c470bd9734b003",
        aarch64: "484370ec7cb91bd8035a7f947620885557daf0d4b10d8dbe50f207875828988e",
    },
    Package {
        repository: "main",
        name: "musl",
        version: "1.2.6-r3",
        x86_64: "e142255d0fa397f14318996bc10378093dcc069dc3dd1b6818c8f28a789c21da",
        aarch64: "2e17864102278d9049c3a56f1bceed9e8cf287065617e579325123443daed8a5",
    },
    Package {
        repository: "main",
        name: "nghttp2-libs",
        version: "1.70.0-r0",
        x86_64: "f0c4014eb2f10308dcdfd84cd49c121673c7b78cb8182b6e7a1388d8ae112a3b",
        aarch64: "7ef451f4dcdc065aa872c2f3c78b9d9bb4348caa5bcc70dee0c9e1fcf8e4c1fd",
    },
    Package {
        repository: "main",
        name: "pcre2",
        version: "10.49-r0",
        x86_64: "2919897914820f42a17a8d7d37df310d2dfad1c82ebe5f539cc5c765f916752a",
        aarch64: "03efd0b29f7f38deb10a9ccb54f56e19683eb7ac27eb1f347e6dbd2e59ea000d",
    },
    Package {
        repository: "community",
        name: "qt6-qtbase",
        version: "6.11.1-r0",
        x86_64: "58c938ec4f6599ae9377784d418a79b5e6e3318a237bb0284d5486465b5763a0",
        aarch64: "70027c3bd72b493fce0afcb211f9af7dab54bd685f1e9579d338d1e70f36456b",
    },
    Package {
        repository: "community",
        name: "qt6-qtbase-wayland",
        version: "6.11.1-r0",
        x86_64: "28525ac0b8d517deb956c77e050ddc20ee94025451f2c02bbf6cfd0264886245",
        aarch64: "3c526e76457ec03cd651d3722648c1a3f1507c55823c1838ad6a337122c6dfde",
    },
    Package {
        repository: "community",
        name: "qt6-qtbase-x11",
        version: "6.11.1-r0",
        x86_64: "b6b70f8f7ddf6b0b67318eb20c648d1344140f5d65b9333026e984571d443ee7",
        aarch64: "7fd03754af1a4c5a17eb5e0269bd9270908e3bffa49935a3542766a2522c2af7",
    },
    Package {
        repository: "main",
        name: "wayland-libs-client",
        version: "1.25.0-r0",
        x86_64: "a728f7103217cafdc83ff6ad0d679c197dbbe280bd5788c8336259f7a39cb303",
        aarch64: "409b4ad8de59d7b69f7c8add50cc76ef0fe3fe8c80e7854d6fe93c6b1dccc005",
    },
    Package {
        repository: "main",
        name: "wayland-libs-cursor",
        version: "1.25.0-r0",
        x86_64: "16ec369226507abaeb84b496ce2bb1a4ef29987ea91ba6bedc40bb0e0af274e7",
        aarch64: "22129b3f85ecf905a00436040c4ca3d07a071e700997aebf9bb7a2430cb42946",
    },
    Package {
        repository: "main",
        name: "zlib",
        version: "1.3.2-r1",
        x86_64: "63aeea03c15a2f9018f81805cfc8aa926bdf5cd68921f22149c2fbb5d0ee9f47",
        aarch64: "1f77a542ac6634761b9fe38d8bdca19b154071dcfa25432e73f2bc8a0740a33f",
    },
    Package {
        repository: "main",
        name: "zstd-libs",
        version: "1.5.7-r2",
        x86_64: "23c6065b0049b2406441564bcf0032515a43f78e80d76fcb85535a3803ef5d4e",
        aarch64: "2bb5136c89f5b0bbe1554c8915a3b520d5aa63ae2a51d4d821eb81698db5a818",
    },
];

/// The libraries the app carries on both architectures, by the names they are loaded by
/// (`DT_SONAME`); the unpacked packages hold each at `usr/lib/<name>`, most as a link the copy
/// follows, but for those in [`LIBRARIES_ELSEWHERE`].
pub const LIBRARIES: [&str; 54] = [
    "libQt6Core.so.6",
    "libQt6DBus.so.6",
    "libQt6Gui.so.6",
    "libQt6Network.so.6",
    "libQt6WaylandClient.so.6",
    "libQt6Widgets.so.6",
    "libQt6Xml.so.6",
    "libb2.so.1",
    "libblkid.so.1",
    "libbrotlicommon.so.1",
    "libbrotlidec.so.1",
    "libbz2.so.1",
    "libcares.so.2",
    "libcprime-core.so.5",
    "libcprime-widgets.so.5",
    "libcrypto.so.3",
    "libcurl.so.4",
    "libdbus-1.so.3",
    "libdouble-conversion.so.3",
    "libduktape.so.207",
    "libeconf.so.0",
    "libexpat.so.1",
    "libffi.so.8",
    "libfontconfig.so.1",
    "libfreetype.so.6",
    "libgcc_s.so.1",
    "libgio-2.0.so.0",
    "libglib-2.0.so.0",
    "libgmodule-2.0.so.0",
    "libgobject-2.0.so.0",
    "libgomp.so.1",
    "libgraphite2.so.3",
    "libharfbuzz.so.0",
    "libicudata.so.78",
    "libicui18n.so.78",
    "libicuuc.so.78",
    "libidn2.so.0",
    "libintl.so.8",
    "libmount.so.1",
    "libnghttp2.so.14",
    "libpcre2-16.so.0",
    "libpcre2-8.so.0",
    "libpng16.so.16",
    "libproxy.so.1",
    "libpsl.so.5",
    "libpxbackend-1.0.so",
    "libssl.so.3",
    "libstdc++.so.6",
    "libunistring.so.5",
    "libwayland-client.so.0",
    "libwayland-cursor.so.0",
    "libxkbcommon.so.0",
    "libz.so.1",
    "libzstd.so.1",
];

/// The libraries the app carries on x86_64 alone: Alpine builds Qt's X11 code only there, and
/// x86_64's `libQt6Gui` is linked against them (loaded, never used on Wayland).
pub const X86_64_LIBRARIES: [&str; 6] = [
    "libX11.so.6",
    "libXau.so.6",
    "libXdmcp.so.6",
    "libbsd.so.0",
    "libmd.so.0",
    "libxcb.so.1",
];

/// The libraries the unpacked packages hold somewhere other than `usr/lib/<name>`, with where.
pub const LIBRARIES_ELSEWHERE: [(&str, &str); 1] = [(
    "libpxbackend-1.0.so",
    "usr/lib/libproxy/libpxbackend-1.0.so",
)];

/// The Qt plugins the app carries, by their kind's folder and file name, at the same place
/// under the packages' `usr/lib/qt6/plugins` and the app's `plugins`: the Wayland platform,
/// its `xdg-shell` integration, the compose input context, and the client-side title bar Qt
/// falls back to when a compositor draws none (eggway draws them, so it stays unloaded).
pub const PLUGINS: [&str; 4] = [
    "platforminputcontexts/libcomposeplatforminputcontextplugin.so",
    "platforms/libqwayland.so",
    "wayland-decoration-client/libbradient.so",
    "wayland-shell-integration/libxdg-shell.so",
];

fn musl_loader(arch: &str) -> String {
    format!("ld-musl-{arch}.so.1")
}

/// Whether the app is checked on `arch`: Alpine builds CorePad for both.
pub fn runs_on(arch: &str) -> bool {
    matches!(arch, "x86_64" | "aarch64")
}

/// The libraries the app carries on `arch`, from the packages.
pub fn libraries(arch: &str) -> Vec<&'static str> {
    let mut libraries = LIBRARIES.to_vec();
    if arch == "x86_64" {
        libraries.extend(X86_64_LIBRARIES);
    }
    libraries
}

fn library_in_package(library: &str) -> PathBuf {
    match LIBRARIES_ELSEWHERE
        .iter()
        .find(|(name, _)| *name == library)
    {
        Some((_, path)) => PathBuf::from(path),
        None => Path::new(LIBRARIES_IN_PACKAGE).join(library),
    }
}

fn root() -> PathBuf {
    crate::target_directory().join("qt-app")
}

fn fetch_packages(arch: &Arch) -> Result<PathBuf, String> {
    let root = root();
    let downloads = root.join(format!("downloads-{}", arch.name));
    create(&downloads)?;
    let unpacked = root.join(format!("packages-{}", arch.name));
    fresh(&unpacked)?;
    for package in PACKAGES {
        let sha256 = package
            .sha256(arch.name)
            .ok_or_else(|| format!("{}: no pinned {}", arch.name, package.name))?;
        let file = downloads.join(package.file());
        fetch_pinned(&package.url(arch.name), &file, sha256)?;
        untar(&file, &unpacked)?;
    }
    Ok(unpacked)
}

fn folder(name: &str) -> Member {
    Member {
        name: format!("{name}/"),
        data: Vec::new(),
        mode: DIRECTORY_MODE,
    }
}

fn file_member(name: String, data: Vec<u8>) -> Member {
    Member {
        name,
        data,
        mode: FILE_MODE,
    }
}

/// The archive's members for `arch`, from the packages unpacked at `packages`: the editor,
/// musl's loader, the libraries with EggOS's own EGL and GL, the plugins, ICU's data and the
/// fonts.
pub fn members(arch: &str, packages: &Path) -> Result<Vec<Member>, String> {
    let machine = Machine::of(arch)?;
    let mut members = vec![
        Member {
            name: APP.to_owned(),
            data: read(&packages.join(PROGRAM_IN_PACKAGE))?,
            mode: PROGRAM_MODE,
        },
        folder(LIBRARY_FOLDER),
        Member {
            name: format!("{LIBRARY_FOLDER}/{}", musl_loader(arch)),
            data: read(&packages.join("lib").join(musl_loader(arch)))?,
            mode: PROGRAM_MODE,
        },
    ];
    for library in libraries(arch) {
        members.push(file_member(
            format!("{LIBRARY_FOLDER}/{library}"),
            read(&packages.join(library_in_package(library)))?,
        ));
    }
    for library in gl_libraries::LIBRARIES {
        members.push(file_member(
            format!("{LIBRARY_FOLDER}/{}", library.soname),
            gl_libraries::shared_object(machine, &library),
        ));
    }
    members.push(folder(PLUGIN_FOLDER));
    let mut kinds: Vec<&str> = PLUGINS
        .iter()
        .filter_map(|plugin| plugin.split_once('/').map(|(kind, _)| kind))
        .collect();
    kinds.dedup();
    for kind in kinds {
        members.push(folder(&format!("{PLUGIN_FOLDER}/{kind}")));
    }
    for plugin in PLUGINS {
        members.push(file_member(
            format!("{PLUGIN_FOLDER}/{plugin}"),
            read(&packages.join(PLUGINS_IN_PACKAGE).join(plugin))?,
        ));
    }
    members.extend([folder("share"), folder(ICU_FOLDER)]);
    members.push(file_member(
        format!("{ICU_FOLDER}/{ICU_DATA}"),
        read(&packages.join(ICU_DATA_IN_PACKAGE))?,
    ));
    members.push(folder(FONTS_FOLDER));
    for font in FONTS {
        members.push(file_member(
            format!("{FONTS_FOLDER}/{font}"),
            read(&packages.join(FONTS_IN_PACKAGE).join(font))?,
        ));
    }
    Ok(members)
}

/// What the app is installed with, as `app install --environment` takes it: Qt's Wayland
/// platform, and where Qt finds its plugins and ICU its data (the app's own folders). The
/// launch line, environment first, must stay within the catalogue's 256 bytes.
pub fn install_environment() -> String {
    format!(
        "QT_QPA_PLATFORM=wayland QT_PLUGIN_PATH=/Applications/{APP}/{PLUGIN_FOLDER} \
         ICU_DATA=/Applications/{APP}/{ICU_FOLDER}"
    )
}

/// What the app is installed with, as `app install --arguments` takes it: the note it edits.
pub fn install_arguments() -> String {
    NOTE.to_owned()
}

fn pack(arch: &Arch, packages: &Path) -> Result<PathBuf, String> {
    let folder = root().join(arch.name).join("host");
    fresh(&folder)?;
    let archive = zip::stored(&members(arch.name, packages)?)?;
    let path = folder.join(ARCHIVE);
    std::fs::write(&path, archive).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(folder)
}

/// Downloads the packages, packs `corepad.zip` into a host folder and runs the boot test
/// ([`crate::qemu::qt_app::check`]) on the release kernel.
pub fn check(arch: &Arch) -> Result<(), String> {
    if !runs_on(arch.name) {
        return Err(format!(
            "{}: the Qt app runs on x86_64 and aarch64, where Alpine builds CorePad",
            arch.name
        ));
    }
    let packages = fetch_packages(arch)?;
    let folder = pack(arch, &packages)?;
    let kernel = build_kernel(arch, None)?;
    crate::qemu::qt_app::check(arch, &kernel, &folder)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_package_is_pinned_on_both_architectures_once() {
        for (at, package) in PACKAGES.iter().enumerate() {
            for arch in ["x86_64", "aarch64"] {
                let sha256 = package.sha256(arch).unwrap();
                assert_eq!(sha256.len(), 64, "{}", package.name);
                assert!(sha256.bytes().all(|byte| byte.is_ascii_hexdigit()));
            }
            assert!(
                PACKAGES[at + 1..]
                    .iter()
                    .all(|other| other.name != package.name),
                "{} is pinned twice",
                package.name
            );
        }
        let corepad = PACKAGES.iter().find(|package| package.name == APP).unwrap();
        assert!(corepad.version.starts_with(COREPAD_VERSION));
        let qt = PACKAGES
            .iter()
            .find(|package| package.name == "qt6-qtbase")
            .unwrap();
        assert!(qt.version.starts_with(QT_VERSION));
        assert_eq!(
            corepad.url("x86_64"),
            "https://dl-cdn.alpinelinux.org/alpine/v3.24/community/x86_64/corepad-5.0.0-r0.apk"
        );
        assert!(LIBRARIES.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(X86_64_LIBRARIES.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(PLUGINS.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(
            X86_64_LIBRARIES
                .iter()
                .all(|library| !LIBRARIES.contains(library))
        );
        assert_eq!(libraries("x86_64").len(), 60);
        assert_eq!(libraries("aarch64").len(), 54);
        assert!(!runs_on("riscv64"));
    }

    #[test]
    fn the_archive_holds_the_editor_its_loader_libraries_plugins_data_and_fonts() {
        let scratch = std::env::temp_dir().join(format!("eggos-qt-app-{}", std::process::id()));
        let packages = scratch.join("packages");
        for folder in [
            packages.join("usr/bin"),
            packages.join("lib"),
            packages.join("usr/lib/libproxy"),
            packages.join("usr/share/icu/78.1"),
            packages.join(FONTS_IN_PACKAGE),
        ] {
            std::fs::create_dir_all(folder).unwrap();
        }
        for plugin in PLUGINS {
            let path = packages.join(PLUGINS_IN_PACKAGE).join(plugin);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, plugin).unwrap();
        }
        std::fs::write(packages.join(PROGRAM_IN_PACKAGE), b"\x7fELFeditor").unwrap();
        std::fs::write(packages.join("lib/ld-musl-x86_64.so.1"), b"loader").unwrap();
        for library in libraries("x86_64") {
            std::fs::write(packages.join(library_in_package(library)), library).unwrap();
        }
        std::fs::write(packages.join(ICU_DATA_IN_PACKAGE), b"icu").unwrap();
        for font in FONTS {
            std::fs::write(packages.join(FONTS_IN_PACKAGE).join(font), b"font").unwrap();
        }
        let members = members("x86_64", &packages).unwrap();
        std::fs::remove_dir_all(&scratch).unwrap();
        let names: Vec<&str> = members.iter().map(|member| member.name.as_str()).collect();
        assert_eq!(names[0], "corepad");
        for wanted in [
            "lib/ld-musl-x86_64.so.1",
            "lib/libQt6Gui.so.6",
            "lib/libX11.so.6",
            "lib/libpxbackend-1.0.so",
            "lib/libEGL.so.1",
            "lib/libGL.so.1",
            "lib/libGLESv2.so.2",
            "plugins/platforms/",
            "plugins/platforms/libqwayland.so",
            "plugins/wayland-shell-integration/libxdg-shell.so",
            "share/icu/icudt78l.dat",
            "share/fonts/DejaVuSansMono.ttf",
        ] {
            assert!(names.contains(&wanted), "{wanted}");
        }
        let egl = members
            .iter()
            .find(|member| member.name == "lib/libEGL.so.1")
            .unwrap();
        assert_eq!(&egl.data[0..4], b"\x7fELF");
        let programs = members
            .iter()
            .filter(|member| member.mode == PROGRAM_MODE)
            .count();
        assert_eq!(programs, 2);
        let folders = names.iter().filter(|name| name.ends_with('/')).count();
        assert_eq!(folders, 9);
        assert_eq!(
            install_environment(),
            "QT_QPA_PLATFORM=wayland QT_PLUGIN_PATH=/Applications/corepad/plugins \
             ICU_DATA=/Applications/corepad/share/icu"
        );
        assert_eq!(install_arguments(), "/tmp/note.txt");
        let launch_line = format!(
            "{} /Applications/{APP}/{LIBRARY_FOLDER}/{} --library-path \
             /Applications/{APP}/{LIBRARY_FOLDER} /Applications/{APP}/{APP} {}",
            install_environment(),
            musl_loader("aarch64"),
            install_arguments()
        );
        assert_eq!(launch_line.len(), 241);
        assert!(launch_line.len() <= eggos_drivers::desktop::COMMAND_CAPACITY);
        assert!(eggos_app_manifest::is_environment(
            install_environment().as_bytes()
        ));
        assert!(eggos_app_manifest::is_arguments(
            install_arguments().as_bytes()
        ));
    }
}

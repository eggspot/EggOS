//! `cargo xtask gtk-app` (D176, the Real apps track's second Linux app): a real GTK app,
//! Mousepad, installed and used inside EggOS through `eggway`.
//!
//! Everything here is test data, downloaded and never shipped or committed (decided by
//! research). Mousepad 0.7.0 (Xfce's text editor, GPL-2.0-or-later) and every library it
//! loads come from Alpine Linux `v3.24`'s packages, each checked against its pinned SHA-256:
//! GTK 3.24.52, which draws with cairo into `wl_shm` buffers and needs no GPU, GLib, Pango,
//! cairo, HarfBuzz, fontconfig, FreeType, GtkSourceView 4, the accessibility and X11
//! libraries GTK is linked against (loaded, never used on Wayland), Wayland's client
//! libraries, `libxkbcommon` and musl's loader. They are packed into `mousepad.zip`: the
//! editor at `mousepad`, GLib's `glib-compile-schemas` at `libexec/`, musl's loader and every
//! library at `lib/`, the GSettings schemas of Mousepad and GTK at `share/glib-2.0/schemas`,
//! GtkSourceView's languages and styles at `share/gtksourceview-4` and four DejaVu fonts at
//! `share/fonts`, which the installer links into `/Applications/.fonts`, where EggOS's
//! `/etc/fonts/fonts.conf` points fontconfig ([`crate::qemu::gtk_app`] boots it).

use crate::sdl_app::{Package, create, fetch_pinned, fresh, read, untar};
use crate::zip::{self, Member};
use crate::{Arch, build_kernel};
use std::path::{Path, PathBuf};

/// The app's name once installed, and its program's file name in the archive.
pub const APP: &str = "mousepad";
/// The archive the host folder holds.
pub const ARCHIVE: &str = "mousepad.zip";
/// Mousepad's version, as Alpine packages it.
pub const MOUSEPAD_VERSION: &str = "0.7.0";
/// GTK's version, as Alpine packages it.
pub const GTK_VERSION: &str = "3.24.52";
/// Where musl's loader and the libraries are inside the app's folder.
pub const LIBRARY_FOLDER: &str = "lib";
/// GLib's schema compiler inside the app's folder: the check compiles the schemas with it,
/// inside EggOS, as a packager would before shipping them.
pub const SCHEMA_COMPILER: &str = "libexec/glib-compile-schemas";
/// Where the GSettings schemas are inside the app's folder: under `share`, which the app's
/// `XDG_DATA_DIRS` names.
pub const SCHEMA_FOLDER: &str = "share/glib-2.0/schemas";
/// The fonts' folder inside the app's folder, the one the installer links.
pub const FONTS_FOLDER: &str = "share/fonts";
/// The file the check edits.
pub const NOTE: &str = "/tmp/note.txt";

const PROGRAM_IN_PACKAGE: &str = "usr/bin/mousepad";
const SCHEMA_COMPILER_IN_PACKAGE: &str = "usr/bin/glib-compile-schemas";
const SCHEMAS_IN_PACKAGE: &str = "usr/share/glib-2.0/schemas";
const SOURCE_VIEW_IN_PACKAGE: &str = "usr/share/gtksourceview-4";
const SOURCE_VIEW_FOLDER: &str = "share/gtksourceview-4";
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

/// The packages, pinned by research (D176): Mousepad and the closure of the libraries its
/// program and theirs name (`DT_NEEDED`), with DejaVu's fonts.
pub const PACKAGES: [Package; 57] = [
    Package {
        repository: "main",
        name: "at-spi2-core-libs",
        version: "2.60.5-r0",
        x86_64: "bcd4846311c8b19ad7e08ffc3cf726ddbe3d36bbc524386bda64758f5c5981a6",
        aarch64: "dc87dba0de9c3af16c13da4c627fafdada70e1c5e2c48bec7464954df7c7d1ee",
    },
    Package {
        repository: "main",
        name: "brotli-libs",
        version: "1.2.1-r0",
        x86_64: "62a7e37e43bbfc1f8525a00ad2125fd5897135dc93b849733884cb78ffd8e26c",
        aarch64: "f802b9d5a329c216f02e73607cb8f662c434767e41686367ac0afcf6d0d45d64",
    },
    Package {
        repository: "main",
        name: "cairo",
        version: "1.18.4-r1",
        x86_64: "23364d77506b5f60eae26daa0a5489cdee8bc100fcd010cd73774dfa5a7f9305",
        aarch64: "edb009c2331efe54a895468690c46d528ef2907025fc90ca9329950f41b87e61",
    },
    Package {
        repository: "main",
        name: "cairo-gobject",
        version: "1.18.4-r1",
        x86_64: "ec302c75a24edaa4a2f867cfb5eec85cf30efe184044bab6e2b9678840e2bc2a",
        aarch64: "fd13da5b7ed6c793dda3e602a7b27611ffdc9b4e03e3d0a64a0e9e85eb4b73e9",
    },
    Package {
        repository: "main",
        name: "dbus-libs",
        version: "1.16.2-r2",
        x86_64: "105f6ba17454c587ca9345f9deb348012d1950588a22da42120c954d396c99b6",
        aarch64: "b09b297d622377f870da5f162563b2bb4926dda17dfb2c7b1c84c37b05e8e295",
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
        name: "fribidi",
        version: "1.0.16-r3",
        x86_64: "be0cba50bc7bfdaadfba020fe8239ba3f25f35d309806a9e89e9e09860f985c0",
        aarch64: "b4f41c04682487ccc0d5e304a20f1062ddb47907ffa0a414208512e2c946e4d7",
    },
    Package {
        repository: "community",
        name: "gdk-pixbuf",
        version: "2.44.7-r1",
        x86_64: "71c422b35822af144fbc14baf7308fa6bd59c490b19842de93aef4ed46c88422",
        aarch64: "152fa4bb9045eb737c7cad912fb58dd9b91594e1f22f87fa79839d73d331e887",
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
        repository: "community",
        name: "gtk+3.0",
        version: "3.24.52-r0",
        x86_64: "c7c0c0faffa3061bc8fca9540b250d024bdba1aa6ae1fb06f5185adefceaa070",
        aarch64: "f228f53f8da557919a194ea4c5a14d67289a83cf67cade80f30f57d4a8244c69",
    },
    Package {
        repository: "community",
        name: "gtksourceview4",
        version: "4.8.4-r4",
        x86_64: "fbc1905d4a5f7cbb1e33723ae1e9aef194da0878fde1134ec7840eea9afe843c",
        aarch64: "328dabdcf33bacc4fb00bb695e833f3a0098180c1bf03f401efc6cb79fb496cf",
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
        name: "lcms2",
        version: "2.19-r0",
        x86_64: "b4cb038af3c2f02bedd8456ae876c97ac0a9f29789add77871bfd6ce084ff136",
        aarch64: "05c9e865ac443b8ff467c8e7c176ef323fac6c34f710410ef4f26c94478be7cd",
    },
    Package {
        repository: "main",
        name: "libatk-1.0",
        version: "2.60.5-r0",
        x86_64: "ab5f603b17f0f0c5577d259b8263c5cbc6c7b3fd031f06ddc1e3e26417e1609a",
        aarch64: "de8231e434a21012c8586e330f9bb26a85d40d700d9af571e23933c65349cb63",
    },
    Package {
        repository: "main",
        name: "libatk-bridge-2.0",
        version: "2.60.5-r0",
        x86_64: "f5e38bc54bae780680134f7b96ce8e01ef24e98ae0ac67ded7229af07aa0b0f1",
        aarch64: "82ff33781a127c1c974e2babb399cdaa5a08a222c9ce3ef6f49bd279c477ec45",
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
        repository: "main",
        name: "libeconf",
        version: "0.8.3-r0",
        x86_64: "2a791c7957961f830cf31b04f35f814a63afcf633badc9052425ff1cf2f64380",
        aarch64: "cf0899d49ded0a6891e3cdf1b37887f2ecd77293afe1d8f3cb8937763c83f183",
    },
    Package {
        repository: "main",
        name: "libepoxy",
        version: "1.5.10-r1",
        x86_64: "0be13b4b524fe64315e0663ab615a244d75d7da73130ac1c630cd6096841444e",
        aarch64: "ddb716957da6c47fbfc68e0b6a4ee87b1d734cc2bce4090d7e695e4aa0216417",
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
        repository: "community",
        name: "libglycin",
        version: "2.1.5-r0",
        x86_64: "3541d1a5d4d2599d145904f4febf26e56942403b6d80834461e196636b35b280",
        aarch64: "3e36d9028c787ab22995769e550d3e5a6d96f608332469aadb2f952dd9c66b9c",
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
        name: "libpng",
        version: "1.6.59-r0",
        x86_64: "d16aeb0597f2bc43e0583ac9ff779388ec160c3e22568b640122e94ba55be6d7",
        aarch64: "bccd127f8c42bc5c21c018b6b1dc977128f3bad64c3412e541c09e96e8b227a4",
    },
    Package {
        repository: "main",
        name: "libseccomp",
        version: "2.6.1-r0",
        x86_64: "b22ccfe8cb072e4c8d3bc17222eeda132e270ce6258c27622acb193bf27b48e1",
        aarch64: "9a2a2a5ff3b17ca394a484a5905bceaa57c7d4d392079f07d68a03e44cdcdcd6",
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
        name: "libxcomposite",
        version: "0.4.7-r0",
        x86_64: "90d6c22a5a730c7f884c992a8ca4c41102a71918aff9d804313356c721f2b50e",
        aarch64: "756c00c22167d020da4bcce1a67a0276e245cdf5f6ed3d5b488cbd076816f1e7",
    },
    Package {
        repository: "main",
        name: "libxcursor",
        version: "1.2.3-r0",
        x86_64: "5be10886e7361c8a9a6ae68b83ea664a6bd6a021043a9203fe6c8b878e99c0cd",
        aarch64: "3e1d5f0beb0d0c1222178951195fe6a896afda3efa315dd89101c00ebc04ada6",
    },
    Package {
        repository: "main",
        name: "libxdamage",
        version: "1.1.7-r0",
        x86_64: "d3fbef1ed735be3385531682c40a593d78467ff1713ad2c4a57daecb755b1bf7",
        aarch64: "15423918fc3968bd458931a6ae003d5155d68f850b04f893191e2c07d610476a",
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
        name: "libxext",
        version: "1.3.7-r0",
        x86_64: "8afe1ee6ab8cb656e12f20dc5b7fb2d4103faee4a38298b94f26ab52e597cbc8",
        aarch64: "06886b15075a537fcd499c243facd0ec8c3e38eddef19be68ef185c079c462e4",
    },
    Package {
        repository: "main",
        name: "libxfixes",
        version: "6.0.2-r0",
        x86_64: "cb00578d1b83abe8baf291f311f194d4deca0f9cf525f47ab10e3bd7c0e54e24",
        aarch64: "5d389f949f0f0e1753fcbfeeda8f06f12c9f14b13ec1cadb7f553cc8f11420a8",
    },
    Package {
        repository: "main",
        name: "libxi",
        version: "1.8.4-r0",
        x86_64: "e41ff864833603ab1065ac37680b47838436f78d4d9510604723341fcb933723",
        aarch64: "45fa9e7de44b246820922d99570d2e17bb2872d860b49db662ce5b8fdd31072d",
    },
    Package {
        repository: "main",
        name: "libxinerama",
        version: "1.1.6-r0",
        x86_64: "9f780efb3cf675b5253b73b84f278b40757fa7869c6fe70a9790a73f4f7e810f",
        aarch64: "96198e671dbfd58a54fb00d6e70ac05975a18c186e68c987aad72d4614ab4ff3",
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
        name: "libxml2",
        version: "2.13.9-r2",
        x86_64: "4b2f986159c659f014b942fe8a5d70c67af475153ca826c3dd53389eea46a300",
        aarch64: "0a860a26ebdfea491498386fe682130f4c5565e46ee8e0a2281256db2ef1be27",
    },
    Package {
        repository: "main",
        name: "libxrandr",
        version: "1.5.5-r0",
        x86_64: "c64856b77001cdf31a32841729b1950ad45e6981c16d09f9fdf08824ab1c0271",
        aarch64: "3d40a24141af8de45fb907864b18197aa1d66914c1102c3b0e11973a716ce232",
    },
    Package {
        repository: "main",
        name: "libxrender",
        version: "0.9.13-r0",
        x86_64: "724aa6c6353e70869e0eb5aa5461e8fc0753b99db765de47a3ae238573dbdcf8",
        aarch64: "02a9ec9c93b6c609f28af28f7a4db0111b8415e973f1a8e1eb31928a3a5a43ab",
    },
    Package {
        repository: "community",
        name: "mousepad",
        version: "0.7.0-r0",
        x86_64: "1d18dde5eeefb5087f50382304889f2c81e4cd094a65a6454283c34ff7b74453",
        aarch64: "88e02812711ed3382a31afabb9c25015029f793941553938f0db05ec701ee593",
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
        name: "pango",
        version: "1.57.1-r0",
        x86_64: "c616b62df908026ef413b338b9043e36706b05b6d035049394e5d14022e64adc",
        aarch64: "59a4be0b15a5dc3cf8bc425c65626a9f8a975a1d1beb5d3645fce34bf578c0a5",
    },
    Package {
        repository: "main",
        name: "pcre2",
        version: "10.49-r0",
        x86_64: "2919897914820f42a17a8d7d37df310d2dfad1c82ebe5f539cc5c765f916752a",
        aarch64: "03efd0b29f7f38deb10a9ccb54f56e19683eb7ac27eb1f347e6dbd2e59ea000d",
    },
    Package {
        repository: "main",
        name: "pixman",
        version: "0.46.4-r0",
        x86_64: "378550cbe348dda9b0733014375dba87a89f0077c5f3dbeba7dce8dbcdda707e",
        aarch64: "022ab7d2b766f3893e6c457a03c723fe3e79e9f009d8a62d06f1692d9f2fc77f",
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
        name: "wayland-libs-egl",
        version: "1.25.0-r0",
        x86_64: "c944a59c91ce89972b4cf0ecbb46dfcd43f95c4b86ee63b5522b9a843a3d6af9",
        aarch64: "c06c148aa8fe57848f1a30ac46b95736b04fb1bf73324ccf29c0bae82ee4a7e7",
    },
    Package {
        repository: "main",
        name: "xz-libs",
        version: "5.8.4-r0",
        x86_64: "ab362dd515ead04c31a6c302163248da7aa4813e283c971e6dd3f789b453a1d9",
        aarch64: "9be375832e747afe14f98e400e8b55d8f6fde25e41663090c81600628daab093",
    },
    Package {
        repository: "main",
        name: "zlib",
        version: "1.3.2-r1",
        x86_64: "63aeea03c15a2f9018f81805cfc8aa926bdf5cd68921f22149c2fbb5d0ee9f47",
        aarch64: "1f77a542ac6634761b9fe38d8bdca19b154071dcfa25432e73f2bc8a0740a33f",
    },
];

/// The libraries the app carries, by the names they are loaded by (`DT_SONAME`); the unpacked
/// packages hold each at `usr/lib/<name>`, most as a link the copy follows.
pub const LIBRARIES: [&str; 64] = [
    "libX11.so.6",
    "libXau.so.6",
    "libXcomposite.so.1",
    "libXcursor.so.1",
    "libXdamage.so.1",
    "libXdmcp.so.6",
    "libXext.so.6",
    "libXfixes.so.3",
    "libXi.so.6",
    "libXinerama.so.1",
    "libXrandr.so.2",
    "libXrender.so.1",
    "libatk-1.0.so.0",
    "libatk-bridge-2.0.so.0",
    "libatspi.so.0",
    "libblkid.so.1",
    "libbrotlicommon.so.1",
    "libbrotlidec.so.1",
    "libbsd.so.0",
    "libbz2.so.1",
    "libcairo-gobject.so.2",
    "libcairo.so.2",
    "libdbus-1.so.3",
    "libeconf.so.0",
    "libepoxy.so.0",
    "libexpat.so.1",
    "libffi.so.8",
    "libfontconfig.so.1",
    "libfreetype.so.6",
    "libfribidi.so.0",
    "libgcc_s.so.1",
    "libgdk-3.so.0",
    "libgdk_pixbuf-2.0.so.0",
    "libgio-2.0.so.0",
    "libglib-2.0.so.0",
    "libglycin-2.so.0",
    "libgmodule-2.0.so.0",
    "libgobject-2.0.so.0",
    "libgraphite2.so.3",
    "libgtk-3.so.0",
    "libgtksourceview-4.so.0",
    "libharfbuzz.so.0",
    "libintl.so.8",
    "liblcms2.so.2",
    "liblzma.so.5",
    "libmd.so.0",
    "libmount.so.1",
    "libmousepad.so.0",
    "libpango-1.0.so.0",
    "libpangocairo-1.0.so.0",
    "libpangoft2-1.0.so.0",
    "libpcre2-8.so.0",
    "libpixman-1.so.0",
    "libpng16.so.16",
    "libseccomp.so.2",
    "libwayland-client.so.0",
    "libwayland-cursor.so.0",
    "libwayland-egl.so.1",
    "libxcb-render.so.0",
    "libxcb-shm.so.0",
    "libxcb.so.1",
    "libxkbcommon.so.0",
    "libxml2.so.2",
    "libz.so.1",
];

fn musl_loader(arch: &str) -> String {
    format!("ld-musl-{arch}.so.1")
}

/// Whether the app is checked on `arch`: Alpine builds Mousepad for both.
pub fn runs_on(arch: &str) -> bool {
    matches!(arch, "x86_64" | "aarch64")
}

fn root() -> PathBuf {
    crate::target_directory().join("gtk-app")
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

/// The archive's members for `arch`, from the packages unpacked at `packages`: the editor,
/// the schema compiler, musl's loader, the libraries, the schemas, GtkSourceView's data and
/// the fonts.
pub fn members(arch: &str, packages: &Path) -> Result<Vec<Member>, String> {
    let mut members = vec![
        Member {
            name: APP.to_owned(),
            data: read(&packages.join(PROGRAM_IN_PACKAGE))?,
            mode: PROGRAM_MODE,
        },
        folder("libexec"),
        Member {
            name: SCHEMA_COMPILER.to_owned(),
            data: read(&packages.join(SCHEMA_COMPILER_IN_PACKAGE))?,
            mode: PROGRAM_MODE,
        },
        folder(LIBRARY_FOLDER),
        Member {
            name: format!("{LIBRARY_FOLDER}/{}", musl_loader(arch)),
            data: read(&packages.join("lib").join(musl_loader(arch)))?,
            mode: PROGRAM_MODE,
        },
    ];
    for library in LIBRARIES {
        members.push(Member {
            name: format!("{LIBRARY_FOLDER}/{library}"),
            data: read(&packages.join(LIBRARIES_IN_PACKAGE).join(library))?,
            mode: FILE_MODE,
        });
    }
    members.extend([folder("share"), folder("share/glib-2.0")]);
    members.extend(zip::folder_members(
        &packages.join(SCHEMAS_IN_PACKAGE),
        SCHEMA_FOLDER,
    )?);
    members.extend(zip::folder_members(
        &packages.join(SOURCE_VIEW_IN_PACKAGE),
        SOURCE_VIEW_FOLDER,
    )?);
    members.push(folder(FONTS_FOLDER));
    for font in FONTS {
        members.push(Member {
            name: format!("{FONTS_FOLDER}/{font}"),
            data: read(&packages.join(FONTS_IN_PACKAGE).join(font))?,
            mode: FILE_MODE,
        });
    }
    Ok(members)
}

/// What the app is installed with, as `app install --environment` takes it: where GLib finds
/// the schemas and GtkSourceView its data (the app's `share`), and no accessibility bus, as
/// EggOS has no D-Bus.
pub fn install_environment() -> String {
    format!("XDG_DATA_DIRS=/Applications/{APP}/share NO_AT_BRIDGE=1")
}

/// What the app is installed with, as `app install --arguments` takes it: no D-Bus service
/// for Mousepad's own windows, and the note it edits.
pub fn install_arguments() -> String {
    format!("--disable-server {NOTE}")
}

fn pack(arch: &Arch, packages: &Path) -> Result<PathBuf, String> {
    let folder = root().join(arch.name).join("host");
    fresh(&folder)?;
    let archive = zip::stored(&members(arch.name, packages)?)?;
    let path = folder.join(ARCHIVE);
    std::fs::write(&path, archive).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(folder)
}

/// Downloads the packages, packs `mousepad.zip` into a host folder and runs the boot test
/// ([`crate::qemu::gtk_app::check`]) on the release kernel.
pub fn check(arch: &Arch) -> Result<(), String> {
    if !runs_on(arch.name) {
        return Err(format!(
            "{}: the GTK app runs on x86_64 and aarch64, where Alpine builds Mousepad",
            arch.name
        ));
    }
    let packages = fetch_packages(arch)?;
    let folder = pack(arch, &packages)?;
    let kernel = build_kernel(arch, None)?;
    crate::qemu::gtk_app::check(arch, &kernel, &folder)
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
        let mousepad = PACKAGES.iter().find(|package| package.name == APP).unwrap();
        assert!(mousepad.version.starts_with(MOUSEPAD_VERSION));
        let gtk = PACKAGES
            .iter()
            .find(|package| package.name == "gtk+3.0")
            .unwrap();
        assert!(gtk.version.starts_with(GTK_VERSION));
        assert_eq!(
            mousepad.url("aarch64"),
            "https://dl-cdn.alpinelinux.org/alpine/v3.24/community/aarch64/mousepad-0.7.0-r0.apk"
        );
        assert!(LIBRARIES.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(!runs_on("riscv64"));
    }

    #[test]
    fn the_archive_holds_the_editor_its_loader_libraries_schemas_and_fonts() {
        let scratch = std::env::temp_dir().join(format!("eggos-gtk-app-{}", std::process::id()));
        let packages = scratch.join("packages");
        for folder in [
            packages.join("usr/bin"),
            packages.join("lib"),
            packages.join(LIBRARIES_IN_PACKAGE),
            packages.join(SCHEMAS_IN_PACKAGE),
            packages.join(SOURCE_VIEW_IN_PACKAGE).join("styles"),
            packages.join(FONTS_IN_PACKAGE),
        ] {
            std::fs::create_dir_all(folder).unwrap();
        }
        std::fs::write(packages.join(PROGRAM_IN_PACKAGE), b"\x7fELFeditor").unwrap();
        std::fs::write(packages.join(SCHEMA_COMPILER_IN_PACKAGE), b"\x7fELFtool").unwrap();
        std::fs::write(packages.join("lib/ld-musl-aarch64.so.1"), b"loader").unwrap();
        for library in LIBRARIES {
            std::fs::write(packages.join(LIBRARIES_IN_PACKAGE).join(library), library).unwrap();
        }
        std::fs::write(
            packages
                .join(SCHEMAS_IN_PACKAGE)
                .join("org.xfce.mousepad.gschema.xml"),
            b"<schemalist/>",
        )
        .unwrap();
        std::fs::write(
            packages
                .join(SOURCE_VIEW_IN_PACKAGE)
                .join("styles/classic.xml"),
            b"<style-scheme/>",
        )
        .unwrap();
        for font in FONTS {
            std::fs::write(packages.join(FONTS_IN_PACKAGE).join(font), b"font").unwrap();
        }
        let members = members("aarch64", &packages).unwrap();
        std::fs::remove_dir_all(&scratch).unwrap();
        let names: Vec<&str> = members.iter().map(|member| member.name.as_str()).collect();
        assert_eq!(names[0], "mousepad");
        for wanted in [
            "libexec/glib-compile-schemas",
            "lib/ld-musl-aarch64.so.1",
            "lib/libgtk-3.so.0",
            "lib/libmousepad.so.0",
            "share/glib-2.0/schemas/org.xfce.mousepad.gschema.xml",
            "share/gtksourceview-4/styles/classic.xml",
            "share/fonts/DejaVuSansMono.ttf",
        ] {
            assert!(names.contains(&wanted), "{wanted}");
        }
        let programs = members
            .iter()
            .filter(|member| member.mode == PROGRAM_MODE)
            .count();
        assert_eq!(programs, 3);
        assert_eq!(
            install_environment(),
            "XDG_DATA_DIRS=/Applications/mousepad/share NO_AT_BRIDGE=1"
        );
        assert_eq!(install_arguments(), "--disable-server /tmp/note.txt");
        assert!(eggos_app_manifest::is_environment(
            install_environment().as_bytes()
        ));
        assert!(eggos_app_manifest::is_arguments(
            install_arguments().as_bytes()
        ));
    }
}

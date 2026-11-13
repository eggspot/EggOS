# EggOS Text Editor: syntax colours (v0.1)

Design by Claude Fable (D16) for the Text Editor's syntax colouring, which `text-editor-v0.3.md`
section 16 left out. Everything in that brief stands; this one adds colour per glyph in the text
view and one item in the `View` menu. The text view is Egg Mono 8x16 on `field` `#0B0E13`, one
colour per glyph, no bold, no italics, no fill behind a token: a token is told by its colour
and by nothing else, so the colours must carry the whole job. The feel to match is gedit's and
Notepad++'s default: a calm page where structure, data and notes read at a glance, not a
rainbow. The palette has no red (the editor's rule since 17.4) and never uses `accent`, which
stays the focus ring's and the primary button's.

## 1. Seven colours, every value already in the desktop palette

The colours are `text`, `text.dim` and five of the desktop's bright ANSI colours
(`desktop-v0.3.md` section 2), so the toolkit gains no value and the terminal and the editor
agree on what green or blue look like. WCAG ratios computed from the values (the field is
`#0B0E13`, the selection `#2E4A6B`); `L` is relative luminance, the lightness the colour-blind
rule of section 3 relies on.

| Token kind | Colour | Hex | On field | On selection | L |
|---|---|---|---|---|---|
| Plain, Punctuation/Operator | `text` | `0x00E6EAF0` | 16.0:1 | 7.5:1 | .82 |
| Function, Macro/Preprocessor | bright yellow | `0x00F2CC6D` | 12.5:1 | 5.9:1 | .63 |
| Type, Key, Link | bright cyan | `0x0072D3E0` | 11.1:1 | 5.2:1 | .56 |
| String | bright green | `0x0084D48A` | 10.8:1 | 5.1:1 | .54 |
| Number, Escape, Attribute | bright magenta | `0x00DA95E4` | 8.7:1 | 4.1:1 | .42 |
| Keyword, Tag, Heading | bright blue | `0x007FB0F0` | 8.6:1 | 4.1:1 | .42 |
| Comment | `text.dim` | `0x008A94A6` | 6.3:1 | 3.0:1 | .29 |

Every kind clears 4.5:1 on the field, comments included. On the selection every kind clears
3:1 (the large-text and UI minimum), and plain text keeps its 7.5:1: selected text keeps its
token colours (the fill is the cue, as in every editor); the brief's "text in `text`" of section
4 now reads "text in its token colour". Comments are the one kind at the floor on the selection,
on purpose: `text.dim` is the toolkit's word for "secondary", which is what a comment is, and a
lighter grey would crowd the keyword and number tier. A glyph the font lacks (drawn `?`) keeps
its token's colour.

Why these groupings: blue is **structure** (the words that shape the file: keywords, a tag's
name, a heading, a `[section]` in TOML and INI, which is the same role as a heading); cyan is
**names the file defines or points at** (types, keys, links); green is **quoted data**; magenta
is **small literal things inside the structure** (numbers, an escape inside a string, an
attribute or decorator before an item); yellow is **a call or a directive** (a function name
before `(`, `println!`, `#include`, `@media`); grey is **not code**.

## 2. What stays plain

The screen stays calm because most glyphs are `text`: identifiers, variables, parameters,
fields, operators, brackets, punctuation, `$VAR` in shell, Markdown body text and its `*`/`_`/`` ` ``
marks, CSS selectors, INI and shell values, and numbers in shell. The rule: **a colour marks a
role the reader could not tell from the characters alone**. A bracket is a bracket; `=` is `=`;
a word the language does not define is a name, and names are the text. At most seven colours
ever appear, and a typical line shows three or four. A file whose language is not known shows
only `text`.

Language by extension (case-insensitive), else by a `#!` first line for shell and Python,
else none: `rs` Rust; `c h cc cpp cxx hpp` C/C++; `py` Python; `js mjs cjs jsx ts tsx`
JavaScript/TypeScript; `sh bash zsh` shell; `json` JSON; `toml` and `Cargo.lock` TOML; `yml
yaml` YAML; `md` Markdown; `html htm xml svg` HTML/XML; `css` CSS; `ini cfg conf` INI. Kinds a
language uses: Rust all nine code kinds; C/C++ the same without Attribute (`#include` is Macro,
`<stdio.h>` String); Python and JS/TS without Macro (decorators are Attribute; `int`, `str`,
`number` and capitalised names are Type); shell only Keyword, String, Comment and Function
(definitions `name()`); JSON Key, String, Number and Keyword (`true false null`); TOML and YAML
add Comment and Heading for `[table]`; INI Key, Heading, Comment; Markdown Heading and Link
only; HTML/XML Tag, Key (attribute names), String, Comment and Escape (`&amp;`); CSS Key
(property names), Number (with its unit), String, Comment, Keyword (at-rules), Function (`url(`).

## 3. Colour-blind safety

No kind depends on red against green: there is no red, and green (String) meets magenta,
blue and cyan, never red. The three kinds that share most lines, keyword, string and
comment, differ in lightness as well as hue (`L` .42, .54, .29: a 1.3x and 1.9x step), so they
still read apart in greyscale. Blue and magenta share a lightness, but a keyword is a word and
a number is digits, an escape sits inside green and an attribute starts with `#[` or `@`: the
characters tell them apart where the hue does not. Cyan and green also share a lightness; a
string is quoted and a type or key is not. Under tritanopia (blue-yellow) the yellow of calls
leans towards the plain white, which is the safe failure: a call read as plain text loses
nothing.

## 4. A few Rust lines, annotated

`K` Keyword, `T` Type, `S` String, `N` Number, `C` Comment, `F` Function, `M` Macro, `A`
Attribute, `E` Escape, `.` Plain:

```
#[derive(Debug)]
AAAAAAAAAAAAAAAA
pub fn area(r: &Rect) -> u32 {
KKK KK FFFF .. .TTTT. .. TTT .
    let w = r.width * 2;        // twice
    KKK . . ....... . N.        CCCCCCCC
    println!("w = {}\n", w);
    MMMMMMMM.SSSSSSSEES. ...
}
```

The whole `#[...]` is one Attribute; `println!` is the Macro and its `(` is plain; `{}` inside
the string stays String (no placeholder kind); `&`, `->`, `*`, `=` and `;` are plain.

## 5. The `View` menu: `Syntax Colours`, on by default

`View` gains a second item, `Syntax Colours`, under `Word Wrap`, checked while on; no key
(neither Notepad++, gedit nor VS Code binds one; it is not a thing one toggles while typing).
**On by default**: the user opened a `.rs` or `.toml` file and expects the editor to know what
it is, as gedit does; a `.txt` shows no colour anyway, so plain notes are unchanged. Off, every
glyph draws `text`. Like `Word Wrap` it is per window and not saved, until the editor keeps
settings (then `editor.syntax_colours`, same default). The status bar gains the language name
in `text.dim` ending 16 px before the state word (`Rust`, `Plain Text` when none), shown
whether colouring is on or off, so the user can see what was detected.

Hot path (the brief's "always the fastest"): the highlighter is a per-line scanner with a
one-byte state carried line to line (in a block comment, in a string); an edit re-scans from
its line until a line's end state matches the stored one, never the whole 4 MiB; it runs on
edit, load and toggle, never on paint.

## 6. Element tree, checks

`Syntax Colours` is a `menuitem` carrying `checked`. Host tests: the kinds per column of
section 4's lines; a block comment opened on line 1 greys line 2 and the edit closing it
restores line 2 in one re-scan; `name.RS` is Rust, `notes` with `#!/bin/sh` is shell, `notes.txt`
is none; the toggle off makes every glyph `text` and on restores the kinds; every value in the
table is at least 4.5:1 on `field` (a test computes the ratios). QEMU frame: a `main.rs`
opened from Files, with the `View` menu open showing `Syntax Colours` checked.

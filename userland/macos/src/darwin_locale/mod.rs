//! Darwin's locales (D190, slice 38b2) as pure logic with host tests: the names `setlocale`
//! accepts for each category and how six categories make one name (`settings`), the character
//! classes and case mappings `isalpha`'s family and `toupper` read (`ctype`), and the byte and
//! UTF-8 encodings of the multibyte functions (`multibyte`).

pub mod ctype;
pub mod multibyte;
pub mod settings;

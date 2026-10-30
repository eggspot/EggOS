use std::collections::BTreeMap;

pub fn counts(text: &str) -> BTreeMap<String, u32> {
    let mut counts = BTreeMap::new();
    for word in words(text) {
        *counts.entry(word).or_insert(0) += 1;
    }
    counts
}

pub fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
}

pub fn longest(text: &str) -> Option<String> {
    words(text).fold(None, |longest: Option<String>, word| match longest {
        Some(found) if found.len() >= word.len() => Some(found),
        _ => Some(word),
    })
}

pub fn title_case(text: &str) -> String {
    words(text)
        .map(|word| {
            let mut characters = word.chars();
            match characters.next() {
                Some(first) => first.to_uppercase().chain(characters).collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<String>>()
        .join(" ")
}

pub fn most_common(text: &str, wanted: usize) -> Vec<(String, u32)> {
    let mut ranked: Vec<(String, u32)> = counts(text).into_iter().collect();
    ranked.sort_by(|(first, first_count), (second, second_count)| {
        second_count
            .cmp(first_count)
            .then_with(|| first.cmp(second))
    });
    ranked.truncate(wanted);
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_are_counted_without_case_or_punctuation() {
        let counts = counts("The cat, the hat. THE end!");
        assert_eq!(counts.get("the"), Some(&3));
        assert_eq!(counts.get("cat"), Some(&1));
        assert_eq!(longest("a bb ccc dd"), Some("ccc".to_owned()));
        assert_eq!(title_case("hello big world"), "Hello Big World");
        assert_eq!(
            most_common("b a b c a b", 2),
            [("b".to_owned(), 3), ("a".to_owned(), 2)]
        );
    }
}

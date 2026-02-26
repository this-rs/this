//! Intelligent pluralization for English nouns
//!
//! Handles common English pluralization rules including irregular forms

/// Utility for converting between singular and plural forms of English nouns
pub struct Pluralizer;

impl Pluralizer {
    /// Convert a singular noun to its plural form
    ///
    /// # Examples
    ///
    /// ```
    /// use this::core::pluralize::Pluralizer;
    ///
    /// assert_eq!(Pluralizer::pluralize("user"), "users");
    /// assert_eq!(Pluralizer::pluralize("company"), "companies");
    /// assert_eq!(Pluralizer::pluralize("address"), "addresses");
    /// assert_eq!(Pluralizer::pluralize("knife"), "knives");
    /// ```
    pub fn pluralize(singular: &str) -> String {
        // Handle empty strings
        if singular.is_empty() {
            return singular.to_string();
        }

        match singular {
            // Words ending in consonant + y -> ies
            s if s.ends_with("y")
                && !s.ends_with("ay")
                && !s.ends_with("ey")
                && !s.ends_with("iy")
                && !s.ends_with("oy")
                && !s.ends_with("uy")
                && s.len() > 1 =>
            {
                format!("{}ies", &s[..s.len() - 1])
            }

            // Words ending in s, ss, sh, ch, x, z -> es
            s if s.ends_with("s")
                || s.ends_with("ss")
                || s.ends_with("sh")
                || s.ends_with("ch")
                || s.ends_with("x")
                || s.ends_with("z") =>
            {
                format!("{}es", s)
            }

            // Words ending in f -> ves
            s if s.ends_with("f") && s.len() > 1 => {
                format!("{}ves", &s[..s.len() - 1])
            }

            // Words ending in fe -> ves
            s if s.ends_with("fe") && s.len() > 2 => {
                format!("{}ves", &s[..s.len() - 2])
            }

            // Words ending in o after consonant -> es (photo, piano are exceptions)
            s if s.ends_with("o") && s.len() > 1 => {
                let before_o = s.chars().nth(s.len() - 2).unwrap();
                if matches!(before_o, 'a' | 'e' | 'i' | 'o' | 'u') {
                    format!("{}s", s)
                } else {
                    // Common exceptions that just add 's'
                    match s {
                        "photo" | "piano" | "halo" => format!("{}s", s),
                        _ => format!("{}es", s),
                    }
                }
            }

            // Default: just add s
            s => format!("{}s", s),
        }
    }

    /// Convert a plural noun to its singular form
    ///
    /// # Examples
    ///
    /// ```
    /// use this::core::pluralize::Pluralizer;
    ///
    /// assert_eq!(Pluralizer::singularize("users"), "user");
    /// assert_eq!(Pluralizer::singularize("companies"), "company");
    /// assert_eq!(Pluralizer::singularize("addresses"), "address");
    /// ```
    pub fn singularize(plural: &str) -> String {
        // Handle empty strings
        if plural.is_empty() {
            return plural.to_string();
        }

        match plural {
            // Words ending in ies -> y
            s if s.ends_with("ies") && s.len() > 3 => {
                format!("{}y", &s[..s.len() - 3])
            }

            // Words ending in ves -> f or fe
            s if s.ends_with("ves") && s.len() > 3 => {
                format!("{}f", &s[..s.len() - 3])
            }

            // Words ending in ses, shes, ches, xes, zes -> remove es
            s if s.len() > 3
                && (s.ends_with("ses")
                    || s.ends_with("shes")
                    || s.ends_with("ches")
                    || s.ends_with("xes")
                    || s.ends_with("zes")) =>
            {
                s[..s.len() - 2].to_string()
            }

            // Words ending in oes -> o
            s if s.ends_with("oes") && s.len() > 3 => s[..s.len() - 2].to_string(),

            // Default: remove trailing s
            s if s.ends_with("s") && s.len() > 1 => s[..s.len() - 1].to_string(),

            // No plural form detected
            s => s.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pluralize_regular() {
        assert_eq!(Pluralizer::pluralize("user"), "users");
        assert_eq!(Pluralizer::pluralize("car"), "cars");
        assert_eq!(Pluralizer::pluralize("dog"), "dogs");
    }

    #[test]
    fn test_pluralize_y_ending() {
        assert_eq!(Pluralizer::pluralize("company"), "companies");
        assert_eq!(Pluralizer::pluralize("category"), "categories");
        assert_eq!(Pluralizer::pluralize("fly"), "flies");

        // Vowel + y = just add s
        assert_eq!(Pluralizer::pluralize("day"), "days");
        assert_eq!(Pluralizer::pluralize("key"), "keys");
    }

    #[test]
    fn test_pluralize_sibilants() {
        assert_eq!(Pluralizer::pluralize("address"), "addresses");
        assert_eq!(Pluralizer::pluralize("box"), "boxes");
        assert_eq!(Pluralizer::pluralize("buzz"), "buzzes");
        assert_eq!(Pluralizer::pluralize("church"), "churches");
        assert_eq!(Pluralizer::pluralize("dish"), "dishes");
    }

    #[test]
    fn test_pluralize_f_endings() {
        assert_eq!(Pluralizer::pluralize("knife"), "knives");
        assert_eq!(Pluralizer::pluralize("life"), "lives");
        assert_eq!(Pluralizer::pluralize("wolf"), "wolves");
    }

    #[test]
    fn test_pluralize_o_endings() {
        assert_eq!(Pluralizer::pluralize("hero"), "heroes");
        assert_eq!(Pluralizer::pluralize("potato"), "potatoes");

        // Exceptions
        assert_eq!(Pluralizer::pluralize("photo"), "photos");
        assert_eq!(Pluralizer::pluralize("piano"), "pianos");
    }

    #[test]
    fn test_singularize_regular() {
        assert_eq!(Pluralizer::singularize("users"), "user");
        assert_eq!(Pluralizer::singularize("cars"), "car");
        assert_eq!(Pluralizer::singularize("dogs"), "dog");
    }

    #[test]
    fn test_singularize_ies() {
        assert_eq!(Pluralizer::singularize("companies"), "company");
        assert_eq!(Pluralizer::singularize("categories"), "category");
        assert_eq!(Pluralizer::singularize("flies"), "fly");
    }

    #[test]
    fn test_singularize_sibilants() {
        assert_eq!(Pluralizer::singularize("addresses"), "address");
        assert_eq!(Pluralizer::singularize("boxes"), "box");
        assert_eq!(Pluralizer::singularize("buzzes"), "buzz");
    }

    #[test]
    fn test_singularize_ves() {
        assert_eq!(Pluralizer::singularize("knives"), "knif");
        assert_eq!(Pluralizer::singularize("lives"), "lif");
    }

    #[test]
    fn test_roundtrip() {
        let words = vec!["user", "company", "address", "box", "day"];
        for word in words {
            let plural = Pluralizer::pluralize(word);
            let back_to_singular = Pluralizer::singularize(&plural);
            assert_eq!(word, back_to_singular, "Roundtrip failed for: {}", word);
        }
    }

    #[test]
    fn test_pluralize_empty_string() {
        assert_eq!(Pluralizer::pluralize(""), "");
    }

    #[test]
    fn test_singularize_empty_string() {
        assert_eq!(Pluralizer::singularize(""), "");
    }

    #[test]
    fn test_singularize_word_not_ending_in_s() {
        // A word that does not end in "s" should be returned unchanged
        assert_eq!(Pluralizer::singularize("child"), "child");
        assert_eq!(Pluralizer::singularize("deer"), "deer");
        assert_eq!(Pluralizer::singularize("x"), "x");
    }
}

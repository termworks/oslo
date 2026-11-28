//! Who a store's files are written for.
//!
//! A recipient is kept as the text it was written as and parsed when something is encrypted. That
//! is deliberate: `oslo secret where` can show a list this binary cannot use, which is the only way
//! somebody finds out that the store in front of them was written by a shell built differently.
//!
//! The published half of a key, and nothing else. A store that wants age's recipients — hardware
//! keys, a colleague who already has an age identity — hands its crypto to `age` itself with
//! `encrypt command`, and none of this is involved.
//!
//! **What was pasted may be a private key**, so an error names a recipient that is not one by its
//! prefix alone. See [`shown`] and [`Recipient::named`].

/// What a published half begins with.
const PUBLISHED: &str = "OSLO-PUB-1:";

/// What a private key begins with, oslo's and age's, and what each is.
const PRIVATE: &[(&str, &str)] = &[
    ("OSLO-KEY-1:", "a secret key"),
    ("OSLO-PROFILE-1:", "a profile key"),
    ("AGE-SECRET-KEY-", "an age identity"),
];

/// `text` as a message may show it: everything from a private key's prefix on is left out.
pub fn shown(text: &str) -> String {
    let upper = text.to_ascii_uppercase();
    let first = PRIVATE
        .iter()
        .filter_map(|(prefix, _)| upper.find(prefix).map(|at| (at, *prefix)))
        .min_by_key(|(at, _)| *at);
    match first {
        Some((at, prefix)) => format!("{}{prefix}…", &text[..at]),
        None => text.to_string(),
    }
}

/// One party a store's files are written for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipient(String);

impl Recipient {
    /// Take it as written, checking only what is cheap to check.
    pub fn new(text: &str) -> Result<Self, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("a recipient needs to be something".to_string());
        }
        let recipient = Recipient(text.to_string());
        if text.split_whitespace().count() > 1 {
            return Err(format!("{} is not one recipient", recipient.named()));
        }
        Ok(recipient)
    }

    /// As written.
    pub fn text(&self) -> &str {
        &self.0
    }

    /// The text as a message may show it: in full when it is a published half, and otherwise by
    /// its prefix alone, with what that prefix says it is.
    pub fn named(&self) -> String {
        let text = &self.0;
        if text.starts_with(PUBLISHED) && shown(text) == *text {
            return text.clone();
        }
        let upper = text.to_ascii_uppercase();
        if let Some((prefix, what)) = PRIVATE.iter().find(|(prefix, _)| upper.starts_with(prefix)) {
            return format!("{prefix}… ({what}, which is private: never paste it anywhere)");
        }
        match text.find(':') {
            Some(at) if at < 24 => format!("{}…", &text[..=at]),
            _ => format!("({} characters, not shown)", text.chars().count()),
        }
    }

    /// The public key this is, or why it is not one. A low-order point is refused.
    #[cfg(feature = "crypt")]
    pub fn public(&self) -> Result<[u8; 32], String> {
        super::native::read_public(&self.0).map_err(|e| format!("{}: {e}", self.named()))
    }
}

impl std::fmt::Display for Recipient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **No error repeats a private key**, whichever prefix it has and wherever it was pasted.
    #[test]
    fn a_private_key_is_named_by_its_prefix_alone() {
        let key = "OSLO-KEY-1:hqFoSGbhpF797BbBSjB+95yipkKjh2g2kEaXOFPlIYk=";
        let profile =
            "OSLO-PROFILE-1:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let age = "AGE-SECRET-KEY-1QQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQ";
        for text in [key, profile, age] {
            let body = &text[text.len() - 20..];
            let named = Recipient::new(text).expect("one word").named();
            assert!(!named.contains(body), "{named}");
            assert!(named.contains("private"), "{named}");
            assert!(!shown(&format!("recipient {text}")).contains(body));
        }
        let refused = Recipient::new(&format!("{key} extra")).expect_err("two words");
        assert!(!refused.contains(&key[20..]), "{refused}");
        // Something with no prefix at all is not shown either: it may be a bare key.
        let bare =
            Recipient::new("hqFoSGbhpF797BbBSjB+95yipkKjh2g2kEaXOFPlIYk=").expect("one word");
        assert!(!bare.named().contains("hqFo"), "{}", bare.named());
        // A published half is safe to repeat, and is.
        let public = "OSLO-PUB-1:zyzEW4hwVZdjajit/OwLB+o2647dl8YfoWY+mdCS+QM=";
        assert_eq!(Recipient::new(public).expect("one word").named(), public);
    }
}

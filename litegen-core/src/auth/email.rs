//! The address an account is made with: what signup and invitations take as
//! an email, and how it is stored (trimmed and lower-cased).
//!
//! The check is a pragmatic one, not RFC 5322: one `@`, a name before it, a
//! domain of two or more dot-separated labels after it, nothing a reader
//! can't see (no whitespace, control, Unicode format or default-ignorable
//! character), no code point Unicode hasn't assigned, no look-alike form
//! (written as NFKC writes it), and SMTP's lengths. An account's email is its
//! actor label in the audit log, where the master key's is `master-key` and
//! litegen's own is `system`; neither is an address, so no account can be
//! mistaken for them, nor for another account.
//!
//! Not detected: look-alikes from another script mixed into a label, such as
//! a Cyrillic `а` among Latin letters. litegen carries no Unicode script data
//! (no script crate is in its dependencies), so `аdmin@example.com` with a
//! Cyrillic first letter is a separate, valid address.

use unicode_general_category::{get_general_category, GeneralCategory};
use unicode_normalization::UnicodeNormalization;

/// The longest address SMTP carries (RFC 5321: a 256-octet path, less its
/// angle brackets), and the longest part before the `@`.
const MAX_LEN: usize = 254;
const MAX_LOCAL_LEN: usize = 64;

/// `raw` trimmed and lower-cased, as accounts store their email, when it is
/// an address. `Err` says what is wrong with it, for a 400 `invalid_email`.
pub fn account_email(raw: &str) -> Result<String, &'static str> {
    let email = raw.trim().to_lowercase();
    if email.chars().any(|c| c.is_whitespace() || c.is_control() || invisible(c)) {
        return Err("email must not hold spaces, control or invisible characters");
    }
    // A code point Unicode hasn't assigned shows as nothing or a box today,
    // and as whatever a later version makes of it.
    if email.chars().any(|c| get_general_category(c) == GeneralCategory::Unassigned) {
        return Err("email must not hold unassigned characters");
    }
    // A fullwidth, ligature or styled letter reads as its plain one: an
    // address NFKC would rewrite looks like another account's.
    if email.nfkc().ne(email.chars()) {
        return Err("email must not use look-alike letters (fullwidth, ligature or styled forms)");
    }
    let mut parts = email.split('@');
    let (Some(local), Some(domain), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err("email must have exactly one @");
    };
    if local.is_empty() {
        return Err("email must have a name before the @");
    }
    if domain.split('.').count() < 2 || domain.split('.').any(str::is_empty) {
        return Err("email must have a domain with a dot after the @, as in name@example.com");
    }
    if email.len() > MAX_LEN || local.len() > MAX_LOCAL_LEN {
        return Err("email must be at most 254 characters, with at most 64 before the @");
    }
    Ok(email)
}

/// What is wrong with `LITEGEN__OWNER_EMAIL` (`raw`, when set), for the
/// start-up check: only that address may claim the first account, and
/// signup takes only addresses, so one that isn't an address would block the
/// first signup for good. An empty value counts as unset.
pub fn owner_email_problem(raw: Option<&str>) -> Option<String> {
    let raw = raw.map(str::trim).filter(|s| !s.is_empty())?;
    account_email(raw)
        .err()
        .map(|why| format!("LITEGEN__OWNER_EMAIL is not an email address ({why}), so no signup could claim the first account"))
}

/// Unicode format characters (Cf), which show nothing or turn the text
/// around them, so an address holding one reads like another: the soft
/// hyphen, zero-width spaces and joiners, direction marks, embeddings,
/// overrides and isolates, invisible operators, tags, the byte-order mark.
/// And the rest of Unicode's Default_Ignorable_Code_Point, which renderers
/// show as nothing too (review round 3b, S5).
fn invisible(c: char) -> bool {
    get_general_category(c) == GeneralCategory::Format || DEFAULT_IGNORABLE.iter().any(|&(lo, hi)| (lo..=hi).contains(&(c as u32)))
}

/// Unicode's Default_Ignorable_Code_Point (DerivedCoreProperties.txt), as
/// inclusive ranges: besides format characters, the combining grapheme
/// joiner, Hangul fillers, Khmer inherent vowels, Mongolian free variation
/// selectors, variation selectors (FE00-FE0F, E0100-E01EF), and the code
/// points reserved for more of them.
const DEFAULT_IGNORABLE: &[(u32, u32)] = &[
    (0x00AD, 0x00AD),
    (0x034F, 0x034F),
    (0x061C, 0x061C),
    (0x115F, 0x1160),
    (0x17B4, 0x17B5),
    (0x180B, 0x180F),
    (0x200B, 0x200F),
    (0x202A, 0x202E),
    (0x2060, 0x206F),
    (0x3164, 0x3164),
    (0xFE00, 0xFE0F),
    (0xFEFF, 0xFEFF),
    (0xFFA0, 0xFFA0),
    (0xFFF0, 0xFFF8),
    (0x1BCA0, 0x1BCA3),
    (0x1D173, 0x1D17A),
    (0xE0000, 0xE0FFF),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Guards the normalization signup and invitations already did (trim,
    /// lower-case): a change that stopped doing either would let
    /// `Owner@Example.com` and `owner@example.com` become two accounts, since
    /// emails are unique as stored.
    #[test]
    fn an_account_email_is_stored_trimmed_and_lower_cased() {
        for (raw, stored) in [
            ("owner@example.com", "owner@example.com"),
            ("  Owner@Example.COM \t", "owner@example.com"),
            ("first.last+tag@mail.example.co.uk", "first.last+tag@mail.example.co.uk"),
            ("a@b.c", "a@b.c"),
            ("José@Bücher.de", "josé@bücher.de"),
        ] {
            assert_eq!(account_email(raw).as_deref(), Ok(stored), "{raw:?}");
        }
    }

    /// An account's email is shown as its actor in the audit log, so what
    /// isn't an address (`master-key` is the master key's label there) or
    /// hides characters (a look-alike of another account) is refused.
    #[test]
    fn what_is_not_an_address_is_refused() {
        let long_local = format!("{}@example.com", "a".repeat(65));
        let long_whole = format!("{}@{}.com", "a".repeat(64), "b".repeat(186));
        for (raw, why) in [
            ("master-key", "email must have exactly one @"),
            ("system", "email must have exactly one @"),
            ("", "email must have exactly one @"),
            ("two@at@example.com", "email must have exactly one @"),
            ("@example.com", "email must have a name before the @"),
            ("owner@", "email must have a domain with a dot after the @, as in name@example.com"),
            ("owner@localhost", "email must have a domain with a dot after the @, as in name@example.com"),
            ("owner@.example.com", "email must have a domain with a dot after the @, as in name@example.com"),
            ("owner@example..com", "email must have a domain with a dot after the @, as in name@example.com"),
            ("owner@example.com.", "email must have a domain with a dot after the @, as in name@example.com"),
            ("first last@example.com", "email must not hold spaces, control or invisible characters"),
            ("owner@exa\nmple.com", "email must not hold spaces, control or invisible characters"),
            ("owner\u{0}@example.com", "email must not hold spaces, control or invisible characters"),
            ("owner\u{200b}@example.com", "email must not hold spaces, control or invisible characters"),
            ("owner@example\u{202e}moc.com", "email must not hold spaces, control or invisible characters"),
            ("\u{feff}owner@example.com", "email must not hold spaces, control or invisible characters"),
            (long_local.as_str(), "email must be at most 254 characters, with at most 64 before the @"),
            (long_whole.as_str(), "email must be at most 254 characters, with at most 64 before the @"),
        ] {
            assert_eq!(account_email(raw), Err(why), "{raw:?}");
        }
        // The longest accepted: 64 before the @, 254 in all.
        let longest = format!("{}@{}.com", "a".repeat(64), "b".repeat(185));
        assert_eq!(longest.len(), 254);
        assert_eq!(account_email(&longest).as_deref(), Ok(longest.as_str()));
    }

    /// Review A8: every Unicode format character (Cf: joiners, marks, tags,
    /// annotation anchors) is refused, not only the few listed before, and
    /// so is an address NFKC normalization would change: a fullwidth,
    /// ligature or styled look-alike of another address.
    #[test]
    fn format_characters_and_look_alike_forms_are_refused() {
        for raw in ["owner\u{0600}@example.com", "owner\u{180e}@example.com", "owner\u{e0041}@example.com", "\u{fff9}owner@example.com", "owner@exam\u{110bd}ple.com"] {
            assert_eq!(account_email(raw), Err("email must not hold spaces, control or invisible characters"), "{raw:?}");
        }
        for raw in ["\u{ff4f}wner@example.com", "owner@ex\u{fb01}rm.com", "owner\u{00b2}@example.com", "owner@example.co\u{2122}"] {
            assert_eq!(account_email(raw), Err("email must not use look-alike letters (fullwidth, ligature or styled forms)"), "{raw:?}");
        }
        assert_eq!(account_email("José@Bücher.de").as_deref(), Ok("josé@bücher.de"), "accented letters NFKC keeps still pass");
    }

    /// Review round 3b, S5: what shows nothing but isn't a format character
    /// (Unicode's Default_Ignorable_Code_Point: variation selectors, the
    /// combining grapheme joiner, Hangul fillers, Khmer inherent vowels,
    /// Mongolian free variation selectors) is refused too, and so is a code
    /// point Unicode hasn't assigned: `owner\u{FE0F}@example.com` read as
    /// owner@example.com and was another account.
    #[test]
    fn default_ignorable_and_unassigned_code_points_are_refused() {
        for c in ['\u{FE0F}', '\u{FE00}', '\u{E0100}', '\u{E01EF}', '\u{034F}', '\u{115F}', '\u{1160}', '\u{3164}', '\u{FFA0}', '\u{17B4}', '\u{180B}', '\u{180F}'] {
            let raw = format!("owner{c}@example.com");
            assert_eq!(account_email(&raw), Err("email must not hold spaces, control or invisible characters"), "U+{:04X}", c as u32);
        }
        for c in ['\u{0378}', '\u{0379}'] {
            let raw = format!("owner{c}@example.com");
            assert_eq!(account_email(&raw), Err("email must not hold unassigned characters"), "U+{:04X}", c as u32);
        }
    }

    /// Follow-up item 3: an owner email that isn't an address is named, with
    /// the setting and what's wrong; unset, empty and real addresses pass.
    #[test]
    fn an_owner_email_that_is_not_an_address_is_named() {
        for fine in [None, Some(""), Some("  "), Some(" Owner@Example.com ")] {
            assert_eq!(owner_email_problem(fine), None, "{fine:?}");
        }
        let problem = owner_email_problem(Some("admin")).expect("named");
        assert!(problem.contains("LITEGEN__OWNER_EMAIL") && problem.contains("email must have exactly one @"), "{problem}");
    }
}

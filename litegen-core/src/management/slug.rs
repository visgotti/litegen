//! Shared slug helpers used by tenant-creation paths.

use crate::db::DatabaseStore;

/// Lowercase, replace non-alphanumeric runs with '-', trim leading/trailing '-'.
///
/// NOTE (Phase-1 TOCTOU): slug selection is check-then-insert; under concurrent
/// org creation two requests could pick the same candidate and the second will
/// hit the DB UNIQUE constraint as a 500. Acceptable for Phase 1 — a
/// transactional unique-slug helper is a Phase-2 follow-up.
pub fn slugify(s: &str) -> String {
    let slug: String = s
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "org".to_string()
    } else {
        slug
    }
}

/// Append `-2`, `-3`, … to `base` until a globally-free org slug is found.
///
/// NOTE (Phase-1 TOCTOU): same check-then-insert caveat as `slugify` — rare
/// concurrent collisions may surface as a 500 on the UNIQUE constraint.
pub async fn unique_org_slug(db: &dyn DatabaseStore, base: &str) -> Result<String, sqlx::Error> {
    if db.get_org_by_slug(base).await?.is_none() {
        return Ok(base.to_string());
    }
    for n in 2..10000 {
        let cand = format!("{base}-{n}");
        if db.get_org_by_slug(&cand).await?.is_none() {
            return Ok(cand);
        }
    }
    Ok(format!("{base}-{}", uuid::Uuid::new_v4()))
}

/// Derive a default org name from the local-part of an email address.
pub fn default_org_name_from_email(email: &str) -> String {
    email.split('@').next().unwrap_or("My Org").to_string()
}

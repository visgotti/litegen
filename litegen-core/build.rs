//! Rebuilds the crate when a migration is added or edited. `sqlx::migrate!`
//! embeds `migrations/` at compile time, but on stable Rust cargo can't tell
//! that the macro read those files. Without this script, a new or edited
//! migration isn't embedded until some `.rs` file changes, and the server
//! and tests run with the old set.
fn main() {
    println!("cargo:rerun-if-changed=migrations");
}

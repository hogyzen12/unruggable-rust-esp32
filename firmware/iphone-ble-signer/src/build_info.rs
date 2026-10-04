//! The release packager supplies a source-derived identity; ad-hoc builds say so.
pub const BUILD_ID: &str = env!("SIGNER_BUILD_ID");

// Override ESP-IDF's cached C project descriptor with this Rust package's version.
esp_idf_sys::esp_app_desc!(true);

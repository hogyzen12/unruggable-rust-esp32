fn main() {
    println!("cargo:rerun-if-env-changed=SIGNER_BOARD_PROFILE");
    println!("cargo:rerun-if-env-changed=SIGNER_BUILD_ID");
    let build_id = std::env::var("SIGNER_BUILD_ID").unwrap_or_else(|_| "development".into());
    assert!(
        !build_id.is_empty()
            && build_id.len() <= 64
            && build_id.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)),
        "SIGNER_BUILD_ID must contain 1–64 letters, digits, dots, underscores or hyphens"
    );
    println!("cargo:rustc-env=SIGNER_BUILD_ID={build_id}");
    // The SDK's descriptor macro evaluates these cfgs in the caller crate.
    for name in ["esp_idf_app_compile_time_date", "esp_idf_app_reproducible_build"] {
        println!("cargo:rustc-check-cfg=cfg({name})");
    }
    for name in ["esp_idf_version_major", "esp_idf_version", "esp_idf_version_full"] {
        println!("cargo:rustc-check-cfg=cfg({name}, values(any()))");
    }
    embuild::espidf::sysenv::output();
}

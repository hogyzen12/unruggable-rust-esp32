fn main() {
    println!("cargo:rerun-if-env-changed=SIGNER_BOARD_PROFILE");
    embuild::espidf::sysenv::output();
}

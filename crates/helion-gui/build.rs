fn main() {
    println!("cargo:rerun-if-env-changed=TARGET");
    println!(
        "cargo:rustc-env=HELION_TARGET={}",
        std::env::var("TARGET").unwrap_or_else(|_| "unknown".into())
    );
}

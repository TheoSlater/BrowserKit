fn main() {
    // cef-rs places libcef.so beside the executable in target/debug. Keep the
    // example runnable with the documented plain `cargo run` command without
    // requiring developers to export LD_LIBRARY_PATH manually.
    #[cfg(target_os = "linux")]
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
}

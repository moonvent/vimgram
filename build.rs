// tdlib-rs sets an rpath for libtdjson only on its own targets, so the final
// binary has to add it itself.
fn main() {
    println!("cargo:rerun-if-env-changed=LOCAL_TDLIB_PATH");
    if let Ok(tdlib) = std::env::var("LOCAL_TDLIB_PATH") {
        println!("cargo:rustc-link-arg-bins=-Wl,-rpath,{tdlib}/lib");
    }
}

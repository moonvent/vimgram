// tdlib-rs sets an rpath for libtdjson only on its own targets, so the final
// binary has to add it itself.
// On Windows the DLLs are found next to the executable instead.
fn main() {
    println!("cargo:rerun-if-env-changed=LOCAL_TDLIB_PATH");
    let rpath = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "linux" || os == "macos");
    if let Ok(tdlib) = std::env::var("LOCAL_TDLIB_PATH")
        && rpath
    {
        println!("cargo:rustc-link-arg-bins=-Wl,-rpath,{tdlib}/lib");
    }
}

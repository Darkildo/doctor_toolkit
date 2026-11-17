//! Link arguments of the DLL.
fn main() {
    // DllMain is stdcall (`_DllMain@12`); the MinGW start-up code refers to
    // `_DllMain`, which the linker resolves to it.
    println!("cargo:rustc-cdylib-link-arg=-Wl,--enable-stdcall-fixup");
    // The game imports `DirectInput8Create` by that very name: the stdcall
    // exports lose their `@20`.
    println!("cargo:rustc-cdylib-link-arg=-Wl,--kill-at");
}

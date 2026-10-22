// Embeds the setup UI when it has been built (`deno task build` in ui/, which
// `neslina bundle` runs). Without it, the server still builds and says so.
fn main() {
    let ui = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui/build");
    println!("cargo::rustc-check-cfg=cfg(ui_built)");
    println!("cargo::rerun-if-changed={}", ui.display());
    if ui.join("index.html").exists() {
        println!("cargo::rustc-cfg=ui_built");
    }
}

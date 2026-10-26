// Grant each window only the commands its capability lists (Tauri allows all otherwise).
include!("src/command_list.rs");
macro_rules! names {
    ($($command:ident),* $(,)?) => {
        &[$(stringify!($command)),*]
    };
}
const COMMANDS: &[&str] = for_each_command!(names);

fn main() {
    println!("cargo:rerun-if-changed=src/command_list.rs");
    let attributes = tauri_build::Attributes::new()
        .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS));
    // A build script reports failure by exiting non-zero.
    if let Err(error) = tauri_build::try_build(attributes) {
        eprintln!("tauri build: {error:#}");
        std::process::exit(1);
    }
}

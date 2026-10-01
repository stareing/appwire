const COMMANDS: &[&str] = &["op"];

fn main() {
    tauri_plugin::Builder::new(COMMANDS).build();
}

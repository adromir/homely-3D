//! Homely 3D desktop application entrypoint (Bevy + egui_dock).

use homely_core::Project;

fn main() {
    let project = Project {
        id: "default".to_string(),
        name: "Untitled Project".to_string(),
        version: "0.1.0".to_string(),
        ..Default::default()
    };
    println!("Homely 3D initialized: {}", project.name);
}

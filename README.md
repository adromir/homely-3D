# Homely 3D

[![Status](https://img.shields.io/badge/Status-Early_Development-blue)](https://github.com/adromir/homely-3D)
[![Language](https://img.shields.io/badge/Language-Rust-orange?logo=rust)](https://www.rust-lang.org/)
[![Engine](https://img.shields.io/badge/Engine-Bevy-lightgrey)](https://bevyengine.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-brightgreen.svg)](LICENSE)

A native, high-performance, open-source 2D/3D room and building planner written in **100% Rust**.

---

## ✨ What Homely 3D Does

Homely 3D lets you design floorplans in 2D and explore them synchronously in 3D without external runtime dependencies (no JVM, no WebView). It combines precision CAD geometry (poly-Bezier walls, parametric openings, multi-level buildings) with hardware-accelerated PBR/raytraced rendering, local/cloud AI asset generation, Lua scripting, MCP automation, and Home Assistant integration.

## 📖 Documentation

Full user guide, controls reference, and architecture docs are available in [`docs/index.md`](docs/index.md) (ready for GitHub Pages via `/docs` on `main`).

## 🚀 Why Use Homely 3D?

- **⚡ Native Performance:** Compiled directly to native machine code using Rust and the Bevy ECS engine—fast startup, low memory footprint, and smooth 60+ FPS viewports.
- **🧱 Precision Poly-Bezier Architecture:** Model straight and curved walls seamlessly in a single continuous path with non-destructive undo/redo history.
- **🏠 Smart-Home Ready:** Built-in Home Assistant entity mapping and glTF/Three.js export with `KHR_lights_punctual` and metadata `extras`.
- **🤖 AI & Automation Native:** Supports Model Context Protocol (MCP) for live AI-assisted floorplan editing alongside local (`diffusion.cpp` / `trellis.cpp`) or ComfyUI asset generation.
- **🧩 Modular & Extensible:** Clean Cargo workspace (`homely-core`, `homely-app`, `homely-mcp`) plus sandboxed Lua plugins (`mlua`).

---

## 📦 Workspace Structure

```text
homely-3D/
├── Cargo.toml            # Workspace manifest
├── LICENSE               # MIT License
└── crates/
    ├── homely-core/      # Headless data model, SQLite persistence, undo/redo, 2D geometry
    ├── homely-app/       # Bevy desktop application (2D/3D viewports & egui_dock UI)
    └── homely-mcp/       # MCP stdio server & IPC bridge to homely-app
```

---

## 🛠️ Installation

### Prerequisites

- [Rust toolchain](https://rustup.rs/) (`rustc` & `cargo` 1.80+ recommended)

### Build from Source

```bash
git clone https://github.com/adromir/homely-3D.git
cd homely-3D
cargo build --workspace
```

---

## 💻 Usage

### Run Tests

```bash
cargo test --workspace
```

### Run the Desktop App (Early Placeholder)

```bash
cargo run -p homely-app
```

---

## 📖 Examples

### Using `homely-core` Programmatically

```rust
use homely_core::command::{CommandHistory, ProjectCommand};
use homely_core::geometry::{tessellate_wall_centerline, wall_footprint_polygon};
use homely_core::{Level, PointType, Project, Wall, WallAnchorPoint};

let mut project = Project::default();
let mut history = CommandHistory::default();

history.execute(&mut project, ProjectCommand::AddLevel(Level {
    id: "lvl-0".into(),
    name: "Ground Floor".into(),
    elevation: 0.0,
    height: 2.6,
    floor_slab_thickness: 0.2,
    sort_order: 0,
})).unwrap();

let wall = Wall {
    id: "wall-1".into(),
    level_id: "lvl-0".into(),
    is_closed_loop: false,
    thickness: 0.24,
    start_height: 2.6,
    end_height: 2.6,
    anchors: vec![
        WallAnchorPoint {
            id: "a1".into(),
            pos_x: 0.0,
            pos_y: 0.0,
            handle_in_x: None,
            handle_in_y: None,
            handle_out_x: None,
            handle_out_y: None,
            point_type: PointType::Corner,
        },
        WallAnchorPoint {
            id: "a2".into(),
            pos_x: 5.0,
            pos_y: 0.0,
            handle_in_x: None,
            handle_in_y: None,
            handle_out_x: None,
            handle_out_y: None,
            point_type: PointType::Corner,
        },
    ],
};

history.execute(&mut project, ProjectCommand::AddWall(wall.clone())).unwrap();
let centerline = tessellate_wall_centerline(&wall, 0.01);
let footprint = wall_footprint_polygon(&centerline, wall.thickness);
assert_eq!(footprint.len(), 4);
```

---

## ⚠️ Disclaimer

Homely 3D is currently in **early development**. APIs, database schemas, and file formats may change before the first stable release. Use at your own risk.

---

## 📄 License

This project is licensed under the **MIT License** — see the [LICENSE](LICENSE) file for details.

---

## 👤 Author

**Adromir**  
Website: <https://github.com/adromir>

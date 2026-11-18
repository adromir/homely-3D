# Homely 3D Documentation

<p align="center">
  <img src="assets/logo.png" alt="Homely 3D Logo" width="112" />
</p>

**Homely 3D** is a native, open-source 2D/3D room and building planner written in **100% Rust** using the **Bevy Engine** and **egui**.

---

## Documentation Index

- **[User Guide & Controls](user-guide.md)** — How to draw walls, close rooms, place doors & windows, move buildings, and manage projects.
- **[Architecture & Workspace](architecture.md)** — Crate structure (`homely-core`, `homely-viewer3d`, `homely-app`, `homely-mcp`), coordinate conventions, and persistence format.

---

## Key Features (Implemented)

- **Dual-Window Workflow:** Synchronized **2D Planner** and dedicated **3D Viewer** OS windows.
- **2-Point Wall Modeling & Bezier Curves:** Walls are modeled as 2-point segments with shared corners, mitered 2D/3D joints at any angle, and optional cubic Bezier curve handles.
- **Automatic Room Detection:** Enclosed spaces are detected automatically via planar-graph cycle traversal, displaying live area (`m²`) in 2D and floor slabs in 3D.
- **Doors & Windows Catalog:** Drag & drop doors and windows directly onto walls with automatic 2D architectural symbols, configurable swing direction, height from floor, and real 3D wall cutouts.
- **Whole-Building Move:** Move an entire connected building (walls, doors, windows, and interior furniture) independently on the same plan.
- **Scene Tree & History:** Photoshop-style **History** tab with jump-to-state, plus a **Scene Tree** grouped by detected rooms (`Walls`, `Openings`, `Furniture`) with custom naming for every object.
- **Native `.h3d` Persistence & `.sh3x` Import:** SQLite-backed ZIP project archives with native OS file dialogs.
- **Bilingual UI:** Instant switching between **English** and **German**.

---

## Quick Start

```bash
git clone https://github.com/adromir/homely-3D.git
cd homely-3D
cargo run -p homely-app
```

Run the workspace test suite:

```bash
cargo test --workspace
```

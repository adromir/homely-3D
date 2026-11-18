# Architecture & Workspace

Homely 3D is structured as a modular **Cargo Workspace** so core CAD logic, 3D rendering, desktop UI, and external integrations stay cleanly decoupled.

---

## Workspace Crates

| Crate | Purpose | Key Dependencies |
| :--- | :--- | :--- |
| `homely-core` | Headless data model, command history (Undo/Redo), SQLite & `.h3d`/`.sh3x` persistence, 2D poly-Bezier tessellation, corner mitering, and planar-graph room detection. | `rusqlite`, `serde`, `zip`, `thiserror` |
| `homely-viewer3d` | Dedicated Bevy 3D viewer plugin (`Viewer3dPlugin`), spawning the 3D window, orbit camera, room floor slabs, and mitered 3D wall meshes with door/window cutouts. | `bevy`, `homely-core` |
| `homely-app` | Desktop launcher combining the `egui_dock` 2D planner UI, native dialogs, and `Viewer3dPlugin`. | `bevy`, `bevy_egui`, `egui_dock`, `rfd`, `image`, `winit` |
| `homely-mcp` | Model Context Protocol (`stdio`) server crate (in preparation). | `homely-core` |

---

## Coordinate System & Units

- **Scale:** `1.0` unit = `1.0` meter.
- **2D Floorplan:** Top-left `(0.0, 0.0)` origin, non-negative coordinates `[0..plan_width] × [0..plan_height]`.
- **3D Scene (Bevy / glTF Y-up):** `X` = width, `Y` = height above level floor, `Z` = depth.
- **Angles:** Radians.

---

## Core Design Principles

1. **Runtime Single Source of Truth:** The in-memory `Project` struct + `CommandHistory` in `homely-core` drive both 2D and 3D views deterministically; SQLite (WAL mode) inside `.h3d` ZIP archives serves as the persistence layer.
2. **2-Point Walls & Planar Graph:** Every wall is a 2-point segment (`A -> B`) with optional Bezier handles. Rooms are detected dynamically via a half-edge minimal-cycle walk (`homely_core::rooms::detect_rooms`), and buildings are grouped by connected wall components (`collect_connected_building_walls`).
3. **Mitered Corners & Non-Boolean Cutouts:** Wall offsets (`compute_level_wall_offsets`) miter shared endpoints cleanly in both 2D and 3D, and door/window cutouts slice wall polylines into sill/lintel spans without fragile 3D boolean CSG failures.

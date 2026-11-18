# User Guide & Controls

This guide covers all interactive tools, keyboard modifiers, and panels in **Homely 3D**.

---

## 1. 2D Floorplan Controls

The 2D floorplan uses a **top-left `(0, 0)` origin** in meters with a **1 m major grid** and **10 cm minor grid** (plus 10 cm snapping).

| Action | Control | Description |
| :--- | :--- | :--- |
| **Select / Move Point** | `Left-click` / `Drag` | Select a wall, anchor point, or curve handle and drag it. Shared room corners move all connected walls together. |
| **Start Building Walls** | `Ctrl + Left-click` (empty or point) | Enters building mode from that coordinate. Subsequent points are placed with normal `Left-click` (no need to hold `Ctrl`). |
| **Dock / Close Room** | `Left-click` on existing point (in building mode) | Snaps cleanly to the target corner and finishes building mode automatically. |
| **Stop Building** | `Right-click` or `Esc` | Exits building mode without placing another segment. |
| **Split Wall / Insert Point** | `Ctrl + Left-click` on a wall line | Splits the 2-point wall into two connected walls at the clicked position (does not enter building mode). |
| **Move Whole Building** | `Alt + Left-drag` on a wall | Moves the entire connected building (walls, openings, and interior furniture) independently of other buildings on the plan. |
| **Context Menu** | `Right-click` on point, wall, or opening | Opens a native context menu (convert corner/curve, insert door/window, insert point, move building, delete). |
| **Delete Selected** | `Del` / `Backspace` | Deletes the selected opening or anchor point (merging adjacent 2-point walls if applicable). |
| **Pan / Zoom 2D** | `Middle-drag` / `Mouse Wheel` | Pan and zoom the 2D floorplan canvas. |

---

## 2. 3D Viewer Window

- **Orbit Camera:** `Right-drag` inside the **Homely 3D — 3D Viewer** window.
- **Zoom:** `Mouse Wheel` inside the 3D Viewer window.
- **Show / Hide / Reset:** Use **View → Reset 3D View** or **View → Show/Hide 3D Viewer** in the top menu bar. Closing the 2D Planner closes both windows cleanly.

---

## 3. Doors & Windows

- **Drag & Drop from Catalog:** Drag any door or window from the left **Catalog** panel onto any wall in the 2D floorplan. A green preview indicates placement; releasing creates the opening and its 3D wall cutout.
- **Via Context Menu:** `Right-click` a wall → **Insert Door Here** or **Insert Window Here**.
- **Properties (Inspector):**
  - **Name:** Custom label for the opening.
  - **Width / Height / Position:** Exact dimensions in meters and normalized position `t` along the wall.
  - **Height from Floor:** Vertical offset from the level floor (e.g., `0.90 m` for windows).
  - **Door Swing:** **Flip Opening Side** and **Flip Hinge Side** buttons.

---

## 4. Inspector, Overview & History

- **Overview Minimap:** Shows the entire plan and the current 2D viewport rectangle; click or drag inside it to jump to any area of a large plan.
- **Scene Tree:** Collapsible hierarchy grouped by detected **Rooms** (`Walls`, `Openings`, `Furniture`) plus **Unassigned** elements. Click any entry to select and rename it.
- **History Tab:** Lists every reversible step (`0. Initial State` onward). Click any step to jump directly to that state, or use `Ctrl+Z` / `Ctrl+Y`.

---

## 5. Project Management

- **File → New Project... / Project Settings...:** Configure project name, `.h3d` save location (via native file dialog), plan size (`W × H` in meters), and default wall thickness/height.
- **File → Open (.h3d)... / Save / Save As...:** Load and save native `.h3d` project archives.
- **File → Import (.sh3x)...:** Import `.sh3x` project archives.
- **Language:** Switch live between **Deutsch** and **English** under **Language**.

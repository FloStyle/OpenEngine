# Editor — rudiments (Unreal-like)

The editor (`crates/editor-shell`) is a windowed egui/wgpu app on top of the
headless editor core (`crates/editor`). Layout mirrors an Unreal/Unity-style
DCC: a lit 3D **viewport** in the middle, a **hierarchy** (outliner) on the
left, an **inspector** (details) on the right, and a **toolbar** on top.

```
┌──────────────────────────────────────────────────────────────────────┐
│ toolbar: Mode · Q Select | W Move | snap·grid · ▶Play/⏹Stop · Save/Load│
├──────────┬─────────────────────────────────────┬──────────────────────┤
│ Hierarchy│       3D VIEWPORT (lit, orbit)      │  Inspector (details) │
│ + Add    │   spheres + checkered ground        │   Transform pos/scale│
│ 🗑        │                                     │                      │
│ entities │                                     │                      │
└──────────┴─────────────────────────────────────┴──────────────────────┘
```

## Shortcuts / interactions
| Action | Input |
|---|---|
| Orbit camera | **right-drag** (or Alt+left) |
| Pan | **middle-drag** |
| Zoom | **scroll wheel** |
| Select actor | click it in the viewport or in the Hierarchy |
| Move tool | **W** (then drag the selected actor on the ground) |
| Select tool | **Q** |
| Snap toggle + grid step | Move-mode toolbar (Snap, grid N) |
| Add actor | **+ Add Actor** (hierarchy) |
| Delete actor | **Delete** (or 🗑) |
| Duplicate actor | **Ctrl+D** |
| Play / Stop | toolbar ▶ Play / ⏹ Stop (wasm engine) |
| Save / Load scene | **💾 Save / 📂 Load** (shared ecs scene codec) |
| Frame scene | **F** |

## Try-it checklist (validate on your display)
```bash
cargo run -p openengine-editor-shell
```
1. Window opens with the layout above; orbit/pan/zoom feel right.
2. **Q** (Select): click an NPC in the viewport → it is selected (hierarchy +
   inspector reflect it).
3. **W** (Move): drag the selected actor across the ground; with **Snap** on it
   steps by the grid value.
4. **+ Add Actor**: a new actor appears and is selected. Move it. **Ctrl+D**
   duplicates it (offset +0.5). **Delete** removes it.
5. **💾 Save** writes `scene.json`; **📂 Load** restores it (Stop first).
6. **▶ Play**: WASD/Space move the player, NPCs wander/chase (toolbar shows
   `engine: wasm`); **⏹ Stop** returns to editing.
7. Repack the edited scene: `bash scripts/package.sh demo scene.json`.

## What's implemented vs. next
- Done (headless-verified where noted): select/pick, Move-on-ground + grid snap
  (math tested), Add/Delete/Duplicate (tested), Save/Load, Play-wasm, camera.
- Next (windowed, validate then refine): a visible **transform gizmo with
  X/Y/Z handles**, **Rotate/Scale** tools, a **visual grid** overlay, PIE-style
  maximized game view, actor **rename/labels**.
- The "feel familiar to a UE user" bar is a UX judgement only you can make on a
  real display; use the checklist above and report what doesn't feel right.

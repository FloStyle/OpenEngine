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
├──────────┴─────────────────────────────────────┴──────────────────────┤
│ status: entities · mode · tool · selected x/y/z · engine              │
└───────────────────────────────────────────────────────────────────────┘
```

## Shortcuts / interactions
| Action | Input |
|---|---|
| Orbit camera | **right-drag** (or Alt+left) |
| Pan | **middle-drag** |
| Zoom | **scroll wheel** |
| Select actor | click it in the viewport or in the Hierarchy |
| Select tool | **Q** |
| Move tool | **W** (then drag the selected actor on the ground) |
| Rotate tool | **E** (drag horizontally to yaw about Y) |
| Scale tool | **R** (drag horizontally for uniform scale) |
| Snap toggle + grid step | Move-mode toolbar (Snap, grid N) |
| Add actor | **+ Add Actor** (hierarchy) |
| Delete actor | **Delete** (or 🗑) |
| Duplicate actor | **Ctrl+D** |
| Rename actor | select it → Inspector **Name** field |
| Play / Stop | toolbar ▶ Play / ⏹ Stop (wasm engine) |
| Save / Load scene | **💾 Save / 📂 Load** (shared ecs scene codec) |
| Play-in-Editor (maximize viewport while playing) | **⛶ PIE** toolbar toggle |
| Visual ground grid | **Grid** toolbar checkbox (off by default) |
| Frame scene | **F** |

## Launch it (double-click friendly)
```bash
# One-click launcher (builds if needed, opens the editor window):
bash scripts/editor.sh
# force a release build / auto-play a scene:
bash scripts/editor.sh --release
bash scripts/editor.sh --play examples/demo-chase.json
```
On Linux you can double-click `scripts/editor.sh` (or install
`.desktop/openengine-editor.desktop` into `~/.local/share/applications/` for a
file-manager launcher). The editor needs a graphical session (Vulkan/GPU); on a
headless box the script prints a friendly message instead.

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
- Done (headless-verified where noted): select/pick; Move-on-ground + grid snap;
  Rotate (E) and Scale (R) tools with drag (rotate_yaw/scale_uniform math
  headless-tested); Add/Delete/Duplicate (tested); Rename (Name field, tested);
  Save/Load; Play-wasm; PIE (maximize viewport); camera; status bar.
- Next (windowed, validate then refine): an on-screen **transform gizmo** with
  X/Y/Z handles (the current tools are axis-free drags), a **visual grid**
  overlay, toolbar **icons**, look&feel pass.
- The "feel familiar to a UE user" bar is a UX judgement only you can make on a
  real display; use the checklist above and report what doesn't feel right.

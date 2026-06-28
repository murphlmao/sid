# GPUI-on-Wayland Spike — Findings

**Date:** 2026-06-27 · **Verdict: 🟢 GREEN — proceed with GPUI.**

The spike (a static app-shell: titlebar + scope buttons + tab strip + a scrollable
`uniform_list` of fake SSH hosts with monospace subtitles) builds, renders, and takes input
on the target machine. No failure mode remains. Next: plan 2 — the layered store foundation.

## Environment

- **OS / session:** Arch Linux, Wayland (`WAYLAND_DISPLAY=wayland-1`, `XDG_SESSION_TYPE=wayland`).
- **GPU / Vulkan:** Intel Iris Xe Graphics (ADL GT2), Mesa open-source driver, Vulkan 1.4
  (`intel_icd.json`). blade found a hardware device — no `NoSupportedDeviceFound`.
- **Toolchain:** Rust 1.96.0, edition 2024, resolver 3.
- **gpui:** `0.2` (crates.io, self-contained monolithic `Application::new().run(...)`).

## Results

| Dimension | Result |
|---|---|
| Build | ✅ Green. Heavy first compile (large graphics/font tree); incremental rebuild of the `sid` crate ~2.7s. |
| Render | ✅ Titlebar, tab strip, scrollable list all paint correctly; flat grayscale as intended. |
| **Monospace text** | ✅ Crisp and column-aligned — validates the embedded-terminal rendering foundation (the SSH spearhead's hard piece). |
| **Pointer input + event loop** | ✅ 30 `on_click` events dispatched correctly across multiple hosts; hover restyling wired. |
| Vulkan | ✅ Hardware ICD present and selected. |

## Fixes needed to compile (both trivial, edition-2024 related)

1. `uniform_list` processor closure needed an explicit `range: std::ops::Range<usize>`
   annotation (E0282 — inference couldn't resolve it).
2. `host_row(&self, …) -> impl IntoElement` had to opt out of edition-2024's implicit RPIT
   lifetime capture with `+ use<>` — otherwise the returned element was treated as borrowing
   `self`, which the processor closure rejects. (The body clones everything, so it borrows
   nothing in fact.)

## Caveats / open risks (for later, not blockers)

- **gpui 0.2.x is pre-1.0.** Expect breaking API changes; pin deliberately and bump on purpose.
- **No built-in text input.** gpui ships no `TextInput`; the examples hand-roll one (focus
  handle + key handling + caret). The SSH command line, query editor, and search boxes will
  each need a real input component — non-trivial, plan for it.
- **Wayland fractional scaling → blurry text** (known gpui issue). Mitigate with integer
  display scale (100%/200%), maximize/fullscreen, or XWayland (`WAYLAND_DISPLAY=''`). Revisit
  for HiDPI/fractional-scale users.
- **GUI feedback must be in-pixel.** Unlike a CLI there's no stdout; selection/active state
  must be rendered (flip a `selected` field and restyle). The spike only printed to stdout,
  which is why clicks looked inert when launched detached.

## Reference — system deps on a fresh Arch box

This machine already had everything (gpui built and ran with no extra installs). For a fresh
Arch + Wayland setup, the build+run set (mirrors Zed's `script/linux`):

```bash
sudo pacman -S --needed base-devel clang cmake pkgconf mold \
  wayland libxkbcommon-x11 libxcb fontconfig freetype2 \
  vulkan-icd-loader vulkan-tools
# plus the GPU's Vulkan ICD: vulkan-intel | vulkan-radeon | nvidia-utils
```

blade is Vulkan-only and rejects software (`llvmpipe`) — a real hardware ICD is required at
runtime (`vulkaninfo --summary` must show a hardware GPU, not `is_software_emulated: true`).

# Headless launch-survives smoke for the `sid` GPUI binary (Plan: sid
# integration/automation harness, 2026-07-02). NOT a pixel/golden-image
# test — see docs/design/2026-07-02-testing-strategy.md for the reasoning.
#
# GPUI's Linux backend picks a platform at startup by env var
# (gpui-0.2.2's `platform::linux::guess_compositor`): WAYLAND_DISPLAY set ->
# Wayland, else DISPLAY set -> X11, else a true no-window `HeadlessClient`
# that *cannot* open a window at all (verified by reading gpui's source —
# `HeadlessClient::open_window` unconditionally errors). So "headless" here
# means X11-via-Xvfb, not GPUI's own headless backend.
#
# Rendering (`blade-graphics`, Zed's GPU abstraction) needs a Vulkan ICD on
# Linux — there is no GL fallback. `mesa-vulkan-drivers` ships Lavapipe (a
# software/CPU Vulkan implementation), which is what makes this work with no
# GPU passthrough at all.
#
# Two-stage build: compile in a full `rust:bookworm` image (needs the same
# dev headers gpui/gpui-component pull in for wayland+x11 features even
# though this smoke only exercises the X11 path), run in a slim runtime image
# with just Xvfb + Lavapipe + the shared libs the binary links against.

FROM rust:bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    libasound2-dev \
    libfontconfig1-dev \
    libwayland-dev \
    wayland-protocols \
    libxkbcommon-dev \
    libxkbcommon-x11-dev \
    libx11-dev \
    libx11-xcb-dev \
    libxcb1-dev \
    libxcb-render0-dev \
    libxcb-shape0-dev \
    libxcb-xfixes0-dev \
    libxcb-xkb-dev \
    libxrandr-dev \
    libxi-dev \
    libgit2-dev \
    cmake \
    clang \
    libclang-dev \
    git \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /work
COPY . .
RUN cargo build -p sid

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    xvfb \
    xdotool \
    mesa-vulkan-drivers \
    libvulkan1 \
    libfontconfig1 \
    libxkbcommon0 \
    libxkbcommon-x11-0 \
    libwayland-client0 \
    libx11-6 \
    libxcb1 \
    libxcb-render0 \
    libxcb-shape0 \
    libxcb-xfixes0 \
    libxrandr2 \
    libxi6 \
    libasound2 \
    ca-certificates \
    procps \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /work/target/debug/sid /usr/local/bin/sid
COPY docker/headless-smoke/run-smoke.sh /usr/local/bin/run-smoke.sh
RUN chmod +x /usr/local/bin/run-smoke.sh

ENTRYPOINT ["/usr/local/bin/run-smoke.sh"]

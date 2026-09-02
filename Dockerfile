# OpenEngine — reproducible build/test environment.
# Portable: works on any machine with Docker; no GPU required for logic tests.
# Base is Debian-based (NOT Arch/Alpine) so wgpu/winit system deps are stable.
FROM rust:1.85-slim-bookworm

# Build-time system deps for compiling wgpu/winit on Linux. These are compile
# deps only; the logic test suite needs no GPU at runtime.
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    pkg-config \
    libssl-dev \
    libx11-dev \
    libxkbcommon-dev \
    libwayland-dev \
    libudev-dev \
    python3 \
    python3-pip \
    && rm -rf /var/lib/apt/lists/*

# Domain B cross-compile target.
RUN rustup target add wasm32-unknown-unknown

WORKDIR /workspace
COPY . .

# Default: run all tests (host + no_std guest unit tests). Logic tests need no
# GPU. After an initial `cargo fetch` builds work offline.
CMD ["cargo", "test", "--workspace"]

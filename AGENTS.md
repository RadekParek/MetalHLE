# AGENTS.md

## Cursor Cloud specific instructions

### Overview

This repo contains two products:
1. **touchHLE emulator** (Rust) — an HLE iPhone OS app emulator
2. **AppDB** (Python/FastAPI) — a compatibility report web app at `appdb/`

### Building the Rust emulator

- **Git submodules must be initialized** before building: `git submodule update --init`
- The build requires `libboost-dev`, `libstdc++-14-dev`, `libsndio-dev`, and X11 development headers (`libxext-dev`, `libx11-dev`, etc.) on Linux.
- On this Cloud Agent VM (x86_64 Linux with clang), you **must** set `RUSTFLAGS="-C link-arg=-latomic"` for linking to succeed. OpenAL Soft's static build uses `std::atomic` operations that require `libatomic`, but the build scripts don't link it automatically.
- Build: `RUSTFLAGS="-C link-arg=-latomic" cargo build`
- The emulator binary requires a display/window system (SDL2) to run graphically. In headless environments, use `--help`, `--copyright`, or `--info` flags to verify the binary works.

### Linting and testing

- Lint: `dev-scripts/lint.sh` (runs comment-length check + `cargo clippy -- --deny warnings` + `cargo doc`). Note: the `t` branch has pre-existing long comment lines and clippy warnings with Rust 1.95+, so the full lint script will fail. Running `cargo clippy` alone (without `--deny warnings`) confirms no hard errors.
- Tests: `RUSTFLAGS="-C link-arg=-latomic" cargo test -- --skip test_app` (the `test_app` integration tests require a custom LLVM/SDK toolchain not available in the Cloud VM).
- Format: `dev-scripts/format.sh` (requires `clang-format` for C/C++ files).

### Running the AppDB

- Setup: `cd appdb && python3 -m venv .venv && source .venv/bin/activate && pip install -e .`
- Run: `cd appdb && source .venv/bin/activate && uvicorn app.main:app --host 0.0.0.0 --port 8000 --reload`
- The SQLite DB is auto-created and seeded with demo data on first startup.
- Submitting reports requires GitHub OAuth (`GITHUB_OAUTH_CLIENT_ID` / `GITHUB_OAUTH_CLIENT_SECRET` env vars); without these, the app works in read-only mode.
- The triage JSON API requires `APPDB_TRIAGE_TOKEN` env var.

## MetalHLE workflow (RadekParek fork)

### Upstream sync (mandatory, every session and before every push)

1. `git fetch upstream trunk` (upstream = https://github.com/KlugKlugTG/HyperHLE-Fork.git)
2. `git rev-list HEAD..upstream/trunk --count` — if > 0, KlugKlugTG has commits we don't:
   - Stash local WIP (`git stash push -u`), merge (`git merge upstream/trunk --no-edit`),
     pop the stash, resolve any conflicts before continuing.
3. Then (and only then) proceed with new work or push.

### Quality gates

- Trunk stays green: `cargo check` must pass (and `cargo clippy` for touched files) before every push.
- Update `CHANGELOG.md` at the end of every work session, before pushing.
- One concern per commit; no AI slop; clean up Russian/placeholder markers in touched files.
- Never break what's working: prefer additive ports over rewrites of existing behaviour.

### Layout

- `origin` = RadekParek/MetalHLE (our trunk), `upstream` = KlugKlugTG/HyperHLE-Fork (base fork).
- Ported legacy features live in `src/environment64.rs` (ARM64 path), `src/arm64_runtime.rs`,
  `src/cpu/a64/`, `src/mem64.rs`, `src/mach_o64.rs` — grafted from the geofasada-star/MetalHLE-test
  fork, then adapted (Russian markers removed, APIs matched to this tree).

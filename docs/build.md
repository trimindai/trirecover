# Building TriRecover

## Prerequisites

| Tool          | Version | Notes                                                |
| ------------- | ------- | ---------------------------------------------------- |
| Rust          | 1.80+   | `rustup default 1.80` and `rustup component add rustfmt clippy` |
| Node.js       | 20+     | LTS recommended.                                     |
| pnpm          | 9+      | `npm i -g pnpm`                                      |
| Tauri CLI     | 2.x     | `cargo install tauri-cli --version "^2"`             |
| Inno Setup 6  | 6.2+    | Windows installer build (Windows host).              |
| Visual Studio | 2022    | "Desktop development with C++" workload (Windows).   |

## First-time setup

```bash
git clone https://github.com/trimindai/trirecover.git
cd trirecover
pnpm -C frontend install
cargo fetch
```

## Run in dev mode

```bash
# Linux dev build (uses /dev/sdX as a stand-in for raw disk I/O — read-only)
cargo tauri dev

# Windows dev build
cargo tauri dev
```

The frontend dev server runs at `http://localhost:1420` and the Rust shell auto-reloads on backend changes.

## Run tests

```bash
# Rust unit + integration tests
cargo test --workspace

# Frontend tests
pnpm -C frontend test
```

## Lint

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
pnpm -C frontend lint
```

## Build a release

```bash
# Production binary + bundle
cargo tauri build

# Output:
# Windows: src-tauri/target/release/bundle/{nsis,msi}/...
# Linux:   src-tauri/target/release/bundle/{deb,appimage}/...
```

## Build the signed Windows installer

The Inno Setup script bundles assets the Tauri MSI does not (license, EULA, README). To build:

```bat
:: from a Windows host
cargo tauri build --target x86_64-pc-windows-msvc
ISCC.exe installer\trirecover.iss
:: => installer\Output\TriRecover-Setup-0.1.0.exe
```

For code signing, set `SIGNING_PFX_PATH` and `SIGNING_PFX_PASSWORD` env vars, and the Inno script will call `signtool` automatically.

## CI/CD

`.github/workflows/ci.yml` runs lint + test on every push.
`.github/workflows/release.yml` runs on tag `v*` and produces:
- `TriRecover-Setup-x.y.z.exe` (Inno installer, signed if secret available)
- `TriRecover-Portable-x.y.z.zip`
- SHA256SUMS

## Targets

We ship Windows 10 (1809+) and Windows 11 x64. Other targets are dev-only.

## Reproducible builds

`Cargo.lock` is committed. The release workflow pins Rust toolchain via `rust-toolchain.toml` and Node via `.nvmrc`. Bundled DLLs (sqlite3, etc.) are vendored under `vendor/`.

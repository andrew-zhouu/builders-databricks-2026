#!/usr/bin/env bash
# Install a project-local Rust toolchain (musl target, linked with rust-lld so no
# system C compiler is needed) and a Python venv with the checker's libraries.
set -euo pipefail
. /home/andre/builders/databricks/scripts/env.sh
mkdir -p "$PROJ/.toolchain/bin"
cd "$PROJ"

if ! command -v cargo >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o "$PROJ/.toolchain/rustup-init.sh"
  sh "$PROJ/.toolchain/rustup-init.sh" -y --no-modify-path --profile minimal \
     --default-toolchain stable --target x86_64-unknown-linux-musl
fi
rustc --version
cargo --version
rustup target list --installed

if ! command -v uv >/dev/null 2>&1; then
  curl -LsSf https://astral.sh/uv/install.sh -o "$PROJ/.toolchain/uv-install.sh"
  UV_INSTALL_DIR="$PROJ/.toolchain/bin" UV_NO_MODIFY_PATH=1 INSTALLER_NO_MODIFY_PATH=1 \
    sh "$PROJ/.toolchain/uv-install.sh"
fi
uv --version
if [ ! -d "$PROJ/.venv" ]; then
  uv venv "$PROJ/.venv" --python 3.12
fi
uv pip install --python "$PROJ/.venv/bin/python" polars pyarrow duckdb numpy
"$PROJ/.venv/bin/python" -c "import polars, pyarrow, duckdb; print('polars', polars.__version__, 'pyarrow', pyarrow.__version__, 'duckdb', duckdb.__version__)"
echo "--- docker server"
docker version --format 'server {{.Server.Version}}' 2>&1 | head -2 || true

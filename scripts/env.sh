# Source this: project-local toolchain paths (nothing is installed outside the project).
# PROJ is the repository root, wherever it is checked out.
export PROJ="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export RUSTUP_HOME="$PROJ/.toolchain/rustup"
export CARGO_HOME="$PROJ/.toolchain/cargo"
export UV_CACHE_DIR="$PROJ/.toolchain/uv-cache"
export UV_PYTHON_INSTALL_DIR="$PROJ/.toolchain/uv-python"
export PATH="$CARGO_HOME/bin:$PROJ/.toolchain/bin:$PROJ/.venv/bin:$PATH"

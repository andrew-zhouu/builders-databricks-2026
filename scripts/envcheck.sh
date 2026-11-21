#!/usr/bin/env bash
# Report which toolchains exist in WSL and current machine load.
for t in rustc cargo gcc g++ clang make cmake docker python3 pip3 uv git zig unzip perf; do
  if command -v "$t" >/dev/null 2>&1; then
    printf '%-8s %s  ' "$t" "$(command -v "$t")"
    case "$t" in
      docker) docker --version 2>&1 | head -1 ;;
      python3) python3 --version ;;
      *) "$t" --version 2>&1 | head -1 ;;
    esac
  else
    printf '%-8s MISSING\n' "$t"
  fi
done
echo "--- ~/.cargo"; ls ~/.cargo/bin 2>/dev/null
echo "--- python pkgs"; python3 -I -c "
import importlib
for m in ('polars','pyarrow','duckdb','numpy'):
    try:
        mod = importlib.import_module(m); print(m, mod.__version__)
    except Exception as e:
        print(m, 'MISSING')
"
echo "--- cpu"; lscpu | grep -E 'Model name|^CPU\(s\)|Thread|Core|Flags' | sed 's/Flags.*avx512/Flags: has avx512/' | cut -c1-200
grep -o 'avx512[a-z0-9_]*' /proc/cpuinfo | sort -u | tr '\n' ' '; echo
echo "--- load"; uptime; ps -eo pcpu,pmem,comm --sort=-pcpu | head -8
echo "--- docker"; docker info 2>&1 | head -5
echo "--- sudo"; sudo -n true 2>&1 | head -1 && echo "sudo ok (passwordless)"

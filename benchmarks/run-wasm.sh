#!/usr/bin/env bash
set -euo pipefail

profile="${1:-smoke}"
max_dimension="${2:-512}"
case "$profile" in
  smoke|memory) ;;
  *) echo "profile must be smoke or memory" >&2; exit 2 ;;
esac

benchmark_root="$(cd "$(dirname "$0")" && pwd)"
repository_root="$(cd "$benchmark_root/.." && pwd)"
corpus_directory="$benchmark_root/corpus/$profile"
package_directory="$benchmark_root/wasm-pkg"
results_directory="$benchmark_root/results"
result_file="$results_directory/wasm-$profile.jsonl"

mkdir -p "$results_directory"
cd "$repository_root"
cargo run --release --manifest-path benchmarks/Cargo.toml -- "generate-$profile" "$corpus_directory"
wasm-pack build crates/streamthumb-wasm --release --target nodejs --out-dir "$package_directory"
node benchmarks/run-wasm.cjs "$package_directory" "$corpus_directory" "$result_file" "$max_dimension"
echo "$result_file"

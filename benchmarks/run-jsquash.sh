#!/usr/bin/env bash
set -euo pipefail

profile="${1:-smoke}"
max_dimension="${2:-512}"
case "$profile" in
  smoke|memory|adam7) ;;
  *) echo "profile must be smoke, memory, or adam7" >&2; exit 2 ;;
esac

benchmark_root="$(cd "$(dirname "$0")" && pwd)"
repository_root="$(cd "$benchmark_root/.." && pwd)"
corpus_directory="$benchmark_root/corpus/$profile"
results_directory="$benchmark_root/results"
result_file="$results_directory/jsquash-$profile.jsonl"

mkdir -p "$results_directory"
cd "$repository_root"
cargo run --release --manifest-path benchmarks/Cargo.toml -- "generate-$profile" "$corpus_directory"
npm ci --prefix benchmarks
node benchmarks/run-jsquash.mjs "$corpus_directory" "$result_file" "$max_dimension"
echo "$result_file"

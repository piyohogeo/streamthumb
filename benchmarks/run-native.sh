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
binary="$benchmark_root/target/release/streamthumb-benchmarks"
result_file="$results_directory/native-$profile.jsonl"

mkdir -p "$results_directory"
cd "$repository_root"
cargo build --release --manifest-path benchmarks/Cargo.toml
"$binary" "generate-$profile" "$corpus_directory"
: > "$result_file"

for input_file in "$corpus_directory"/*.png; do
  base_name="$(basename "$input_file" .png)"
  for method in streamthumb-png streamthumb-jpeg streamthumb-cover-png streamthumb-cover-jpeg image-rs; do
    extension="png"
    if [[ "$method" = *jpeg ]]; then extension="jpg"; fi
    output_file="$results_directory/$base_name-$method.$extension"
    stdout_file="$(mktemp)"
    time_file="$(mktemp)"
    /usr/bin/time -f '%M' -o "$time_file" \
      "$binary" run "$method" "$input_file" "$output_file" "$max_dimension" > "$stdout_file"
    peak_kib="$(cat "$time_file")"
    python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); r.update(peak_rss_bytes=int(sys.argv[2])*1024, platform="linux"); print(json.dumps(r,separators=(",",":")))' "$stdout_file" "$peak_kib" >> "$result_file"
    rm -f "$stdout_file" "$time_file"
  done
done

echo "$result_file"

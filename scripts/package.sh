#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/.." && pwd)"
plugin_name="com.emilsvaldmanis.codexlimits.sdPlugin"
dist_dir="$repo_root/dist"
stage_dir="$dist_dir/$plugin_name"
archive="$dist_dir/com.emilsvaldmanis.codexlimits.streamDeckPlugin"
binary="$repo_root/target/release/codex-limits"

cargo build --release --manifest-path "$repo_root/Cargo.toml"

case "$stage_dir" in
  "$repo_root"/dist/*.sdPlugin) ;;
  *)
    echo "Refusing to replace unexpected staging path: $stage_dir" >&2
    exit 1
    ;;
esac

rm -rf -- "$stage_dir"
mkdir -p "$stage_dir/x86_64-unknown-linux-gnu/bin"
mkdir -p "$stage_dir/assets"
cp "$repo_root/assets/manifest.json" "$stage_dir/manifest.json"
cp -R "$repo_root/assets/icons" "$stage_dir/icons"
cp -R "$repo_root/assets/propertyInspector" "$stage_dir/propertyInspector"
cp -R "$repo_root/assets/branding" "$stage_dir/assets/branding"
cp "$repo_root/LICENSE" "$stage_dir/LICENSE"
cp "$repo_root/README.md" "$stage_dir/README.md"
cp "$repo_root/CONTRIBUTING.md" "$stage_dir/CONTRIBUTING.md"
install -m 0755 "$binary" "$stage_dir/x86_64-unknown-linux-gnu/bin/codex-limits"

rm -f -- "$archive"
(
  cd "$dist_dir"
  zip -q -r "$archive" "$plugin_name"
)

echo "Plugin directory: $stage_dir"
echo "Installable package: $archive"

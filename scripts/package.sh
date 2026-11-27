#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "$script_dir/.." && pwd)"
plugin_id="com.emilsvaldmanis.codexlimits"
plugin_name="$plugin_id.sdPlugin"
archive_name="$plugin_id.streamDeckPlugin"
dist_dir="$repo_root/dist"
platform="x86_64-unknown-linux-gnu"
binary_name="codex-limits"
package_binary="$platform/bin/$binary_name"
target_dir="${CARGO_TARGET_DIR:-$repo_root/target}"

cargo build --release --locked --manifest-path "$repo_root/Cargo.toml" \
  --target "$platform" --target-dir "$target_dir" --bin "$binary_name"

package_id=$(cargo pkgid --locked --manifest-path "$repo_root/Cargo.toml")
# Cargo prints path+URL#version or path+URL#name@version.
version=${package_id##*[@#]}

mkdir -p "$dist_dir"
staging=$(mktemp -d "$dist_dir/.package.XXXXXXXX")
trap 'rm -rf -- "$staging"' EXIT
stage_dir="$staging/$plugin_name"
mkdir -p "$stage_dir/assets"
sed -e "s|@VERSION@|$version|g" -e "s|@BINARY_PATH@|$package_binary|g" \
  "$repo_root/assets/manifest.json.in" > "$stage_dir/manifest.json"
cp -R "$repo_root/assets/icons" "$repo_root/assets/propertyInspector" "$stage_dir/"
cp -R "$repo_root/assets/branding" "$stage_dir/assets/"
cp "$repo_root/LICENSE" "$repo_root/README.md" "$repo_root/CONTRIBUTING.md" "$stage_dir/"
install -D -m 0755 "$target_dir/$platform/release/$binary_name" "$stage_dir/$package_binary"

(
  cd "$staging"
  zip -q -r "$archive_name" "$plugin_name"
)
(
  flock -x 9
  if [[ -e "$dist_dir/$plugin_name" ]]; then
    mv -T -- "$dist_dir/$plugin_name" "$staging/previous"
  fi
  mv -T -- "$stage_dir" "$dist_dir/$plugin_name"
  mv -fT -- "$staging/$archive_name" "$dist_dir/$archive_name"
) 9<"$dist_dir"

echo "Plugin directory: $dist_dir/$plugin_name"
echo "Installable package: $dist_dir/$archive_name"

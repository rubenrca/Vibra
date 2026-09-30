# Shared helpers for packaging and release scripts.
# shellcheck disable=SC2034

vibra_cargo_version() {
  local root=$1
  sed -n 's/^version = "\([^"]*\)"/\1/p' "$root/Cargo.toml" | head -n 1
}

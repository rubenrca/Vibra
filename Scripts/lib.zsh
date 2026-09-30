# Shared helpers for packaging and release scripts.
# shellcheck disable=SC2034

vibra_repo_root() {
  local script_path=${1:A}
  print -r -- ${script_path:h:h}
}

vibra_cargo_version() {
  local root=$1
  sed -n 's/^version = "\([^"]*\)"/\1/p' "$root/Cargo.toml" | head -n 1
}

vibra_verify_checksum() {
  local archive=$1
  local expected=$2
  local actual
  actual=$(shasum -a 256 "$archive" | cut -d ' ' -f 1)
  if [[ $actual != $expected ]]; then
    print -u2 -- 'archive checksum mismatch'
    return 1
  fi
}

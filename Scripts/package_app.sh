#!/bin/zsh

set -euo pipefail

configuration=${1:-debug}
repo_root=${0:A:h:h}
app_dir="$repo_root/dist/Vibra.app"
contents_dir="$app_dir/Contents"
macos_dir="$contents_dir/MacOS"
resources_dir="$contents_dir/Resources"
plist="$contents_dir/Info.plist"
icon_source="$repo_root/Resources/AppIcon.png"
iconset_dir="$repo_root/.build/Vibra.iconset"
icon_file="$resources_dir/Vibra.icns"

case "$configuration" in
  debug|release) ;;
  *)
    print -u2 "usage: $0 [debug|release]"
    exit 64
    ;;
esac

swift build --package-path "$repo_root" -c "$configuration" --product Vibra
binary_path=$(swift build --package-path "$repo_root" -c "$configuration" --show-bin-path)/Vibra

mkdir -p "$macos_dir" "$resources_dir"
cp "$binary_path" "$macos_dir/Vibra"
swift "$repo_root/Scripts/generate_app_icon.swift" "$icon_source" "$iconset_dir"
iconutil -c icns "$iconset_dir" -o "$icon_file"

plutil -create xml1 "$plist"
plutil -insert CFBundleDisplayName -string Vibra "$plist"
plutil -insert CFBundleExecutable -string Vibra "$plist"
plutil -insert CFBundleIdentifier -string app.vibra.Vibra "$plist"
plutil -insert CFBundleIconFile -string Vibra "$plist"
plutil -insert CFBundleInfoDictionaryVersion -string 6.0 "$plist"
plutil -insert CFBundleName -string Vibra "$plist"
plutil -insert CFBundlePackageType -string APPL "$plist"
plutil -insert CFBundleShortVersionString -string 0.1.0 "$plist"
plutil -insert CFBundleVersion -string 1 "$plist"
plutil -insert LSMinimumSystemVersion -string 14.0 "$plist"
plutil -insert NSHighResolutionCapable -bool true "$plist"

codesign --force --sign - --timestamp=none "$app_dir"
print "$app_dir"

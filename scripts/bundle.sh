#!/usr/bin/env bash
# Builds the release binary and makes "dist/Better Finder.app".
# The script signs the bundle ad hoc (no developer identity).
#
# Usage: scripts/bundle.sh [--install]
#   --install  Also copy the app to /Applications, so that Spotlight,
#              Raycast, and Launchpad find it.
set -euo pipefail

install=false
case "${1:-}" in
    --install) install=true ;;
    "") ;;
    *) echo "usage: scripts/bundle.sh [--install]" >&2; exit 2 ;;
esac

cd "$(dirname "$0")/.."

cargo build --release -p bf-app

# The version comes from Cargo (for example "path+file:///x#bf-app@0.1.0").
version="$(cargo pkgid -p bf-app | sed -E 's/.*[#@]([0-9][^#@]*)$/\1/')"
app="dist/Better Finder.app"

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/better-finder "$app/Contents/MacOS/better-finder"
# scripts/make-icon.swift makes the icon file.
cp assets/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"

# CFBundleDocumentTypes: the app can open folders (public.folder). The rank
# Alternate keeps Finder as the default app of folders. macOS 26 refuses a
# change of that default (error -50). README.md, "Use Better Finder as the
# file viewer", tells how other apps reach this app.
cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>Better Finder</string>
    <key>CFBundleDisplayName</key>
    <string>Better Finder</string>
    <key>CFBundleIdentifier</key>
    <string>com.p1ratedev.better-finder</string>
    <key>CFBundleExecutable</key>
    <string>better-finder</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleDocumentTypes</key>
    <array>
        <dict>
            <key>CFBundleTypeName</key>
            <string>Folder</string>
            <key>CFBundleTypeRole</key>
            <string>Viewer</string>
            <key>LSHandlerRank</key>
            <string>Alternate</string>
            <key>LSItemContentTypes</key>
            <array>
                <string>public.folder</string>
            </array>
        </dict>
    </array>
    <key>CFBundleShortVersionString</key>
    <string>${version}</string>
    <key>CFBundleVersion</key>
    <string>${version}</string>
    <key>LSMinimumSystemVersion</key>
    <string>13.0</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.productivity</string>
    <key>NSAppleEventsUsageDescription</key>
    <string>Better Finder opens text files in your editor in a terminal window.</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSPrincipalClass</key>
    <string>NSApplication</string>
</dict>
</plist>
EOF

codesign --force --sign - "$app"
echo "Built $app (version $version)"

if $install; then
    target="/Applications/Better Finder.app"
    # Replace only an earlier copy of this app, never a different app.
    if [ -e "$target" ]; then
        id="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$target/Contents/Info.plist" 2>/dev/null || true)"
        if [ "$id" != "com.p1ratedev.better-finder" ]; then
            echo "error: $target exists and is not Better Finder ($id)" >&2
            exit 1
        fi
        rm -rf "$target"
    fi
    ditto "$app" "$target"
    # Tell Launch Services about the new copy, so that the icon updates.
    /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$target"
    echo "Installed $target"
fi

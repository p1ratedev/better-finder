#!/usr/bin/env bash
# Builds the release binary and makes "dist/Better Finder.app".
# The script signs the bundle ad hoc (no developer identity).
set -euo pipefail

cd "$(dirname "$0")/.."

cargo build --release -p bf-app

# The version comes from Cargo (for example "path+file:///x#bf-app@0.1.0").
version="$(cargo pkgid -p bf-app | sed -E 's/.*[#@]([0-9][^#@]*)$/\1/')"
app="dist/Better Finder.app"

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/better-finder "$app/Contents/MacOS/better-finder"

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
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
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

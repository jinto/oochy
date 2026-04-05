#!/bin/bash
set -e

# Navigate to project root (where Cargo.toml workspace is)
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${PROJECT_ROOT}"

APP_NAME="KittyPaw"
BUNDLE_ID="com.kittypaw.app"
BIN_NAME="kittypaw-gui"
BUILD_DIR="target/release"
APP_DIR="${BUILD_DIR}/${APP_NAME}.app"

echo "Building ${APP_NAME} (release)..."
cargo build --release -p kittypaw-gui

echo "Creating ${APP_NAME}.app bundle..."
rm -rf "${APP_DIR}"
mkdir -p "${APP_DIR}/Contents/MacOS"
mkdir -p "${APP_DIR}/Contents/Resources"

# Copy binary
cp "${BUILD_DIR}/${BIN_NAME}" "${APP_DIR}/Contents/MacOS/${APP_NAME}"

# Compile and bundle Swift mic helper
echo "Compiling kittypaw-mic Swift helper..."
swiftc -O -o "${APP_DIR}/Contents/MacOS/kittypaw-mic" \
    "${SCRIPT_DIR}/kittypaw-mic.swift" \
    -framework Speech -framework AVFoundation 2>/dev/null || \
    echo "Warning: Swift mic helper compilation failed (optional)"

# Create Info.plist
cat > "${APP_DIR}/Contents/Info.plist" << PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>${APP_NAME}</string>
    <key>CFBundleDisplayName</key>
    <string>${APP_NAME}</string>
    <key>CFBundleIdentifier</key>
    <string>${BUNDLE_ID}</string>
    <key>CFBundleVersion</key>
    <string>0.1.0</string>
    <key>CFBundleShortVersionString</key>
    <string>0.1.0</string>
    <key>CFBundleExecutable</key>
    <string>${APP_NAME}</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>LSMinimumSystemVersion</key>
    <string>12.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key>
    <true/>
    <key>NSMicrophoneUsageDescription</key>
    <string>KittyPaw uses the microphone for voice input.</string>
    <key>NSSpeechRecognitionUsageDescription</key>
    <string>KittyPaw uses speech recognition to convert voice to text.</string>
</dict>
</plist>
PLIST

echo ""
echo "Done! ${APP_DIR}"
echo ""
echo "Run with:"
echo "  open ${APP_DIR}"

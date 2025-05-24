#!/bin/bash
# quick_build.sh - Simple script to build and install ZED without any checks
# Assumes the repository is already cloned and up to date

# Configuration
CLONE_DIR="$HOME/Developer/zed"
INSTALL_DIR="/Applications/FORKLIFT.app"
BINARY_PATH="$CLONE_DIR/target/release/zed"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOCAL_LOGO_PATH="$SCRIPT_DIR/forklift_logo.png"
WRAPPER_NAME="zed"
SYMLINK_PATH="/usr/local/bin/$WRAPPER_NAME"

# Check if repo exists
if [ ! -d "$CLONE_DIR" ]; then
  echo "Error: Repository directory does not exist at $CLONE_DIR"
  echo "Please run the full build script first to clone the repository."
  exit 1
fi

echo "🚀 FORKLIFT QUICK BUILD"
echo "Using existing code at $CLONE_DIR"
echo "No checks or updates will be performed."

# Build ZED
build_zed() {
  echo "Building ZED with --release flag..."
  cd "$CLONE_DIR"

  # Set environment variables for faster compilation
  export RUSTFLAGS="-C target-cpu=native"
  export CARGO_INCREMENTAL=1

  # Configure cargo to use all available cores
  cores=$(sysctl -n hw.ncpu)
  cores=${cores:-4} # Default if we can't detect

  echo "Using $cores CPU cores for build..."

  # Run the build with release flag
  echo "This may take a while..."
  cargo build --release

  if [ $? -ne 0 ]; then
    echo "Build failed."
    exit 1
  fi

  echo "Build completed successfully."
}

# Install the built application
install_zed() {
  echo "Installing FORKLIFT..."

  # Check if the binary exists
  if [ ! -f "$BINARY_PATH" ]; then
    echo "Error: Binary not found at $BINARY_PATH"
    exit 1
  fi

  # Check if a Zed.app bundle was created in the build output
  if [ -d "$CLONE_DIR/target/release/Zed.app" ]; then
    echo "Found Zed.app bundle in build output."

    # Remove any existing installation
    if [ -d "$INSTALL_DIR" ]; then
      echo "Removing existing installation..."
      sudo rm -rf "$INSTALL_DIR" 2>/dev/null
    fi

    # Create a temporary copy to customize
    TMP_BUNDLE="$CLONE_DIR/FORKLIFT.app"
    echo "Creating customized app bundle..."
    cp -R "$CLONE_DIR/target/release/Zed.app" "$TMP_BUNDLE"

    # Add custom logo
    echo "Adding FORKLIFT logo..."
    mkdir -p "$TMP_BUNDLE/Contents/Resources"

    # Convert PNG to ICNS
    TMP_ICONSET="$CLONE_DIR/tmp.iconset"
    mkdir -p "$TMP_ICONSET"

    # Generate different sizes
    sips -z 16 16 "$LOCAL_LOGO_PATH" --out "$TMP_ICONSET/icon_16x16.png" &>/dev/null
    sips -z 32 32 "$LOCAL_LOGO_PATH" --out "$TMP_ICONSET/icon_16x16@2x.png" &>/dev/null
    sips -z 32 32 "$LOCAL_LOGO_PATH" --out "$TMP_ICONSET/icon_32x32.png" &>/dev/null
    sips -z 64 64 "$LOCAL_LOGO_PATH" --out "$TMP_ICONSET/icon_32x32@2x.png" &>/dev/null
    sips -z 128 128 "$LOCAL_LOGO_PATH" --out "$TMP_ICONSET/icon_128x128.png" &>/dev/null
    sips -z 256 256 "$LOCAL_LOGO_PATH" --out "$TMP_ICONSET/icon_128x128@2x.png" &>/dev/null
    sips -z 256 256 "$LOCAL_LOGO_PATH" --out "$TMP_ICONSET/icon_256x256.png" &>/dev/null
    sips -z 512 512 "$LOCAL_LOGO_PATH" --out "$TMP_ICONSET/icon_256x256@2x.png" &>/dev/null
    sips -z 512 512 "$LOCAL_LOGO_PATH" --out "$TMP_ICONSET/icon_512x512.png" &>/dev/null
    sips -z 1024 1024 "$LOCAL_LOGO_PATH" --out "$TMP_ICONSET/icon_512x512@2x.png" &>/dev/null

    # Convert to icns
    iconutil -c icns "$TMP_ICONSET" -o "$TMP_BUNDLE/Contents/Resources/AppIcon.icns"
    rm -rf "$TMP_ICONSET"

    # Update app name in Info.plist
    if [ -f "$TMP_BUNDLE/Contents/Info.plist" ]; then
      /usr/libexec/PlistBuddy -c "Set :CFBundleDisplayName 'FORKLIFT'" "$TMP_BUNDLE/Contents/Info.plist" 2>/dev/null
      /usr/libexec/PlistBuddy -c "Set :CFBundleName 'FORKLIFT'" "$TMP_BUNDLE/Contents/Info.plist" 2>/dev/null
      /usr/libexec/PlistBuddy -c "Set :CFBundleIconFile AppIcon" "$TMP_BUNDLE/Contents/Info.plist" 2>/dev/null
    fi

    # Copy to Applications
    echo "Installing to Applications folder..."
    sudo cp -R "$TMP_BUNDLE" "$INSTALL_DIR"
    rm -rf "$TMP_BUNDLE"

    # Create symlink for CLI access
    echo "Creating command-line wrapper at $SYMLINK_PATH..."
    sudo rm -f "$SYMLINK_PATH"

    cat << EOF | sudo tee "$SYMLINK_PATH" > /dev/null
#!/bin/bash
if [ \$# -eq 0 ]; then
    open "/Applications/FORKLIFT.app"
else
    open -a "FORKLIFT" "\$@"
fi
EOF

    sudo chmod +x "$SYMLINK_PATH"
  else
    echo "No app bundle found. Creating one manually..."

    # Create basic app structure
    TMP_APP_DIR="$CLONE_DIR/FORKLIFT.app"
    mkdir -p "$TMP_APP_DIR/Contents/MacOS"
    mkdir -p "$TMP_APP_DIR/Contents/Resources"

    # Create Info.plist
    cat > "$TMP_APP_DIR/Contents/Info.plist" << EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>zed</string>
    <key>CFBundleIdentifier</key>
    <string>dev.zed.Zed</string>
    <key>CFBundleName</key>
    <string>FORKLIFT</string>
    <key>CFBundleDisplayName</key>
    <string>FORKLIFT</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleShortVersionString</key>
    <string>1.0</string>
    <key>CFBundleVersion</key>
    <string>1</string>
    <key>LSMinimumSystemVersion</key>
    <string>10.14</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
EOF

    # Copy the binary to the app bundle
    cp "$BINARY_PATH" "$TMP_APP_DIR/Contents/MacOS/"

    # Create app icon
    TMP_ICONSET="$CLONE_DIR/tmp.iconset"
    mkdir -p "$TMP_ICONSET"

    # Generate different sizes
    sips -z 16 16 "$LOCAL_LOGO_PATH" --out "$TMP_ICONSET/icon_16x16.png" &>/dev/null
    sips -z 32 32 "$LOCAL_LOGO_PATH" --out "$TMP_ICONSET/icon_16x16@2x.png" &>/dev/null
    sips -z 32 32 "$LOCAL_LOGO_PATH" --out "$TMP_ICONSET/icon_32x32.png" &>/dev/null
    sips -z 64 64 "$LOCAL_LOGO_PATH" --out "$TMP_ICONSET/icon_32x32@2x.png" &>/dev/null
    sips -z 128 128 "$LOCAL_LOGO_PATH" --out "$TMP_ICONSET/icon_128x128.png" &>/dev/null
    sips -z 256 256 "$LOCAL_LOGO_PATH" --out "$TMP_ICONSET/icon_128x128@2x.png" &>/dev/null
    sips -z 256 256 "$LOCAL_LOGO_PATH" --out "$TMP_ICONSET/icon_256x256.png" &>/dev/null
    sips -z 512 512 "$LOCAL_LOGO_PATH" --out "$TMP_ICONSET/icon_256x256@2x.png" &>/dev/null
    sips -z 512 512 "$LOCAL_LOGO_PATH" --out "$TMP_ICONSET/icon_512x512.png" &>/dev/null
    sips -z 1024 1024 "$LOCAL_LOGO_PATH" --out "$TMP_ICONSET/icon_512x512@2x.png" &>/dev/null

    # Convert to icns
    iconutil -c icns "$TMP_ICONSET" -o "$TMP_APP_DIR/Contents/Resources/AppIcon.icns"
    rm -rf "$TMP_ICONSET"

    # Remove any existing installation
    if [ -d "$INSTALL_DIR" ]; then
      sudo rm -rf "$INSTALL_DIR"
    fi

    # Install the app
    sudo cp -R "$TMP_APP_DIR" "$INSTALL_DIR"
    rm -rf "$TMP_APP_DIR"

    # Create symlink for CLI access
    echo "Creating command-line wrapper at $SYMLINK_PATH..."
    sudo rm -f "$SYMLINK_PATH"

    cat << EOF | sudo tee "$SYMLINK_PATH" > /dev/null
#!/bin/bash
if [ \$# -eq 0 ]; then
    open "/Applications/FORKLIFT.app"
else
    open -a "FORKLIFT" "\$@"
fi
EOF

    sudo chmod +x "$SYMLINK_PATH"
  fi

  # Clear icon caches
  echo "Refreshing system caches..."
  sudo touch "$INSTALL_DIR"
  sudo killall -HUP Finder
  sudo killall -HUP Dock

  echo "FORKLIFT has been installed to $INSTALL_DIR"
  echo "You can run it with the '$WRAPPER_NAME' command or from your Applications folder"
}

# Launch ZED after installation
launch_zed() {
  echo "Launching FORKLIFT..."
  open "$INSTALL_DIR"
}

# Main execution
echo "🔨 Starting build process..."
build_zed
echo "📦 Installing FORKLIFT..."
install_zed
echo "🚀 Launching FORKLIFT..."
launch_zed

echo "✅ Quick build and install completed successfully!"

#!/bin/bash
#
# quick_build_source.sh - Build and run ZED EDGE in debug mode quickly
#
# This script performs a quick debug build of ZED without extensive checks
# It assumes the repository is already cloned and up to date

set -e  # Exit immediately if a command exits with non-zero status

# Configuration variables
CLONE_DIR="$HOME/Developer/zed"
# Correct path for the debug binary
BINARY_PATH="$CLONE_DIR/target/debug/zed"
# Set to "true" if you want to save logs to a file
SAVE_LOGS="false"

# Color codes for prettier output
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[0;33m'
RED='\033[0;31m'
CYAN='\033[0;36m'
MAGENTA='\033[0;35m'
NC='\033[0m' # No Color

# Function to print colored status messages
print_status() {
  local symbol="$1"
  local message="$2"
  local color="$3"
  echo -e "${color}${symbol} ${message}${NC}"
}

# Function to verify the build environment
verify_environment() {
  print_status "🔍" "Verifying build environment..." "$BLUE"

  if [ ! -d "$CLONE_DIR" ]; then
    print_status "❌" "Repository directory does not exist at $CLONE_DIR" "$RED"
    print_status "ℹ️" "Please ensure the Zed source code is cloned at $CLONE_DIR." "$YELLOW"
    exit 1
  fi

  # Check if cargo is available
  if ! command -v cargo &> /dev/null; then
    print_status "❌" "Rust and Cargo are required but not found." "$RED"
    exit 1
  fi

  print_status "✅" "Build environment verified." "$GREEN"
}

# Function to build ZED in debug mode
build_zed() {
  print_status "🔨" "Building ZED in debug mode..." "$BLUE"
  cd "$CLONE_DIR"

  # Set base environment variables for faster compilation
  export RUSTFLAGS="-C target-cpu=native"
  export CARGO_INCREMENTAL=1 # Default for debug, but explicit

  # Try to use a faster linker if available (zld on macOS)
  if [[ "$(uname)" == "Darwin" ]]; then
    if command -v zld &> /dev/null; then
      print_status "ℹ️" "Using zld linker for faster linking." "$YELLOW"
      export RUSTFLAGS="$RUSTFLAGS -C link-arg=-fuse-ld=zld"
    else
      print_status "⚠️" "zld linker not found. Using default linker. Install zld for potentially faster builds (e.g., 'brew install michaeleisel/zld/zld')." "$YELLOW"
    fi
  # Add similar check for lld on Linux if needed
  # elif [[ "$(uname)" == "Linux" ]]; then
  #   if command -v lld &> /dev/null; then
  #     print_status "ℹ️" "Using lld linker for faster linking." "$YELLOW"
  #     export RUSTFLAGS="$RUSTFLAGS -C link-arg=-fuse-ld=lld"
  #   else
  #     print_status "⚠️" "lld linker not found. Using default linker." "$YELLOW"
  #   fi
  fi

  # Detect number of CPU cores for optimal parallel build
  cores=$(sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo 4) # Added nproc for Linux compatibility
  print_status "ℹ️" "Using $cores CPU cores for build..." "$YELLOW"

  # Run the build in debug mode
  print_status "⏳" "Building... (this may take a while on first run)" "$YELLOW"
  # RUSTC_BOOTSTRAP=1 is needed for Zed due to unstable features
  RUSTC_BOOTSTRAP=1 cargo build -j "$cores"

  if [ $? -ne 0 ]; then
    print_status "❌" "Build failed." "$RED"
    exit 1
  fi

  print_status "✅" "Build completed successfully." "$GREEN"
}

# Function to filter and colorize Copilot chat logs
filter_copilot_logs() {
  # First, let's see ALL logs to debug what's being output
  if [ "${DEBUG_ALL_LOGS:-false}" = "true" ]; then
    echo -e "${YELLOW}[DEBUG MODE] Showing ALL logs to debug...${NC}"
    cat
    return
  fi

  # More permissive filtering - look for copilot in any case
  grep --line-buffered -iE "(copilot|github.*copilot|chat.*completion|stream.*completion)" | while IFS= read -r line; do
    # Color different log levels
    if echo "$line" | grep -iq "error"; then
      echo -e "${RED}[COPILOT ERROR]${NC} $line"
    elif echo "$line" | grep -iq "warn"; then
      echo -e "${YELLOW}[COPILOT WARN]${NC} $line"
    elif echo "$line" | grep -iq "initiating.*copilot"; then
      echo -e "${GREEN}[COPILOT REQUEST]${NC} $line"
    elif echo "$line" | grep -iq "request.*copilot"; then
      echo -e "${CYAN}[REQUEST DETAILS]${NC} $line"
    elif echo "$line" | grep -iq "payload"; then
      echo -e "${MAGENTA}[HTTP PAYLOAD]${NC} $line"
    elif echo "$line" | grep -iq "headers"; then
      echo -e "${BLUE}[HEADERS]${NC} $line"
    elif echo "$line" | grep -iq "response.*status"; then
      echo -e "${GREEN}[RESPONSE STATUS]${NC} $line"
    elif echo "$line" | grep -iq "response.*body"; then
      echo -e "${CYAN}[RESPONSE BODY]${NC} $line"
    elif echo "$line" | grep -iq "streaming.*chunk"; then
      echo -e "${YELLOW}[STREAMING CHUNK]${NC} $line"
    elif echo "$line" | grep -iq "token"; then
      echo -e "${MAGENTA}[API TOKEN]${NC} $line"
    else
      echo -e "${NC}[COPILOT]${NC} $line"
    fi
  done
}

# Function to run ZED in debug mode
run_zed() {
  print_status "🚀" "Running ZED EDGE in debug mode..." "$BLUE"

  # Check if the binary exists before trying to run it
  if [ ! -f "$BINARY_PATH" ]; then
      print_status "❌" "Zed binary not found at $BINARY_PATH" "$RED"
      print_status "ℹ️" "Build might have failed or the path is incorrect." "$YELLOW"
      exit 1
  fi

  # Set log levels to show Copilot logs with maximum detail
  export RUST_LOG=copilot=debug,copilot_chat=debug,language_models::provider=debug

  print_status "🔍" "Enabled debug logging for all components (filtering for Copilot)" "$YELLOW"
  print_status "📋" "Log Legend:" "$CYAN"
  print_status "  🟢" "REQUEST - New chat completion request initiated" "$GREEN"
  print_status "  🔵" "HEADERS - HTTP request/response headers" "$BLUE"
  print_status "  🟣" "PAYLOAD - JSON request payload sent to API" "$MAGENTA"
  print_status "  🟡" "STREAMING - Real-time response chunks" "$YELLOW"
  print_status "  🔴" "ERROR - Error responses and failures" "$RED"
  print_status "  🔷" "DETAILS - Request parameters and response bodies" "$CYAN"
  echo ""

  # Check if we want to save logs to a file
  if [ "$SAVE_LOGS" = "true" ]; then
    LOG_FILE="$HOME/zed_copilot_chat.log"
    print_status "📝" "Saving filtered Copilot logs to $LOG_FILE" "$YELLOW"
    "$BINARY_PATH" 2>&1 | filter_copilot_logs | tee "$LOG_FILE"
  else
      export RUST_LOG=copilot=debug,copilot_chat=debug,language_models::provider=debug

    # Run the binary with filtered logging for Copilot chat only
    print_status "🎯" "Showing ONLY Copilot chat HTTP requests/responses..." "$CYAN"
    echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    "$BINARY_PATH"
  fi
}

# Main function
main() {
  print_status "🔄" "Starting ZED EDGE quick debug build with Copilot chat logging..." "$BLUE"

  verify_environment
  build_zed
  run_zed

  print_status "✅" "Quick debug build and run completed!" "$GREEN"
}

# Execute main function
main

#!/bin/bash
# Build and Developer ID-sign Sumaru; optionally notarize its ZIP archive.
set -euo pipefail

usage() {
    cat <<'EOF'
Usage: scripts/release.sh [universal|arm64|x86_64] [--notarize]

Default: universal (native Intel and Apple Silicon in one executable).
Outputs: target/distribution/<architecture>/sumaru and sumaru-macos-<architecture>.zip

Environment overrides:
  SIGN_ID         Developer ID Application certificate name or SHA-1 hash
  NOTARY_PROFILE  notarytool Keychain profile (default: eva-notary)
  SIGN_IDENTIFIER  Code-signing identifier (default: io.github.pmolfese.sumaru)
  RUSTUP_TOOLCHAIN  Rustup toolchain (default: active toolchain for this repository)

Requires macOS, Xcode tools, Rust >= 1.99, and a Developer ID private key.
Missing Rust targets are installed through rustup. --notarize uploads to Apple.
Standalone executables and ZIPs cannot be stapled; ticket lookup needs a network.
EOF
}

ARCH=universal
ARCH_SET=false
NOTARIZE=false
for arg in "$@"; do
    case "$arg" in
        universal|arm64|x86_64)
            if "$ARCH_SET"; then echo 'error: specify only one architecture' >&2; exit 1; fi
            ARCH="$arg"
            ARCH_SET=true
            ;;
        --notarize) NOTARIZE=true ;;
        -h|--help) usage; exit 0 ;;
        *) echo "error: unknown argument: $arg" >&2; usage >&2; exit 1 ;;
    esac
done

log() { printf '\n==> %s\n' "$1"; }
fail() { echo "error: $1" >&2; exit 1; }

[[ "$(uname -s)" == Darwin ]] || fail 'run this script on macOS'
for tool in rustup security codesign xcrun ditto lipo; do
    command -v "$tool" >/dev/null || fail "required tool not found: $tool"
done
xcrun --sdk macosx --show-sdk-path >/dev/null

SIGN_ID="${SIGN_ID:-Developer ID Application: Peter Molfese (7V8RRF84QH)}"
NOTARY_PROFILE="${NOTARY_PROFILE:-eva-notary}"
SIGN_IDENTIFIER="${SIGN_IDENTIFIER:-io.github.pmolfese.sumaru}"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

# Homebrew cargo/rustc may precede rustup's proxies on PATH. Use one rustup
# toolchain for target installation, Cargo, and the compiler Cargo invokes.
TOOLCHAIN="$(rustup show active-toolchain | awk '{print $1}')"
[[ -n "$TOOLCHAIN" ]] || fail 'no active rustup toolchain; install one with rustup toolchain install stable'
export RUSTC="$(rustup which --toolchain "$TOOLCHAIN" rustc)"
export RUSTDOC="$(rustup which --toolchain "$TOOLCHAIN" rustdoc)"
log "Using Rust toolchain: $TOOLCHAIN"
"$RUSTC" --version

# Match the exact certificate name or hash, and require a Developer ID identity.
IDENTITIES="$(security find-identity -v -p codesigning)"
if ! awk -v identity="$SIGN_ID" '
    /"Developer ID Application:/ {
        name = $0
        sub(/^[^"]*"/, "", name)
        sub(/"[[:space:]]*$/, "", name)
        if (name == identity || $2 == identity) found = 1
    }
    END { exit !found }
' <<< "$IDENTITIES"; then
    fail "valid signing identity not found: $SIGN_ID. Check security find-identity -v -p codesigning in your Terminal."
fi

if "$NOTARIZE"; then
    xcrun --find notarytool >/dev/null
    # Preserve the real error: failure can mean credentials, network, or service issues.
    xcrun notarytool history --keychain-profile "$NOTARY_PROFILE" >/dev/null ||
        fail "could not access notarization using profile '$NOTARY_PROFILE'; see the error above"
fi

case "$ARCH" in
    universal) TARGETS=(aarch64-apple-darwin x86_64-apple-darwin) ;;
    arm64) TARGETS=(aarch64-apple-darwin) ;;
    x86_64) TARGETS=(x86_64-apple-darwin) ;;
esac

# Explicit target-dir keeps artifact paths predictable despite Cargo configuration.
CARGO_OUTPUT="$ROOT_DIR/target"
INPUTS=()
for target in "${TARGETS[@]}"; do
    log "Building $target"
    rustup target add --toolchain "$TOOLCHAIN" "$target"
    rustup run "$TOOLCHAIN" cargo build --release --locked --bin sumaru --target "$target" --target-dir "$CARGO_OUTPUT"
    INPUTS+=("$CARGO_OUTPUT/$target/release/sumaru")
done

# Use a fresh directory for each run so a failed run cannot reuse an old ZIP.
OUTPUT_PARENT="$CARGO_OUTPUT/distribution"
mkdir -p "$OUTPUT_PARENT"
STAGE="$(mktemp -d "$OUTPUT_PARENT/.${ARCH}.XXXXXX")"
trap 'rm -rf "$STAGE"' EXIT
BIN_PATH="$STAGE/sumaru"
ZIP_NAME="sumaru-macos-$ARCH.zip"
if [[ "$ARCH" == universal ]]; then
    lipo -create "${INPUTS[@]}" -output "$BIN_PATH"
else
    cp "${INPUTS[0]}" "$BIN_PATH"
fi
chmod 755 "$BIN_PATH"
lipo -archs "$BIN_PATH"

log 'Signing and verifying executable'
codesign --force --sign "$SIGN_ID" --identifier "$SIGN_IDENTIFIER" \
    --options runtime --timestamp "$BIN_PATH"
codesign --verify --strict --all-architectures --verbose=2 "$BIN_PATH"
codesign --display --verbose=4 "$BIN_PATH"
ditto -c -k --keepParent "$BIN_PATH" "$STAGE/$ZIP_NAME"

if "$NOTARIZE"; then
    log 'Submitting ZIP to Apple for notarization'
    RESULT="$STAGE/notarization.json"
    SUBMIT_EXIT=0
    xcrun notarytool submit "$STAGE/$ZIP_NAME" --keychain-profile "$NOTARY_PROFILE" \
        --wait --output-format json > "$RESULT" || SUBMIT_EXIT=$?
    cat "$RESULT"
    STATUS="$(plutil -extract status raw -o - "$RESULT" 2>/dev/null || true)"
    if [[ "$SUBMIT_EXIT" != 0 || "$STATUS" != Accepted ]]; then
        # Keep the submission ID/status for diagnosis without publishing a release.
        cp "$RESULT" "$OUTPUT_PARENT/$ARCH-notarization-failed.json"
        fail "notarization did not succeed; use the submission ID above with xcrun notarytool log ID --keychain-profile $NOTARY_PROFILE"
    fi
fi

OUTPUT_DIR="$OUTPUT_PARENT/$ARCH"
mkdir -p "$OUTPUT_DIR"
# Remove only this script's previous results after all requested steps succeed.
rm -f "$OUTPUT_DIR/notarization.json"
mv -f "$BIN_PATH" "$OUTPUT_DIR/sumaru"
mv -f "$STAGE/$ZIP_NAME" "$OUTPUT_DIR/$ZIP_NAME"
if "$NOTARIZE"; then
    mv -f "$RESULT" "$OUTPUT_DIR/notarization.json"
    log "Signed and notarized: $OUTPUT_DIR/$ZIP_NAME"
else
    log "Signed (not notarized): $OUTPUT_DIR/$ZIP_NAME"
fi
printf 'Executable: %s\n' "$OUTPUT_DIR/sumaru"

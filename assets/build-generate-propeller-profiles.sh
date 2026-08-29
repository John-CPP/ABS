#!/usr/bin/env bash
# Build generate_propeller_profiles against the system LLVM (required for
# SHT_LLVM_BB_ADDR_MAP v5 / LLVM 22+ kernels). create_llvm_prof 0.30 cannot.
#
# Usage: build-generate-propeller-profiles.sh <cache-root>
# Writes: <cache-root>/bin/generate_propeller_profiles
set -euo pipefail

DEST_ROOT=${1:?cache root required}
SRC="$DEST_ROOT/src"
BUILD="$DEST_ROOT/build"
BIN="$DEST_ROOT/bin/generate_propeller_profiles"
REPO_URL=${ABS_LLVM_PROPELLER_GIT:-https://github.com/google/llvm-propeller.git}

if [[ -x "$BIN" && "${ABS_PROPELLER_REBUILD:-}" != "1" ]]; then
  echo "Using existing $BIN"
  exit 0
fi

need() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "PGO Propeller bootstrap needs '$1' in PATH. On Arch/CachyOS:" >&2
    echo "  sudo pacman -S --needed cmake ninja clang llvm git libelf" >&2
    exit 1
  }
}
need git
need cmake
need ninja
need clang
need clang++
if ! command -v llvm-config >/dev/null 2>&1 && ! command -v llvm-config-22 >/dev/null 2>&1; then
  echo "PGO Propeller bootstrap needs llvm-config (package 'llvm')." >&2
  echo "  sudo pacman -S --needed llvm clang cmake ninja git libelf" >&2
  exit 1
fi

LLVM_CONFIG=$(command -v llvm-config || command -v llvm-config-22)
LLVM_MAJOR=$("$LLVM_CONFIG" --version | cut -d. -f1)
echo "Bootstrapping generate_propeller_profiles against LLVM $("$LLVM_CONFIG" --version)"

mkdir -p "$DEST_ROOT"
if [[ ! -d "$SRC/.git" ]]; then
  rm -rf "$SRC"
  git clone --depth 1 "$REPO_URL" "$SRC"
fi

# Force system LLVM. Upstream CMake downloads a pinned LLVM that cannot read BB_ADDR_MAP v5.
cat > "$SRC/CMake/LLVM/LLVM.cmake" << 'EOF'
find_package(LLVM REQUIRED CONFIG)
message(STATUS "ABS: using system LLVM ${LLVM_PACKAGE_VERSION} at ${LLVM_DIR}")
include_directories(${LLVM_INCLUDE_DIRS})
if(LLVM_DEFINITIONS)
  separate_arguments(_ABS_LLVM_DEFS NATIVE_COMMAND ${LLVM_DEFINITIONS})
  add_definitions(${_ABS_LLVM_DEFS})
endif()
if(LLVM_LIBRARY_DIRS)
  link_directories(${LLVM_LIBRARY_DIRS})
endif()
EOF

patch_propeller_cmake() {
  local cmake=$1
  if ! grep -qE 'LLVMDebugInfoDWARF|LLVM\$\{tgt\}\$\{tool\}' "$cmake"; then
    return 0
  fi
  local tmp
  tmp=$(mktemp)
  awk '
    BEGIN { skip=0; ends=0; done=0 }
    skip == 0 && done == 0 && $0 ~ /target_link_libraries\(propeller_lib/ {
      print "target_link_libraries(propeller_lib"
      print "  LLVM"
      print "  absl::base"
      print "  propeller_protos"
      print "  quipper_lib"
      print "  quipper_protos"
      print ")"
      print "# ABS: Arch/CachyOS ships libLLVM.so; component LLVM*.a names are unavailable."
      skip=1
      ends=0
      next
    }
    skip {
      if ($0 ~ /endforeach\(\)/) ends++
      if (ends >= 2) { skip=0; done=1 }
      next
    }
    { print }
    END { if (done != 1) exit 1 }
  ' "$cmake" >"$tmp" || {
    rm -f "$tmp"
    echo "failed to rewrite propeller/CMakeLists.txt LLVM link libraries" >&2
    exit 1
  }
  mv "$tmp" "$cmake"
}

patch_mini_disassembler() {
  local cc=$1
  local major=$2
  if [[ $major -lt 22 ]] || grep -q 'asm_info_.get()' "$cc"; then
    return 0
  fi
  if ! grep -q '\*disassembler->asm_info_' "$cc"; then
    echo "failed to patch mini_disassembler.cc for LLVM 22 MCContext" >&2
    exit 1
  fi
  sed -i \
    -e 's/\*disassembler->asm_info_/disassembler->asm_info_.get()/g' \
    -e 's/\*disassembler->mri_/disassembler->mri_.get()/g' \
    -e 's/\*disassembler->sti_/disassembler->sti_.get()/g' \
    "$cc"
}

patch_propeller_cmake "$SRC/propeller/CMakeLists.txt"
patch_mini_disassembler "$SRC/propeller/mini_disassembler.cc" "$LLVM_MAJOR"

cmake -G Ninja -S "$SRC" -B "$BUILD" \
  -DCMAKE_BUILD_TYPE=Release \
  -DBUILD_TESTING=OFF \
  -DCMAKE_C_COMPILER=clang \
  -DCMAKE_CXX_COMPILER=clang++
ninja -C "$BUILD" generate_propeller_profiles

found=$(find "$BUILD" -name generate_propeller_profiles -type f -executable | head -n1)
if [[ -z "$found" ]]; then
  echo "ninja finished but generate_propeller_profiles was not produced" >&2
  exit 1
fi
mkdir -p "$(dirname "$BIN")"
install -m755 "$found" "$BIN"
echo "Installed $BIN"

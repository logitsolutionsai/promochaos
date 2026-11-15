#!/usr/bin/env sh
# Builds the promochaos examples:
#   target/debug/homegrown_cart                                         command target, seeded bugs (--fixed at run time)
#   examples/functions/tiered-discount-rust/dist/function.wasm          Rust Function, seeded bugs
#   examples/functions/tiered-discount-rust/dist/function-fixed.wasm    Rust Function, fixed
#   examples/functions/bundle-discount-js/dist/function.wasm            JS Function, seeded bugs (needs the Shopify CLI)
#   examples/functions/bundle-discount-js/dist/function-fixed.wasm      JS Function, fixed (needs the Shopify CLI)
#
# Run from anywhere. Stops on the first error.
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
EXAMPLES="$ROOT/examples"
TIERED="$EXAMPLES/functions/tiered-discount-rust"
BUNDLE="$EXAMPLES/functions/bundle-discount-js"
RAW_WASM="wasm32-unknown-unknown/release/tiered-discount-rust.wasm"
BUILT=""

note() { printf '%s\n' "$*"; }
built() {
  BUILT="$BUILT
  $1"
}
have() { command -v "$1" >/dev/null 2>&1; }

# schema.graphql is not committed (we found no licence that allows
# redistributing it). Stop with the fetch command when it is missing.
need_schema() {
  if [ ! -f "$EXAMPLES/functions/$1/schema.graphql" ]; then
    note "Missing examples/functions/$1/schema.graphql. Fetch it (needs a Shopify login): cd examples && shopify app function schema --path functions/$1 --client-id <your app client ID>"
    exit 1
  fi
}

# Wasm API Functions need Shopify's trampoline step before function-runner can
# run them. The Shopify CLI does it during `shopify app function build`.
trampoline() {
  if have shopify-function-trampoline; then
    shopify-function-trampoline -i "$1" -o "$2"
  else
    cp "$1" "$2"
    note "WARNING: shopify-function-trampoline is not on the PATH, so $2 was copied"
    note "         without the trampoline step and function-runner may refuse to run it."
    note "         Install the Shopify CLI, or the trampoline tool from the"
    note "         Shopify/shopify-function-wasm-api releases, and build again."
  fi
}

note "== homegrown-cart"
(cd "$ROOT" && cargo build -p homegrown-cart)
built "target/debug/homegrown_cart"

note "== tiered-discount-rust"
need_schema tiered-discount-rust
if have rustup; then
  if ! rustup target list --installed | grep -q '^wasm32-unknown-unknown$'; then
    note "The wasm32-unknown-unknown target is missing. Run: rustup target add wasm32-unknown-unknown"
    exit 1
  fi
fi
mkdir -p "$TIERED/dist"
# Build the seeded module exactly like the fixed one below (cargo, then the
# trampoline) whenever the trampoline is on the PATH, so the before and after
# instruction counts compare like with like. The Shopify CLI build uses its own
# trampoline and gave about 5 percent fewer instructions on 2026-11-15.
if ! have shopify-function-trampoline && have shopify; then
  (cd "$EXAMPLES" && shopify app function build --path functions/tiered-discount-rust)
  cp "$TIERED/target/$RAW_WASM" "$TIERED/dist/function.wasm"
else
  note "Building with cargo and the trampoline."
  (cd "$TIERED" && cargo build --target=wasm32-unknown-unknown --release)
  trampoline "$TIERED/target/$RAW_WASM" "$TIERED/dist/function.wasm"
fi
built "examples/functions/tiered-discount-rust/dist/function.wasm"

(cd "$TIERED" && cargo build --target=wasm32-unknown-unknown --release --features fixed --target-dir target-fixed)
trampoline "$TIERED/target-fixed/$RAW_WASM" "$TIERED/dist/function-fixed.wasm"
built "examples/functions/tiered-discount-rust/dist/function-fixed.wasm"

note "== bundle-discount-js"
if have shopify; then
  need_schema bundle-discount-js
  if [ ! -d "$BUNDLE/node_modules" ]; then
    (cd "$BUNDLE" && npm install)
  fi
  restore_index() {
    printf '%s\n' "export * from './cart_lines_discounts_generate_run';" > "$BUNDLE/src/index.js"
  }
  trap restore_index EXIT
  cp "$BUNDLE/src/index.fixed.js" "$BUNDLE/src/index.js"
  (cd "$EXAMPLES" && shopify app function build --path functions/bundle-discount-js)
  cp "$BUNDLE/dist/function.wasm" "$BUNDLE/dist/function-fixed.wasm"
  restore_index
  (cd "$EXAMPLES" && shopify app function build --path functions/bundle-discount-js)
  trap - EXIT
  built "examples/functions/bundle-discount-js/dist/function-fixed.wasm"
  built "examples/functions/bundle-discount-js/dist/function.wasm"
else
  note "Skipped: JavaScript Functions are compiled by the Shopify CLI (Javy), which is not on the PATH."
  note "To build them later:"
  note "  npm install -g @shopify/cli"
  note "  cd examples"
  note "  shopify app function schema --path functions/bundle-discount-js --client-id <your app client ID>   # needs a Shopify login"
  note "  cd functions/bundle-discount-js && npm install && cd ../.."
  note "  shopify app function build --path functions/bundle-discount-js        # buggy, dist/function.wasm"
  note "  copy src/index.fixed.js over src/index.js, build again, rename dist/function.wasm"
  note "  to dist/function-fixed.wasm, restore src/index.js and build once more"
fi

note ""
note "Built:$BUILT"

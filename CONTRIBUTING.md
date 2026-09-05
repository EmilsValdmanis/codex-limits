# Contributing

Thanks for helping improve Codex Limits.

## Development setup

You need a current Rust toolchain, Node.js, `zip`, and Linux x86_64. Clone the
repository and run:

```bash
cargo test --locked
```

Before opening a pull request, run the complete local gate:

```bash
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
node --check assets/propertyInspector/inspector.js
node --test tests/inspector.test.cjs
./scripts/package.sh
```

## Performance measurements

Run the optional microbenchmarks in release mode, one at a time:

```bash
cargo test --release --locked benchmark_ -- --ignored --nocapture --test-threads=1
```

They report the median of seven samples. Rendering covers six tile states and
includes PNG encoding and base64. Polling covers 32 visible tiles sharing four
accounts. These measure local CPU work, excluding OpenDeck transport, Codex
startup, and network latency; they are not timing assertions in the test suite.

The bitmap-rendering optimization measured 279 → 217 µs per tile on the
development machine (about 22% less time). Polling measured about 1.3 µs per pass.
Compare runs on the same machine under similar load.

## Testing with OpenDeck

Build the package with `./scripts/package.sh`, install the resulting
`.streamDeckPlugin` file in OpenDeck, and test at least one signed-in account.
When changing account lifecycle or cache behavior, also test two tiles sharing
one `CODEX_HOME`.

Never add Codex credentials, `auth.json`, account email addresses, or generated
OpenDeck logs to fixtures or bug reports. Fixtures should contain synthetic
account data only.

## Pull requests

Keep changes focused, explain user-visible behavior, and include tests for
protocol parsing, cache behavior, or rendering changes. Screenshots from the
physical device are especially useful for tile-layout changes.

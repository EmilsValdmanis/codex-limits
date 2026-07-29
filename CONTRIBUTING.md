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
./scripts/package.sh
```

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

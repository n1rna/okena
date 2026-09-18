# An okena extension

Start a new extension by copying this folder (into its own repository, or as
`extensions/<id>/` in a library repository), then:

1. Rename `id`, `name` and `description` in `extension.toml`, and the crate in `Cargo.toml`.
2. Declare the commands and paths it needs under `[permissions]`, the tools it
   depends on under `[[requires]]`, and its settings under `[[config]]`.
3. Write `refresh` (the view), and `describe` / `run_action` / `query` if it
   has actions or queries.
4. Build: `cargo build --release --target wasm32-wasip2` (the
   `rust-toolchain.toml` installs the target).
5. In okena: Settings → Extensions → Install an extension → From a local folder,
   pick this folder, review and approve. After an edit, **Rebuild & reload**.

To ship a prebuilt component, copy `target/wasm32-wasip2/release/<crate>.wasm`
to `extension.wasm` here and commit it.

The reference: `docs/reference/extensions.md` in the okena repository.

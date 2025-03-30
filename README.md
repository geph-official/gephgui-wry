# gephgui-wry

`gephgui-wry` is the desktop GUI for Geph. It works on Windows, Mac, and Linux.

To compile `gephgui-wry`:

1. Clone the `gephgui-wry` repository with `git clone --recursive` (this is to also clone the `gephgui` submodule inside it, which is the html that the webview app hosts)
2. Install platform-specific wry dependencies; instructions on this page: https://lib.rs/crates/wry
3. Install a `JavaScript` package manager, then `cd` into the `gephgui` git submodule and build `gephgui`. With `npm`:
  ```shell!
  npm i; npm run build
  ```
4. Back in the `gephgui-wry` directory:
  ```shell!
  cargo install --path .
  ```
5. Now run `gephgui-wry` in any terminal to start the Geph desktop GUI!

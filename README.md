# gephgui-wry

To compile and run `gephgui-wry`:

1. Clone the `gephgui-wry` repository with `git clone --recursive` (this is to also clone the `gephgui` submodule inside it)
2. Comment out autoconfiguring the proxy in [`rpc_handlers.rs`](https://github.com/geph-official/gephgui-wry/blob/master/src/rpc_handler.rs#L146). This is because the `pac` file required is currently not available in the repo.
3. Install `geph4-client`:
  ```shell!
  cargo install geph4-client
  ```
4. Install a `JavaScript` package manager, then `cd` into the `gephgui` git submodule and build `gephgui`. With `npm`:
  ```shell!
  npm i; npm run build
  ```
5. Back in the `gephgui-wry` directory:
  ```shell!
  cargo run
  ```

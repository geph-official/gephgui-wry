document.addEventListener("click", (e) => {
  if (e.target.matches("a")) {
    e.preventDefault();
    if (e.target.getAttribute("target") === "_blank") {
      window.rpc.call("open_browser", e.target.getAttribute("href"));
    }
  }
});
window.open = (url) => window.rpc.call("open_browser", url);

function convertRemToPixels(rem) {
  return rem * parseFloat(getComputedStyle(document.documentElement).fontSize);
}

addEventListener("load", (_) => {
  window["rpc"].call("set_conversion_factor", convertRemToPixels(1) / 15);
});

let running = false;
let connected = false;

window["NATIVE_GATE"] = {
  async start_daemon(params) {
    await window.rpc.call("start_daemon", params);
    while (true) {
      try {
        await this.is_connected();
        break;
      } catch (e) {
        console.error("start daemon", e);
        await new Promise((r) => setTimeout(r, 200));
        continue;
      }
    }
  },
  async stop_daemon() {
    await window.rpc.call("stop_daemon", []);
  },
  async is_connected() {
    return await this.daemon_rpc("is_connected", []);
  },
  async is_running() {
    try {
      await this.daemon_rpc("is_connected", []);
      return true;
    } catch {
      return false;
    }
  },
  sync_user_info: async (username, password) => {
    let sync_info = JSON.parse(
      await window.rpc.call("sync", username, password, false)
    );
    if (sync_info.user.subscription)
      return {
        level: sync_info.user.subscription.level.toLowerCase(),
        expires: sync_info.user.subscription
          ? new Date(sync_info.user.subscription.expires_unix * 1000.0)
          : null,
      };
    else return { level: "free", expires: null };
  },

  daemon_rpc: async (method, args) => {
    const req = { jsonrpc: "2.0", method: method, params: args, id: 1 };
    const resp = JSON.parse(
      await window.rpc.call("daemon_rpc", JSON.stringify(req))
    );
    if (resp.error) {
      throw resp.error.message;
    }
//    console.log("DAEMON RESULT", resp);
    return resp.result;
  },

  binder_rpc: async (method, args) => {
    const req = { jsonrpc: "2.0", method: method, params: args, id: 1 };
    const resp = JSON.parse(
      await window.rpc.call("binder_rpc", JSON.stringify(req))
    );
    if (resp.error) {
      throw resp.error.message;
    }
    console.log("BINDER RESULT", resp);
    return resp.result;
  },

  sync_exits: async (username, password) => {
    let sync_info = JSON.parse(
      await window.rpc.call("sync", username, password, false)
    );
    return sync_info.exits;
  },

  async purge_caches(username, password) {
    await window.rpc.call("sync", username, password, true);
  },

  async export_debug_pack() {
    await window.rpc.call("export_logs");
  },

  supports_app_whitelist: false,
  supports_prc_whitelist: true,
  supports_proxy_conf: true,
  supports_listen_all: true,
  supports_vpn_conf: true,
  supports_autoupdate: true,
  async get_native_info() {
    return {
      platform_type: "desktop",
      platform_details: "Desktop",
      version: await window.rpc.call("version"),
    };
  },
};

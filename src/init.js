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
  window["rpc"].call("set_conversion_factor", convertRemToPixels(1) / 16);
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
    await this.daemon_rpc("kill", []);
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
      await window.rpc.call("sync", username, password)
    );
    return {
      level: sync_info.user.subscription.level.toLowerCase(),
      expires: sync_info.user.subscription
        ? new Date(sync_info.user.subscription.expires_unix * 1000.0)
        : null,
    };
  },

  daemon_rpc: async (method, args) => {
    const req = { jsonrpc: "2.0", method: method, params: args, id: 1 };
    const resp = JSON.parse(
      await window.rpc.call("daemon_rpc", JSON.stringify(req))
    );
    if (resp.error) {
      throw resp.error.message;
    }
    console.log("DAEMON RESULT", resp);
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
      await window.rpc.call("sync", username, password)
    );
    return sync_info.exits;
  },

  sync_app_list: async () => {
    return [
      {
        id: "com.tencent.mm",
        friendly_name: "WeChat",
      },
      {
        id: "com.tencent.mmm",
        friendly_name: "MeChat",
      },
    ];
  },

  get_app_icon_url: async (id) => {
    return "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACAAAAAgCAMAAABEpIrGAAAAM1BMVEWA2HEdwgUkwwBCxS5RxT1YyE5szGJ/0HiQ1YmZ2JSh2p2v363B5b7R68/b79js9+z///9HPSCbAAAAAXRSTlMAQObYZgAAAOxJREFUOMuFk1cWhCAMRWmhCYT9r3akSRv0fciRXEIKIYQQxuhfMUaSDtbK3AB91YeD5KIC4gp4y+n1QLmBu9iE+g8gMQ5ybAVgst/ECoS4SM+AypuVwuQNZyAHaMoSCi4nIEdw8ewChUmL3YEuvJQAfgTQSJfD8PoBxiRQ9pIFqIAd7X6koQC832GuKZxQC6X6kVSlDNVv7YVuNU67Mv/JnK5v3VTlFuuXmmMDaib6CLAYFAUF7gRIW96Ajlvjlze7dB42YH5blm5Ay+exbwAFP0SYgH0uwDrntFAeDkAfmjJ7X6P3PbwvSDL/AIYAHEpiL5B+AAAAAElFTkSuQmCC";
  },

  supports_app_whitelist: true,
  supports_proxy_conf: true,
  supports_vpn_conf: true,
  native_info: {
    platform_type: "linux",
    platform_details: "MockLinux Trololol",
    daemon_version: "0.0.0-mock",
  },
};

document.addEventListener("click", (e) => {
  // Use closest to find the nearest ancestor <a> element
  const anchor = e.target.closest("a");

  // If an <a> element is found within the clicked element's hierarchy
  if (anchor) {
    e.preventDefault(); // Prevent the default link behavior

    // Check if the <a> has target="_blank"
    if (anchor.getAttribute("target") === "_blank") {
      // Call the RPC method with the href of the <a>
      jsonrpc_call("open_browser", anchor.getAttribute("href"));
    }
  }
});

window.open = (url) => jsonrpc_call("open_browser", url);

function convertRemToPixels(rem) {
  return rem * parseFloat(getComputedStyle(document.documentElement).fontSize);
}

let running = false;
let connected = false;

window["NATIVE_GATE"] = {
  async start_daemon(params) {
    await jsonrpc_call("start_daemon", params);
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
    await jsonrpc_call("stop_daemon", []);
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
      await jsonrpc_call("sync", username, password, false)
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
      await jsonrpc_call("daemon_rpc", JSON.stringify(req))
    );
    if (resp.error) {
      throw resp.error.message;
    }

    return resp.result;
  },

  binder_rpc: async (method, args) => {
    const req = { jsonrpc: "2.0", method: method, params: args, id: 1 };
    const resp = JSON.parse(
      await jsonrpc_call("binder_rpc", JSON.stringify(req))
    );
    if (resp.error) {
      throw resp.error.message;
    }
    console.log("BINDER RESULT", resp);
    return resp.result;
  },

  sync_exits: async (username, password) => {
    let sync_info = JSON.parse(
      await jsonrpc_call("sync", username, password, false)
    );
    return sync_info.exits;
  },

  async purge_caches(username, password) {
    await jsonrpc_call("sync", username, password, true);
  },

  async export_debug_pack() {
    await jsonrpc_call("export_logs");
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
      version: await jsonrpc_call("version"),
    };
  },
};

let rpc_count = 0;

async function raw_jsonrpc_call(inner) {
  rpc_count += 1;
  const promise = new Promise((resolve) => {
    const callback_name = "callback" + rpc_count;
    window[callback_name] = (response) => {
      resolve(response);
    };
    const ipc_string = JSON.stringify({
      callback_code: callback_name,
      inner,
    });
    window.ipc.postMessage(ipc_string);
  });
  const res = await promise;
  if (res.error) {
    throw res.error.message;
  }
  return res.result;
}

async function jsonrpc_call(method, ...params) {
  return await raw_jsonrpc_call({
    jsonrpc: "2.0",
    method,
    params,
    id: 1,
  });
}

document.addEventListener(
  "click",
  (e) => {
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
  },
  true
);

window.open = (url) => jsonrpc_call("open_browser", url);

function convertRemToPixels(rem) {
  return rem * parseFloat(getComputedStyle(document.documentElement).fontSize);
}

let running = false;
let connected = false;

const hardcodedProps = {
  supports_app_whitelist: false,
  supports_prc_whitelist: true,
  supports_proxy_conf: true,
  supports_listen_all: true,
  supports_vpn_conf: false,
  supports_autoupdate: true,
};

window["NATIVE_GATE"] = new Proxy(hardcodedProps, {
  get(target, propKey, receiver) {
    // If this is one of the hardcoded properties (or a Symbol, or native property),
    // return it as-is.
    if (propKey in target) {
      return Reflect.get(target, propKey, receiver);
    }

    if (propKey === "then") {
      return window["NATIVE_GATE"];
    }

    // Otherwise, assume it's a method name and return an async function for jsonrpc_call.
    return (...args) => jsonrpc_call(propKey, ...args);
  },
});

let rpc_count = 0;

async function raw_jsonrpc_call(inner) {
  console.log("call", inner);
  rpc_count += 1;
  const promise = new Promise((resolve) => {
    const callback_name = "callback" + rpc_count;
    window[callback_name] = (response) => {
      delete window[callback_name];
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
  console.log("call", method, params);
  return await raw_jsonrpc_call({
    jsonrpc: "2.0",
    method,
    params,
    id: 1,
  });
}

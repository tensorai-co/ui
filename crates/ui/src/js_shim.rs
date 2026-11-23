//! Unified browser-side initialization script (`window.ipc` / `ipc` + drag-region handling).

/// Startup JS injected at document creation on both `webkit` and `cef`.
///
/// Uses the reserved `ui-ipc://ipc/` custom URL scheme with `fetch` + binary
/// `ArrayBuffer` framing for RPC (`/rpc`), fire-and-forget (`/send`), and
/// coalesced host-to-renderer event batches (`/drain`). On `webkit` `https://`
/// pages where WebKit's Mixed Content policy blocks custom-scheme `fetch`, it
/// automatically tunnels the exact same binary packets over `webkit.messageHandlers.ui`.
pub(crate) const SHIM_JS: &str = r#"(function() {
  if (window.ipc && window.__ipc_wake) return;

  const IPC_BASE = "ui-ipc://ipc";
  const WIRE_NULL = 0x00;
  const WIRE_JSON_UTF8 = 0x01;
  const WIRE_BYTES = 0x02;
  const WIRE_ARRAY = 0x03;
  const WIRE_OBJECT = 0x04;
  const SEND_OP_EVENT = 0x01;
  const SEND_OP_MSG = 0x02;
  const SEND_OP_DRAG = 0x03;
  const SEND_OP_TITLEBAR_DOUBLE_CLICK = 0x04;
  const SEND_OP_CANCEL = 0x05;
  const SEND_OP_STREAM_DATA = 0x06;
  const RPC_FLAG_CALL_ID = 0x01;
  const RPC_FLAG_STREAM = 0x02;
  const STREAM_DATA_PREFIX = "__ui_stream:";
  const STREAM_END_PREFIX = "__ui_stream_end:";

  const enc = new TextEncoder();
  const dec = new TextDecoder();
  const listeners = new Map();
  const pendingFallback = new Map();
  let nextFallbackId = 1;
  let nextCallId = 1;
  let hasAnyListener = false;
  let draining = false;
  let wakeAgain = false;

  const scheduleMicrotask = typeof queueMicrotask === "function"
    ? queueMicrotask
    : (fn) => Promise.resolve().then(fn);

  function useWkFallback() {
    return window.location.protocol === "https:" &&
      !!(window.webkit && window.webkit.messageHandlers && window.webkit.messageHandlers.ui);
  }

  function isBinary(v) {
    return v instanceof ArrayBuffer || ArrayBuffer.isView(v);
  }

  function toUint8(v) {
    if (v instanceof ArrayBuffer) return new Uint8Array(v);
    return new Uint8Array(v.buffer, v.byteOffset, v.byteLength);
  }

  function hasBytes(v, depth) {
    if (!v || typeof v !== "object" || depth > 64) return false;
    if (isBinary(v)) return true;
    if (Array.isArray(v)) {
      for (let i = 0; i < v.length; i++) {
        if (hasBytes(v[i], depth + 1)) return true;
      }
      return false;
    }
    const keys = Object.keys(v);
    for (let i = 0; i < keys.length; i++) {
      if (hasBytes(v[keys[i]], depth + 1)) return true;
    }
    return false;
  }

  function encodeWireValue(val) {
    if (val === null || val === undefined) {
      return new Uint8Array([WIRE_NULL]);
    }
    if (isBinary(val)) {
      const view = toUint8(val);
      const out = new Uint8Array(1 + view.byteLength);
      out[0] = WIRE_BYTES;
      out.set(view, 1);
      return out;
    }
    if (Array.isArray(val) && hasBytes(val, 0)) {
      const parts = new Array(val.length);
      let total = 5;
      for (let i = 0; i < val.length; i++) {
        const part = encodeWireValue(val[i]);
        parts[i] = part;
        total += 4 + part.byteLength;
      }
      const out = new Uint8Array(total);
      const dv = new DataView(out.buffer);
      out[0] = WIRE_ARRAY;
      dv.setUint32(1, parts.length, true);
      let pos = 5;
      for (let i = 0; i < parts.length; i++) {
        const part = parts[i];
        dv.setUint32(pos, part.byteLength, true);
        pos += 4;
        out.set(part, pos);
        pos += part.byteLength;
      }
      return out;
    }
    if (typeof val === "object" && hasBytes(val, 0)) {
      const keys = Object.keys(val);
      const entries = [];
      let total = 5;
      for (let i = 0; i < keys.length; i++) {
        const k = keys[i];
        const kv = val[k];
        if (kv === undefined || typeof kv === "function" || typeof kv === "symbol") continue;
        const kb = enc.encode(k);
        const vb = encodeWireValue(kv);
        entries.push([kb, vb]);
        total += 8 + kb.byteLength + vb.byteLength;
      }
      const out = new Uint8Array(total);
      const dv = new DataView(out.buffer);
      out[0] = WIRE_OBJECT;
      dv.setUint32(1, entries.length, true);
      let pos = 5;
      for (let i = 0; i < entries.length; i++) {
        const kb = entries[i][0];
        const vb = entries[i][1];
        dv.setUint32(pos, kb.byteLength, true);
        pos += 4;
        out.set(kb, pos);
        pos += kb.byteLength;
        dv.setUint32(pos, vb.byteLength, true);
        pos += 4;
        out.set(vb, pos);
        pos += vb.byteLength;
      }
      return out;
    }
    const json = JSON.stringify(val);
    if (json === undefined) {
      return new Uint8Array([WIRE_NULL]);
    }
    const jb = enc.encode(json);
    const out = new Uint8Array(1 + jb.byteLength);
    out[0] = WIRE_JSON_UTF8;
    out.set(jb, 1);
    return out;
  }

  function decodeWireValue(u8) {
    if (u8.byteLength === 0) throw new Error("Empty binary IPC value");
    const tag = u8[0];
    if (tag === WIRE_NULL) return null;
    if (tag === WIRE_JSON_UTF8) return JSON.parse(dec.decode(u8.subarray(1)));
    if (tag === WIRE_BYTES) return u8.slice(1);
    if (tag === WIRE_ARRAY) {
      const dv = new DataView(u8.buffer, u8.byteOffset, u8.byteLength);
      const count = dv.getUint32(1, true);
      const arr = new Array(count);
      let pos = 5;
      for (let i = 0; i < count; i++) {
        const len = dv.getUint32(pos, true);
        pos += 4;
        arr[i] = decodeWireValue(u8.subarray(pos, pos + len));
        pos += len;
      }
      return arr;
    }
    if (tag === WIRE_OBJECT) {
      const dv = new DataView(u8.buffer, u8.byteOffset, u8.byteLength);
      const count = dv.getUint32(1, true);
      const obj = {};
      let pos = 5;
      for (let i = 0; i < count; i++) {
        const keyLen = dv.getUint32(pos, true);
        pos += 4;
        const key = dec.decode(u8.subarray(pos, pos + keyLen));
        pos += keyLen;
        const valLen = dv.getUint32(pos, true);
        pos += 4;
        obj[key] = decodeWireValue(u8.subarray(pos, pos + valLen));
        pos += valLen;
      }
      return obj;
    }
    throw new Error("Unknown binary IPC tag: " + tag);
  }

  // flags 0: [0][u16 len][method][value]
  // with a call id: [flags][u32 id][u16 len][method][value]
  function encodeRpcPacket(method, arg, callId, stream) {
    const mb = enc.encode(String(method));
    const ab = encodeWireValue(arg);
    const head = callId === undefined ? 1 : 5;
    const out = new Uint8Array(head + 2 + mb.byteLength + ab.byteLength);
    const dv = new DataView(out.buffer);
    if (callId === undefined) {
      out[0] = 0;
    } else {
      out[0] = RPC_FLAG_CALL_ID | (stream ? RPC_FLAG_STREAM : 0);
      dv.setUint32(1, callId, true);
    }
    dv.setUint16(head, mb.byteLength, true);
    out.set(mb, head + 2);
    out.set(ab, head + 2 + mb.byteLength);
    return out;
  }

  function encodeCallPacket(op, callId, arg) {
    const ab = arg === undefined ? new Uint8Array(0) : encodeWireValue(arg);
    const out = new Uint8Array(5 + ab.byteLength);
    out[0] = op;
    new DataView(out.buffer).setUint32(1, callId, true);
    out.set(ab, 5);
    return out;
  }

  function packArgs(args) {
    return args.length === 0 ? null : (args.length === 1 ? args[0] : args);
  }

  // A trailing `{ signal, timeout }` object is call options, not an argument.
  function isCallOptions(v) {
    if (!v || typeof v !== "object" || Array.isArray(v) || isBinary(v)) return false;
    if (Object.getPrototypeOf(v) !== Object.prototype) return false;
    const keys = Object.keys(v);
    if (keys.length === 0) return false;
    for (const k of keys) {
      if (k !== "signal" && k !== "timeout") return false;
    }
    const sigOk = v.signal === undefined ||
      (typeof AbortSignal !== "undefined" && v.signal instanceof AbortSignal);
    const timeoutOk = v.timeout === undefined || typeof v.timeout === "number";
    return sigOk && timeoutOk;
  }

  function abortError(signal) {
    if (signal && signal.reason !== undefined) return signal.reason;
    const err = new Error("The IPC call was aborted");
    err.name = "AbortError";
    return err;
  }

  function postRpc(pkt) {
    if (useWkFallback()) {
      const id = nextFallbackId++;
      return new Promise((resolve, reject) => {
        pendingFallback.set(id, { resolve, reject });
        window.webkit.messageHandlers.ui.postMessage(
          JSON.stringify({ kind: "rpc", id, data: bytesToB64(pkt) })
        );
      });
    }
    return fetch(IPC_BASE + "/rpc", { method: "POST", body: pkt }).then(async (res) => {
      if (!res.ok) throw new Error("IPC HTTP " + res.status);
      return decodeRpcResponse(new Uint8Array(await res.arrayBuffer()));
    });
  }

  function invokeWithOptions(method, arg, opts) {
    const signal = opts.signal;
    if (signal && signal.aborted) return Promise.reject(abortError(signal));
    const callId = nextCallId++;
    return new Promise((resolve, reject) => {
      let settled = false;
      let timer = null;
      const cleanup = () => {
        settled = true;
        if (timer !== null) clearTimeout(timer);
        if (signal) signal.removeEventListener("abort", onAbort);
      };
      const giveUp = (err) => {
        if (settled) return;
        cleanup();
        sendPacket(encodeCallPacket(SEND_OP_CANCEL, callId));
        reject(err);
      };
      const onAbort = () => giveUp(abortError(signal));
      if (signal) signal.addEventListener("abort", onAbort, { once: true });
      if (typeof opts.timeout === "number" && opts.timeout >= 0) {
        timer = setTimeout(() => {
          const err = new Error("IPC call \"" + method + "\" timed out after " + opts.timeout + " ms");
          err.name = "TimeoutError";
          giveUp(err);
        }, opts.timeout);
      }
      postRpc(encodeRpcPacket(method, arg, callId, false)).then(
        (v) => { if (!settled) { cleanup(); resolve(v); } },
        (e) => { if (!settled) { cleanup(); reject(e); } }
      );
    });
  }

  function openStream(method, args) {
    const callId = nextCallId++;
    const dataCh = STREAM_DATA_PREFIX + callId;
    const endCh = STREAM_END_PREFIX + callId;
    const dataListeners = new Set();
    let done = false;
    let resolveClosed, rejectClosed;
    const closed = new Promise((res, rej) => { resolveClosed = res; rejectClosed = rej; });
    // Unobserved rejections of `closed` should not be reported as unhandled.
    closed.catch(() => {});
    const offData = api.on(dataCh, (chunk) => {
      for (const fn of Array.from(dataListeners)) {
        try { fn(chunk); } catch (e) { console.error("ipc stream listener error:", e); }
      }
    });
    const finish = () => {
      done = true;
      offData();
      offEnd();
      dataListeners.clear();
    };
    const offEnd = api.on(endCh, (end) => {
      if (done) return;
      finish();
      if (end && Object.prototype.hasOwnProperty.call(end, "error")) {
        rejectClosed(new Error(end.error));
      } else {
        resolveClosed(end ? end.ok : null);
      }
    });
    postRpc(encodeRpcPacket(method, packArgs(args), callId, true)).then(
      () => {},
      (err) => { if (!done) { finish(); rejectClosed(err); } }
    );
    return Object.freeze({
      id: callId,
      closed,
      on(event, fn) {
        if (event !== "data") throw new TypeError("ipc stream only emits \"data\"");
        if (typeof fn !== "function") throw new TypeError("ipc listener must be a function");
        dataListeners.add(fn);
        return () => dataListeners.delete(fn);
      },
      send(...data) {
        if (done) return;
        sendPacket(encodeCallPacket(SEND_OP_STREAM_DATA, callId, packArgs(data)));
      },
      close() {
        if (done) return;
        sendPacket(encodeCallPacket(SEND_OP_CANCEL, callId));
      }
    });
  }

  function encodeSendEvent(name, arg) {
    const nb = enc.encode(name);
    const ab = encodeWireValue(arg);
    const out = new Uint8Array(3 + nb.byteLength + ab.byteLength);
    const dv = new DataView(out.buffer);
    out[0] = SEND_OP_EVENT;
    dv.setUint16(1, nb.byteLength, true);
    out.set(nb, 3);
    out.set(ab, 3 + nb.byteLength);
    return out;
  }

  function encodeSendMsg(arg) {
    const ab = encodeWireValue(arg);
    const out = new Uint8Array(1 + ab.byteLength);
    out[0] = SEND_OP_MSG;
    out.set(ab, 1);
    return out;
  }

  function decodeRpcResponse(u8) {
    if (u8.byteLength === 0) throw new Error("Empty binary IPC response");
    if (u8[0] === 0x00) return decodeWireValue(u8.subarray(1));
    throw new Error(dec.decode(u8.subarray(1)) || "IPC error");
  }

  function dispatchEvent(channel, payload) {
    const set = listeners.get(channel);
    if (!set) return;
    for (const fn of Array.from(set)) {
      try {
        fn(payload);
      } catch (e) {
        console.error("ipc listener error:", e);
      }
    }
  }

  function dispatchBatch(u8) {
    if (u8.byteLength < 4) return;
    const dv = new DataView(u8.buffer, u8.byteOffset, u8.byteLength);
    const count = dv.getUint32(0, true);
    let pos = 4;
    for (let i = 0; i < count; i++) {
      const chLen = dv.getUint16(pos, true);
      pos += 2;
      const channel = dec.decode(u8.subarray(pos, pos + chLen));
      pos += chLen;
      const frameLen = dv.getUint32(pos, true);
      pos += 4;
      const payload = decodeWireValue(u8.subarray(pos, pos + frameLen));
      pos += frameLen;
      dispatchEvent(channel, payload);
    }
  }

  function bytesToB64(u8) {
    let s = "";
    const chunk = 0x8000;
    for (let i = 0; i < u8.length; i += chunk) {
      s += String.fromCharCode.apply(null, u8.subarray(i, i + chunk));
    }
    return btoa(s);
  }

  function b64ToBytes(b64) {
    const bin = atob(b64);
    const out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
  }

  function sendPacket(pkt) {
    if (useWkFallback()) {
      window.webkit.messageHandlers.ui.postMessage(
        JSON.stringify({ kind: "send", data: bytesToB64(pkt) })
      );
      return;
    }
    fetch(IPC_BASE + "/send", { method: "POST", body: pkt }).catch(() => {});
  }

  async function drainLoop() {
    if (useWkFallback() || !hasAnyListener) return;
    if (draining) {
      wakeAgain = true;
      return;
    }
    draining = true;
    try {
      do {
        wakeAgain = false;
        const res = await fetch(IPC_BASE + "/drain", { method: "POST" });
        if (!res.ok) break;
        const buf = await res.arrayBuffer();
        if (buf.byteLength > 0) {
          dispatchBatch(new Uint8Array(buf));
        }
      } while (wakeAgain);
    } catch (_) {
    } finally {
      draining = false;
      if (wakeAgain) {
        wakeAgain = false;
        drainLoop();
      }
    }
  }

  window.__ipc_wake = function() {
    if (!hasAnyListener) {
      wakeAgain = true;
      return;
    }
    drainLoop();
  };

  window.__ipc_deliver_b64 = function(b64) {
    dispatchBatch(b64ToBytes(b64));
  };

  window.__ipc_wk_rpc = function(id, b64) {
    const entry = pendingFallback.get(id);
    if (!entry) return;
    pendingFallback.delete(id);
    try {
      entry.resolve(decodeRpcResponse(b64ToBytes(b64)));
    } catch (err) {
      entry.reject(err);
    }
  };

  function off(event, fn) {
    const key = String(event);
    const set = listeners.get(key);
    if (!set) return api;
    if (set.has(fn)) {
      set.delete(fn);
    } else {
      for (const entry of set) {
        if (entry._ipcOrig === fn) {
          set.delete(entry);
          break;
        }
      }
    }
    if (set.size === 0) listeners.delete(key);
    return api;
  }

  const api = Object.freeze({
    invoke(method, ...args) {
      if (args.length > 0 && isCallOptions(args[args.length - 1])) {
        const opts = args.pop();
        return invokeWithOptions(method, packArgs(args), opts);
      }
      return postRpc(encodeRpcPacket(method, packArgs(args)));
    },
    stream(method, ...args) {
      return openStream(method, args);
    },
    emit(eventOrMsg, ...args) {
      if (typeof eventOrMsg === "string") {
        sendPacket(encodeSendEvent(eventOrMsg, packArgs(args)));
      } else {
        sendPacket(encodeSendMsg(eventOrMsg === undefined ? null : eventOrMsg));
      }
    },
    send(eventOrMsg, ...args) {
      api.emit(eventOrMsg, ...args);
    },
    on(event, fn) {
      if (typeof fn !== "function") throw new TypeError("ipc listener must be a function");
      const key = String(event);
      let set = listeners.get(key);
      if (!set) {
        set = new Set();
        listeners.set(key, set);
      }
      set.add(fn);
      if (!hasAnyListener) {
        hasAnyListener = true;
        scheduleMicrotask(drainLoop);
      }
      return () => off(key, fn);
    },
    once(event, fn) {
      if (typeof fn !== "function") throw new TypeError("ipc listener must be a function");
      const key = String(event);
      const wrapper = (data) => {
        off(key, wrapper);
        fn(data);
      };
      wrapper._ipcOrig = fn;
      return api.on(key, wrapper);
    },
    window: Object.freeze({
      // "light" | "dark" | "system": the native window's appearance.
      setTheme(theme) {
        return api.invoke("ui:window.setTheme", theme);
      }
    }),
    menu: Object.freeze({
      // Fires when a menu item made with `.emit()` (or "settings") is chosen.
      on(id, fn) {
        return api.on("menu:" + id, fn);
      },
      // Updates an item of the app menu: { id, title?, enabled?, checked? }.
      set(update) {
        return api.invoke("ui:menu.set", update);
      }
    }),
    off,
    removeListener: off,
    removeAllListeners(event) {
      if (event === undefined) {
        listeners.clear();
      } else {
        listeners.delete(String(event));
      }
      return api;
    }
  });

  window.ipc = api;

  // Title-bar layout and window state as CSS variables and attributes on <html>:
  //   --ui-titlebar-height, --ui-window-controls-left, --ui-window-controls-right
  //   data-ui-platform, data-ui-fullscreen, data-ui-focused, data-ui-theme
  let chrome = null;
  function applyChrome() {
    const root = document.documentElement;
    if (!root || !chrome) return;
    const px = (v) => (typeof v === "number" ? v : 0) + "px";
    root.style.setProperty("--ui-titlebar-height", px(chrome.titlebar_height));
    root.style.setProperty("--ui-window-controls-left", px(chrome.controls_left));
    root.style.setProperty("--ui-window-controls-right", px(chrome.controls_right));
    if (chrome.platform) root.setAttribute("data-ui-platform", chrome.platform);
    if (chrome.theme) root.setAttribute("data-ui-theme", chrome.theme);
    root.setAttribute("data-ui-fullscreen", chrome.fullscreen ? "true" : "false");
    root.setAttribute("data-ui-focused", chrome.focused === false ? "false" : "true");
  }
  function setChrome(c) {
    if (!c || typeof c !== "object") return;
    chrome = c;
    if (document.documentElement) {
      applyChrome();
    } else {
      const mo = new MutationObserver(() => {
        if (document.documentElement) {
          mo.disconnect();
          applyChrome();
        }
      });
      mo.observe(document, { childList: true });
    }
  }
  api.on("__ui_chrome", setChrome);
  api.invoke("ui:chrome").then(setChrome, () => {});

  function isInteractive(el) {
    return el.matches("input, textarea, select, option, button, a, summary, [contenteditable], [data-ui-no-drag]");
  }

  function isDragTarget(target) {
    let el = target instanceof Element ? target : (target && target.parentElement);
    while (el) {
      if (isInteractive(el)) return false;
      const style = window.getComputedStyle(el);
      const appRegion = style.webkitAppRegion || style.appRegion;
      if (appRegion === "no-drag" || el.hasAttribute("data-ui-no-drag")) return false;
      if (appRegion === "drag" || el.hasAttribute("data-ui-drag")) return true;
      el = el.parentElement;
    }
    return false;
  }

  // The second press of a double click is handled on mousedown, not `dblclick`:
  // the window drag started by the first press can swallow the click events.
  window.addEventListener("mousedown", function(ev) {
    if (ev.button !== 0 || ev.detail > 2) return;
    if (!isDragTarget(ev.target)) return;
    if (ev.detail === 2) {
      sendPacket(new Uint8Array([SEND_OP_TITLEBAR_DOUBLE_CLICK]));
    } else {
      sendPacket(new Uint8Array([SEND_OP_DRAG]));
    }
  }, true);
})();"#;

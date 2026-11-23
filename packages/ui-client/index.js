// ui-client: typed access to ui's `window.ipc` and the standard page calls.
// Versioned with the `ui` crate. Inside a ui webview everything goes through
// `window.ipc`; in a plain browser, `isNative()` is false and the helpers fall
// back to web APIs where one exists.

const NOT_NATIVE = "not running inside a ui webview";

function nativeIpc() {
  return typeof window !== "undefined" && window.ipc && typeof window.ipc.invoke === "function"
    ? window.ipc
    : null;
}

/** True inside a ui webview. */
export function isNative() {
  return nativeIpc() !== null;
}

function detectPlatform() {
  if (!isNative() || typeof navigator === "undefined") return "web";
  const p = (navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || "";
  if (/mac/i.test(p)) return "macos";
  if (/win/i.test(p)) return "windows";
  return "linux";
}

/** "macos" | "windows" | "linux" inside a ui webview, "web" otherwise. */
export const platform = detectPlatform();

const webStub = Object.freeze({
  invoke() {
    return Promise.reject(new Error(NOT_NATIVE));
  },
  stream() {
    const closed = Promise.reject(new Error(NOT_NATIVE));
    closed.catch(() => {});
    return Object.freeze({ id: 0, closed, on: () => () => {}, send() {}, close() {} });
  },
  emit() {},
  send() {},
  on: () => () => {},
  once: () => () => {},
  off() { return webStub; },
  removeListener() { return webStub; },
  removeAllListeners() { return webStub; },
  menu: Object.freeze({ on: () => () => {}, set: () => Promise.resolve(false) }),
  contextMenu: () => Promise.resolve(null),
  window: Object.freeze({ setTheme: () => Promise.resolve() }),
});

/**
 * `window.ipc` inside a ui webview, or a stub whose calls reject (and whose
 * listeners never fire) in a plain browser. Resolved on each use, so importing
 * this module before the shim runs is fine.
 */
export const ipc = new Proxy({}, {
  get(_target, key) {
    const real = nativeIpc() || webStub;
    const v = real[key];
    return typeof v === "function" ? v.bind(real) : v;
  },
  has(_target, key) {
    return key in (nativeIpc() || webStub);
  },
});

function call(method, ...args) {
  return ipc.invoke(method, ...args);
}

function fileOptions(o) {
  if (!o) return null;
  return {
    title: o.title,
    directory: o.directory,
    default_name: o.defaultName,
    filters: o.filters,
  };
}

const BUTTONS = { ok: "ok", okCancel: "ok_cancel", yesNo: "yes_no", yesNoCancel: "yes_no_cancel" };

/** Typed wrappers for the standard page calls (see `WebViewBuilder::standard_rpcs`). */
export const ui = Object.freeze({
  app: Object.freeze({
    info() {
      if (!isNative()) {
        return Promise.resolve({ name: typeof document !== "undefined" ? document.title : "", version: null, backend: "web", platform: "web" });
      }
      return call("ui:app.info");
    },
    openExternal(url) {
      if (!isNative()) {
        window.open(url, "_blank", "noopener");
        return Promise.resolve();
      }
      return call("ui:app.openExternal", String(url));
    },
  }),
  dialog: Object.freeze({
    pickFile: (options) => call("ui:dialog.pickFile", fileOptions(options)),
    pickFiles: (options) => call("ui:dialog.pickFiles", fileOptions(options)),
    pickFolder: (options) => call("ui:dialog.pickFolder", fileOptions(options)),
    saveFile: (options) => call("ui:dialog.saveFile", fileOptions(options)),
    message(options) {
      return call("ui:dialog.message", {
        title: options.title,
        body: options.body,
        level: options.level,
        buttons: options.buttons ? BUTTONS[options.buttons] || options.buttons : undefined,
      });
    },
    confirm(title, body, options) {
      if (!isNative()) return Promise.resolve(window.confirm(body ? `${title}\n\n${body}` : title));
      return call("ui:dialog.confirm", { title, body, level: options && options.level });
    },
  }),
  clipboard: Object.freeze({
    readText() {
      if (!isNative()) return navigator.clipboard.readText();
      return call("ui:clipboard.readText");
    },
    writeText(text) {
      if (!isNative()) return navigator.clipboard.writeText(text);
      return call("ui:clipboard.writeText", String(text));
    },
  }),
  window: Object.freeze({
    minimize: () => call("ui:window.minimize"),
    maximize: () => call("ui:window.maximize"),
    setFullscreen: (on) => call("ui:window.setFullscreen", !!on),
    setTitle(title) {
      if (!isNative()) {
        document.title = title;
        return Promise.resolve();
      }
      return call("ui:window.setTitle", String(title));
    },
    state: () => call("ui:window.state"),
    close: () => call("ui:window.close"),
    setTheme: (theme) => ipc.window.setTheme(theme),
  }),
  menu: Object.freeze({
    on: (id, fn) => ipc.menu.on(id, fn),
    set: (update) => ipc.menu.set(update),
  }),
  /** Native context menu; resolves to the chosen id or null. Web: always null. */
  contextMenu(items, position) {
    if (!isNative()) return Promise.resolve(null);
    return ipc.contextMenu(items, position);
  },
  notify(title, body) {
    return call("ui:notify", String(title), body == null ? null : String(body));
  },
});

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

// URLs and files opened before this page loaded (e.g. the link that launched
// the app), fetched once and replayed to each `ui.app.on` listener.
let openedOnce = null;
function opened() {
  if (!openedOnce) openedOnce = call("ui:app.takeOpened").catch(() => ({ urls: [], files: [] }));
  return openedOnce;
}

const APP_EVENTS = { "open-url": "ui:app:open-url", "open-files": "ui:app:open-files" };

const notificationClickOffs = new Map();

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
    /**
     * "open-url": a URL with one of the app's schemes (fn gets the URL string).
     * "open-files": files opened with the app (fn gets an array of paths).
     * Includes anything that arrived before the page subscribed.
     */
    on(event, fn) {
      const channel = APP_EVENTS[event];
      if (!channel) throw new TypeError(`unknown ui.app event "${event}"`);
      if (!isNative()) return () => {};
      let active = true;
      const off = ipc.on(channel, fn);
      opened().then(({ urls, files }) => {
        if (!active) return;
        if (event === "open-url") for (const u of urls || []) fn(u);
        else if (files && files.length) fn(files);
      });
      return () => {
        active = false;
        off();
      };
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
    /** The unsaved-changes dot in the close button (macOS). */
    setDocumentEdited: (edited) => call("ui:window.setDocumentEdited", !!edited),
    setTheme: (theme) => ipc.window.setTheme(theme),
  }),
  webview: Object.freeze({
    /** Finds text and selects the next match; true/false, or null when the engine can't tell (CEF). */
    find(text, options = {}) {
      if (!isNative()) return Promise.resolve(null);
      return call("ui:webview.find", {
        text: String(text),
        case_sensitive: options.caseSensitive,
        backwards: options.backwards,
        wraps: options.wraps,
      });
    },
    clearFind: () => (isNative() ? call("ui:webview.clearFind") : Promise.resolve()),
    print() {
      if (!isNative()) {
        window.print();
        return Promise.resolve();
      }
      return call("ui:webview.print");
    },
    /** The page as PDF bytes (WebKit). */
    pdf: () => call("ui:webview.pdf"),
    /** Files dropped on the page, with real paths: fn({ paths, x, y }). WebKit. */
    onFileDrop: (fn) => ipc.on("ui:file-drop", fn),
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
  /**
   * Shows a notification; resolves to its id. `onClick` runs when it's clicked
   * (bundled apps only). Web: uses the Notification API when permitted.
   */
  async notify(title, body, options = {}) {
    if (!isNative()) {
      if (typeof Notification !== "undefined" && Notification.permission === "granted") {
        const n = new Notification(title, { body, tag: options.id });
        if (options.onClick) n.onclick = options.onClick;
      }
      return options.id || "";
    }
    const { onClick, ...rest } = options;
    const opts = { id: rest.id, subtitle: rest.subtitle, sound: rest.sound };
    // Listen before showing so a fast click isn't missed.
    let wanted = opts.id;
    const early = [];
    const off = onClick
      ? ipc.on("ui:notification:click", (id) => {
          if (wanted === undefined) early.push(id);
          else if (id === wanted) onClick(id);
        })
      : null;
    const id = await call("ui:notify", String(title), body == null ? null : String(body), opts);
    wanted = id;
    if (onClick && early.includes(id)) onClick(id);
    if (off) notificationClickOffs.set(id, off);
    return id;
  },
  notification: Object.freeze({
    /** Removes a notification shown with `ui.notify`, and its click listener. */
    remove(id) {
      const off = notificationClickOffs.get(id);
      if (off) {
        off();
        notificationClickOffs.delete(id);
      }
      if (!isNative()) return Promise.resolve();
      return call("ui:notification.remove", String(id));
    },
  }),
});

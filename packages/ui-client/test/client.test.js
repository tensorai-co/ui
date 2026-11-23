import { test } from "node:test";
import assert from "node:assert/strict";

test("outside a ui webview: not native, calls reject, listeners are no-ops", async () => {
  delete globalThis.window;
  const m = await import("../index.js?web");
  assert.equal(m.isNative(), false);
  assert.equal(m.platform, "web");
  await assert.rejects(m.ipc.invoke("x"), /not running inside a ui webview/);
  assert.equal(typeof m.ipc.on("e", () => {}), "function");
  assert.equal(await m.ipc.menu.set({ id: "a" }), false);
});

test("inside a ui webview: wrappers call the standard RPC names", async () => {
  const calls = [];
  globalThis.window = {
    ipc: {
      invoke: (method, ...args) => {
        calls.push([method, ...args]);
        return Promise.resolve(method === "ui:dialog.confirm" ? true : null);
      },
      menu: { on: () => () => {}, set: (u) => Promise.resolve(u.id === "known") },
      window: { setTheme: (t) => { calls.push(["setTheme", t]); return Promise.resolve(); } },
    },
  };
  const m = await import("../index.js?native");
  assert.equal(m.isNative(), true);

  await m.ui.dialog.pickFile({ title: "Open", defaultName: "a.txt", filters: [{ name: "Text", extensions: ["txt"] }] });
  assert.deepEqual(calls.at(-1), [
    "ui:dialog.pickFile",
    { title: "Open", directory: undefined, default_name: "a.txt", filters: [{ name: "Text", extensions: ["txt"] }] },
  ]);
  await m.ui.dialog.pickFolder();
  assert.deepEqual(calls.at(-1), ["ui:dialog.pickFolder", null]);
  await m.ui.dialog.message({ title: "Hi", buttons: "yesNoCancel" });
  assert.equal(calls.at(-1)[1].buttons, "yes_no_cancel");
  assert.equal(await m.ui.dialog.confirm("Delete?"), true);
  await m.ui.notify("Done");
  assert.deepEqual(calls.at(-1), ["ui:notify", "Done", null]);
  await m.ui.window.setFullscreen(1);
  assert.deepEqual(calls.at(-1), ["ui:window.setFullscreen", true]);
  await m.ui.window.setTheme("dark");
  assert.deepEqual(calls.at(-1), ["setTheme", "dark"]);
  assert.equal(await m.ui.menu.set({ id: "known" }), true);
  // `ipc` resolves window.ipc on each use.
  await m.ipc.invoke("custom", 1, 2);
  assert.deepEqual(calls.at(-1), ["custom", 1, 2]);
  delete globalThis.window;
});

test("ui.app.on replays URLs and files that arrived before the page", async () => {
  const listeners = new Map();
  let takes = 0;
  globalThis.window = {
    ipc: {
      invoke: (method) => {
        if (method !== "ui:app.takeOpened") return Promise.resolve(null);
        takes++;
        return Promise.resolve({ urls: ["myapp://a"], files: ["/tmp/x.md"] });
      },
      on: (ch, fn) => { listeners.set(ch, fn); return () => listeners.delete(ch); },
      menu: { on: () => () => {}, set: () => Promise.resolve(false) },
      window: { setTheme: () => Promise.resolve() },
    },
  };
  const m = await import("../index.js?opened");
  const urls = [];
  const files = [];
  m.ui.app.on("open-url", (u) => urls.push(u));
  m.ui.app.on("open-files", (f) => files.push(...f));
  await new Promise((r) => setTimeout(r, 0));
  listeners.get("ui:app:open-url")("myapp://b");
  assert.deepEqual(urls, ["myapp://a", "myapp://b"]);
  assert.deepEqual(files, ["/tmp/x.md"]);
  assert.equal(takes, 1, "pending items are fetched once per page");
  assert.throws(() => m.ui.app.on("nope", () => {}), /unknown ui.app event/);
  delete globalThis.window;
});

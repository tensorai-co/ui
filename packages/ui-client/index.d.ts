// Types for ui-client. Versioned with the `ui` crate.

/** Values that cross IPC: JSON-compatible data, with binary as Uint8Array / ArrayBuffer views. */
export type IpcValue =
  | null
  | boolean
  | number
  | string
  | Uint8Array
  | ArrayBuffer
  | ArrayBufferView
  | IpcValue[]
  | { [key: string]: IpcValue | undefined };

/** Trailing `ipc.invoke` options. An object with only these keys is never sent as an argument. */
export interface CallOptions {
  /** Aborting rejects the call and makes Rust's `ctx.is_cancelled()` true. */
  signal?: AbortSignal;
  /** Milliseconds before the call rejects with a `TimeoutError` (and is cancelled in Rust). */
  timeout?: number;
}

export type Unsubscribe = () => void;

export interface IpcStream<T = IpcValue> {
  readonly id: number;
  /** Resolves with the Rust handler's return value; rejects with its error. */
  readonly closed: Promise<IpcValue>;
  /** Frames sent with Rust's `Stream::send`, in order. */
  on(event: "data", fn: (data: T) => void): Unsubscribe;
  /** Data for Rust's `Stream::recv`. */
  send(...data: IpcValue[]): void;
  /** Closes the stream: Rust's `Stream::is_closed()` turns true. */
  close(): void;
}

export interface MenuItemUpdate {
  id: string;
  title?: string;
  enabled?: boolean;
  checked?: boolean;
}

export type Theme = "light" | "dark" | "system";

export type ContextMenuItem =
  | "-"
  | {
      id?: string;
      label: string;
      /** Shown next to the label, e.g. "F2" or "Cmd+Backspace". */
      shortcut?: string;
      enabled?: boolean;
      checked?: boolean;
      /** Destructive action, drawn in red. */
      danger?: boolean;
      /** A submenu (then `id` may be left out). */
      items?: ContextMenuItem[];
    };

export interface Ipc {
  invoke<T = IpcValue>(method: string, ...args: (IpcValue | CallOptions)[]): Promise<T>;
  stream<T = IpcValue>(method: string, ...args: IpcValue[]): IpcStream<T>;
  /** Fire-and-forget to Rust `.on(event)`, or an object to `.on_msg`. */
  emit(event: string, ...args: IpcValue[]): void;
  emit(message: { type: string; [key: string]: IpcValue | undefined }): void;
  /** @deprecated Use `emit`. */
  send(event: string, ...args: IpcValue[]): void;
  /** @deprecated Use `emit`. */
  send(message: { type: string; [key: string]: IpcValue | undefined }): void;
  on<T = IpcValue>(event: string, fn: (payload: T) => void): Unsubscribe;
  once<T = IpcValue>(event: string, fn: (payload: T) => void): Unsubscribe;
  off(event: string, fn: (payload: any) => void): Ipc;
  removeListener(event: string, fn: (payload: any) => void): Ipc;
  removeAllListeners(event?: string): Ipc;
  readonly menu: {
    on(id: string, fn: () => void): Unsubscribe;
    /** Resolves false when no menu item has that id. */
    set(update: MenuItemUpdate): Promise<boolean>;
  };
  readonly window: {
    setTheme(theme: Theme): Promise<void>;
  };
  /** Native context menu at `position` (CSS px) or the last pointer press; resolves to the chosen id. */
  contextMenu(items: ContextMenuItem[], position?: { x: number; y: number }): Promise<string | null>;
}

export const ipc: Ipc;

/** True inside a ui webview. */
export function isNative(): boolean;

export const platform: "macos" | "windows" | "linux" | "web";

export interface AppInfo {
  name: string;
  version: string | null;
  backend: "webkit" | "cef" | "web";
  platform: "macos" | "windows" | "linux" | "web";
}

export interface FileFilter {
  name: string;
  extensions: string[];
}

export interface FileDialogOptions {
  title?: string;
  directory?: string;
  defaultName?: string;
  filters?: FileFilter[];
}

export type MessageLevel = "info" | "warning" | "error";

export interface MessageOptions {
  title: string;
  body?: string;
  level?: MessageLevel;
  buttons?: "ok" | "okCancel" | "yesNo" | "yesNoCancel";
}

export type WindowState = "normal" | "minimized" | "maximized" | "fullscreen";

/** The standard page calls; the app enables them with `WebViewBuilder::standard_rpcs`. */
export const ui: {
  readonly app: {
    info(): Promise<AppInfo>;
    /** A URL with one of the app's `url_schemes`, including one that launched the app. */
    on(event: "open-url", fn: (url: string) => void): Unsubscribe;
    /** Files opened with the app (Finder, Dock icon drop). */
    on(event: "open-files", fn: (paths: string[]) => void): Unsubscribe;
    /** http, https and mailto URLs only. */
    openExternal(url: string): Promise<void>;
  };
  readonly dialog: {
    pickFile(options?: FileDialogOptions): Promise<string | null>;
    pickFiles(options?: FileDialogOptions): Promise<string[]>;
    pickFolder(options?: FileDialogOptions): Promise<string | null>;
    saveFile(options?: FileDialogOptions): Promise<string | null>;
    message(options: MessageOptions): Promise<"ok" | "cancel" | "yes" | "no">;
    /** A native OK/Cancel sheet; resolves true for OK. */
    confirm(title: string, body?: string, options?: { level?: MessageLevel }): Promise<boolean>;
  };
  readonly clipboard: {
    readText(): Promise<string | null>;
    writeText(text: string): Promise<void>;
  };
  readonly window: {
    minimize(): Promise<void>;
    maximize(): Promise<void>;
    setFullscreen(on: boolean): Promise<void>;
    setTitle(title: string): Promise<void>;
    state(): Promise<WindowState>;
    close(): Promise<void>;
    /** Shows the unsaved-changes dot in the close button (macOS; no-op elsewhere). */
    setDocumentEdited(edited: boolean): Promise<void>;
    setTheme(theme: Theme): Promise<void>;
  };
  readonly menu: {
    on(id: string, fn: () => void): Unsubscribe;
    set(update: MenuItemUpdate): Promise<boolean>;
  };
  /** Native context menu; resolves to the chosen id, or null when dismissed (always null on the web). */
  contextMenu(items: ContextMenuItem[], position?: { x: number; y: number }): Promise<string | null>;
  notify(title: string, body?: string): Promise<void>;
};

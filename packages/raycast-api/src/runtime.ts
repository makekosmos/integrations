import type {
  AlertOptions,
  LaunchCommandOptions,
  PreferenceValues,
  RaycastChild,
} from "./types";

export interface ToastOptions {
  style?: ToastStyle;
  title: string;
  message?: string;
  primaryAction?: { title: string; onAction?: () => void | Promise<void> };
}

export const Toast = {
  Style: {
    Success: "success",
    Failure: "failure",
    Animated: "animated",
  },
} as const;

export type ToastStyle = (typeof Toast.Style)[keyof typeof Toast.Style];

interface RaycastNavigation {
  push(target: RaycastChild): void;
  pop(): void;
  popToRoot(): void;
}

function isToastStyle(value: ToastOptions | ToastStyle): value is ToastStyle {
  return typeof value === "string";
}

export interface RaycastRuntimeAdapter {
  showToast(options: ToastOptions): Promise<void>;
  showHUD(title: string): Promise<void>;
  confirmAlert(options: AlertOptions): Promise<boolean>;
  clipboardCopy(content: string): Promise<void>;
  clipboardPaste(content: string): Promise<void>;
  clipboardReadText(): Promise<string>;
  clipboardClear(): Promise<void>;
  systemOpen(target: string): Promise<void>;
  systemShowInFinder(path: string): Promise<void>;
  systemTrash(path: string): Promise<void>;
  localStorageGetItem(key: string): Promise<string | undefined>;
  localStorageAllItems(): Promise<Record<string, string>>;
  localStorageSetItem(key: string, value: string): Promise<void>;
  localStorageRemoveItem(key: string): Promise<void>;
  localStorageClear(): Promise<void>;
  cacheGet(namespace: string, key: string): Promise<string | undefined>;
  cacheSet(namespace: string, key: string, value: string): Promise<void>;
  cacheRemove(namespace: string, key: string): Promise<void>;
  cacheClear(namespace: string): Promise<void>;
  getPreferenceValues(): PreferenceValues;
  launchCommand(options: LaunchCommandOptions): Promise<void>;
  navigationPush(target: RaycastChild): void;
  navigationPop(): void;
  navigationPopToRoot(): void;
}

function memoryRuntime(): RaycastRuntimeAdapter {
  const localStorage = new Map<string, string>();
  const cache = new Map<string, Map<string, string>>();
  const stack: unknown[] = [];
  let clipboardText = "";

  function cacheNamespace(namespace: string): Map<string, string> {
    let bucket = cache.get(namespace);
    if (!bucket) {
      bucket = new Map<string, string>();
      cache.set(namespace, bucket);
    }
    return bucket;
  }

  return {
    async showToast() {},
    async showHUD() {},
    async confirmAlert() {
      return false;
    },
    async clipboardCopy(content) {
      clipboardText = content;
    },
    async clipboardPaste(content) {
      clipboardText = content;
    },
    async clipboardReadText() {
      return clipboardText;
    },
    async clipboardClear() {
      clipboardText = "";
    },
    async systemOpen() {},
    async systemShowInFinder() {},
    async systemTrash() {},
    async localStorageGetItem(key) {
      return localStorage.get(key);
    },
    async localStorageAllItems() {
      return Object.fromEntries(localStorage.entries());
    },
    async localStorageSetItem(key, value) {
      localStorage.set(key, value);
    },
    async localStorageRemoveItem(key) {
      localStorage.delete(key);
    },
    async localStorageClear() {
      localStorage.clear();
    },
    async cacheGet(namespace, key) {
      return cacheNamespace(namespace).get(key);
    },
    async cacheSet(namespace, key, value) {
      cacheNamespace(namespace).set(key, value);
    },
    async cacheRemove(namespace, key) {
      cacheNamespace(namespace).delete(key);
    },
    async cacheClear(namespace) {
      cacheNamespace(namespace).clear();
    },
    getPreferenceValues() {
      return {};
    },
    async launchCommand() {
      throw new Error("[kosmos-raycast] launchCommand is not configured");
    },
    navigationPush(target) {
      stack.push(target);
    },
    navigationPop() {
      stack.pop();
    },
    navigationPopToRoot() {
      stack.splice(0, Math.max(0, stack.length - 1));
    },
  };
}

let runtime = memoryRuntime();

export function configureRaycastRuntime(adapter: RaycastRuntimeAdapter): void {
  runtime = adapter;
}

export function resetRaycastRuntimeForTest(): void {
  runtime = memoryRuntime();
}

export function getRaycastRuntime(): RaycastRuntimeAdapter {
  return runtime;
}

export async function showToast(options: ToastOptions | ToastStyle, title?: string): Promise<void> {
  if (isToastStyle(options)) {
    await runtime.showToast({ style: options, title: title ?? "" });
    return;
  }
  await runtime.showToast(options);
}

export async function showHUD(title: string): Promise<void> {
  await runtime.showHUD(title);
}

export async function confirmAlert(options: AlertOptions): Promise<boolean> {
  return runtime.confirmAlert(options);
}

export const Clipboard = {
  copy: (content: string): Promise<void> => runtime.clipboardCopy(content),
  paste: (content: string): Promise<void> => runtime.clipboardPaste(content),
  readText: (): Promise<string> => runtime.clipboardReadText(),
  read: (): Promise<string> => runtime.clipboardReadText(),
  clear: (): Promise<void> => runtime.clipboardClear(),
};

export function open(target: string): Promise<void> {
  return runtime.systemOpen(target);
}

export function showInFinder(path: string): Promise<void> {
  return runtime.systemShowInFinder(path);
}

export function trash(path: string): Promise<void> {
  return runtime.systemTrash(path);
}

export const LocalStorage = {
  getItem: (key: string): Promise<string | undefined> => runtime.localStorageGetItem(key),
  allItems: (): Promise<Record<string, string>> => runtime.localStorageAllItems(),
  setItem: (key: string, value: string): Promise<void> => runtime.localStorageSetItem(key, value),
  removeItem: (key: string): Promise<void> => runtime.localStorageRemoveItem(key),
  clear: (): Promise<void> => runtime.localStorageClear(),
};

export class Cache {
  readonly namespace: string;

  constructor(options: { namespace?: string } = {}) {
    this.namespace = options.namespace ?? "default";
  }

  get(key: string): Promise<string | undefined> {
    return runtime.cacheGet(this.namespace, key);
  }

  set(key: string, value: string): Promise<void> {
    return runtime.cacheSet(this.namespace, key, value);
  }

  remove(key: string): Promise<void> {
    return runtime.cacheRemove(this.namespace, key);
  }

  clear(): Promise<void> {
    return runtime.cacheClear(this.namespace);
  }
}

export function getPreferenceValues<T extends PreferenceValues = PreferenceValues>(): T {
  // SAFETY: the configured runtime adapter returns the preference contract requested by the caller.
  return runtime.getPreferenceValues() as T;
}

export function launchCommand(options: LaunchCommandOptions): Promise<void> {
  return runtime.launchCommand(options);
}

export function useNavigation(): RaycastNavigation {
  return {
    push: (target) => runtime.navigationPush(target),
    pop: () => runtime.navigationPop(),
    popToRoot: () => runtime.navigationPopToRoot(),
  };
}

export const environment = {
  extensionName: "kosmos-raycast-extension",
  commandName: "",
  isDevelopment: false,
  supportPath: "",
  assetsPath: "",
  launchType: "userInitiated",
} as const;

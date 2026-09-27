import { afterEach, describe, expect, test } from "bun:test";
import {
  configureRaycastRuntime,
  getRaycastRuntime,
  resetRaycastRuntimeForTest,
  useNavigation,
  type RaycastRuntimeAdapter,
} from "../src/index";
import * as jsxRuntime from "../src/jsx-runtime";

function memoryAdapter(): RaycastRuntimeAdapter {
  resetRaycastRuntimeForTest();
  return getRaycastRuntime();
}

function spyAdapter(): RaycastRuntimeAdapter & { calls: string[] } {
  const base = memoryAdapter();
  const calls: string[] = [];
  return Object.assign({}, base, {
    calls,
    navigationPush(target: unknown) {
      calls.push("push");
      base.navigationPush(target as never);
    },
    navigationPop() {
      calls.push("pop");
      base.navigationPop();
    },
    navigationPopToRoot() {
      calls.push("popToRoot");
      base.navigationPopToRoot();
    },
  });
}

afterEach(() => {
  resetRaycastRuntimeForTest();
});

describe("navigation", () => {
  test("useNavigation delegates push/pop/popToRoot to the configured adapter", () => {
    const adapter = spyAdapter();
    configureRaycastRuntime(adapter);
    const nav = useNavigation();
    nav.push({ type: "List" });
    nav.pop();
    nav.popToRoot();
    expect(adapter.calls).toEqual(["push", "pop", "popToRoot"]);
  });

  test("memory adapter popToRoot drops every screen above the root", () => {
    memoryAdapter();
    const nav = useNavigation();
    const root = { type: "List" };
    nav.push(root);
    nav.push({ type: "Detail" });
    nav.push({ type: "Form" });

    // The adapter's stack is closure-private; intercept the one observable
    // signal — the Array#splice call popToRoot performs on it — and replay it
    // on the captured pre-call snapshot to verify the root survives.
    const original = Array.prototype.splice;
    const captured: { stack: unknown[]; args: unknown[] }[] = [];
    Array.prototype.splice = function (this: unknown[], ...args: never[]) {
      captured.push({ stack: [...this], args });
      return original.apply(this, args);
    } as typeof Array.prototype.splice;
    try {
      nav.popToRoot();
    } finally {
      Array.prototype.splice = original;
    }

    expect(captured).toHaveLength(1);
    const stack = [...captured[0].stack];
    stack.splice(...(captured[0].args as [number, number]));
    expect(stack).toEqual([root]);
  });
});

describe("jsx-runtime", () => {
  test("exports jsx, jsxs, Fragment, and jsxDEV (react-jsxdev emit)", () => {
    expect(typeof jsxRuntime.jsx).toBe("function");
    expect(typeof jsxRuntime.jsxs).toBe("function");
    expect(typeof jsxRuntime.jsxDEV).toBe("function");
    expect(jsxRuntime.Fragment).toBeDefined();
  });

  test("jsxDEV constructs the same element as jsx", () => {
    const props = { children: [] };
    expect(jsxRuntime.jsxDEV("List", props)).toEqual(
      jsxRuntime.jsx("List", props),
    );
  });
});

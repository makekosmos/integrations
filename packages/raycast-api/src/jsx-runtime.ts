import { createRaycastElement } from "./components";
import type { RaycastChild } from "./types";

export const Fragment = "Fragment";

const EMPTY_JSX_PROPS = {};
type JsxCallback = (...args: never[]) => void;
type JsxProp =
  | RaycastChild
  | Date
  | JsxCallback
  | readonly JsxProp[]
  | { readonly [key: string]: JsxProp };
type JsxProps = Record<string, JsxProp>;
type JsxComponent = (props: JsxProps) => RaycastChild;

function normalizeJsxProps(
  props: JsxProps | null | undefined,
): JsxProps {
  return props ?? EMPTY_JSX_PROPS;
}

function isJsxComponent(type: string | JsxComponent): type is JsxComponent {
  return typeof type === "function";
}

export function jsx(
  type: string | JsxComponent,
  props: JsxProps | null | undefined,
): RaycastChild {
  const normalizedProps = normalizeJsxProps(props);
  if (isJsxComponent(type)) return type(normalizedProps);
  return createRaycastElement(type, normalizedProps);
}

export const jsxs = jsx;

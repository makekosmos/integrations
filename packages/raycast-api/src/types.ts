export type RaycastChild =
  | RaycastElement
  | string
  | number
  | boolean
  | null
  | undefined
  | RaycastChild[];

export interface RaycastElement<TType extends string = string, TProps = object> {
  readonly $$typeof: "kosmos.raycast.element";
  readonly type: TType;
  readonly props: TProps & { children?: RaycastChild[] };
}

interface Icon {
  source: string;
  tintColor?: string;
}

export type ImageLike = string | Icon;

export interface LaunchProps<TArguments = object, TContext = object> {
  arguments?: TArguments;
  launchContext?: TContext;
  launchType: LaunchTypeValue;
  fallbackText?: string;
}

export const LaunchType = {
  UserInitiated: "userInitiated",
  Background: "background",
  LaunchCommand: "launchCommand",
} as const;

export type LaunchTypeValue = (typeof LaunchType)[keyof typeof LaunchType];

export interface LaunchCommandOptions {
  name: string;
  extensionName?: string;
  type?: LaunchTypeValue;
  context?: object;
  arguments?: object;
  fallbackText?: string;
}

export interface AlertOptions {
  title: string;
  message?: string;
  primaryAction?: { title: string; style?: "default" | "destructive" };
  dismissAction?: { title: string };
}

export type PreferenceValue = string | number | boolean | null | readonly string[];

export interface PreferenceValues {
  [key: string]: PreferenceValue;
}

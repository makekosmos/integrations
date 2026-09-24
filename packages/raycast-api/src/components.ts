import { Clipboard, getRaycastRuntime, showInFinder, trash } from "./runtime";
import type { ImageLike, RaycastChild, RaycastElement } from "./types";

export function createRaycastElement<TType extends string, TProps extends object>(
  type: TType,
  props: TProps,
  ...children: RaycastChild[]
): RaycastElement<TType, TProps> {
  // SAFETY: component props are the typed object supplied by the caller; only its optional child is read.
  const propsWithChildren = props as TProps & { children?: RaycastChild | RaycastChild[] };
  const propChildren = propsWithChildren.children;
  const normalizedChildren =
    children.length > 0
      ? children
      : Array.isArray(propChildren)
        ? propChildren
        : propChildren === undefined
          ? []
          : [propChildren];

  return {
    $$typeof: "kosmos.raycast.element",
    type,
    props: {
      ...props,
      children: normalizedChildren,
    },
  };
}

type RaycastArgumentValue = string | number | boolean | null | RaycastArgumentMap | RaycastArgumentMap[];
interface RaycastArgumentMap {
  [key: string]: RaycastArgumentValue | undefined;
}

function component<TType extends string, TProps extends object = object>(
  type: TType,
) {
  // SAFETY: an omitted component props object is the empty object for every component contract.
  return (props = {} as TProps): RaycastElement<TType, TProps> => createRaycastElement(type, props);
}

interface ListItemProps {
  id?: string;
  title: string;
  subtitle?: string;
  icon?: ImageLike;
  keywords?: string[];
  accessories?: unknown[];
  actions?: RaycastChild;
  detail?: RaycastChild;
}

interface DetailProps {
  markdown?: string;
  navigationTitle?: string;
  metadata?: RaycastChild;
  actions?: RaycastChild;
}

const DetailMetadata = Object.assign(component("Detail.Metadata"), {
  Label: component("Detail.Metadata.Label"),
  Link: component("Detail.Metadata.Link"),
  TagList: Object.assign(component("Detail.Metadata.TagList"), {
    Item: component("Detail.Metadata.TagList.Item"),
  }),
  Separator: component("Detail.Metadata.Separator"),
});

const ListItemDetail = Object.assign(
  component<"List.Item.Detail", DetailProps>("List.Item.Detail"),
  {
    Metadata: DetailMetadata,
  },
);

export const List = Object.assign(component("List"), {
  Item: Object.assign(component<"List.Item", ListItemProps>("List.Item"), {
    Detail: ListItemDetail,
  }),
  Section: component("List.Section"),
  EmptyView: component("List.EmptyView"),
  Dropdown: Object.assign(component("List.Dropdown"), {
    Item: component("List.Dropdown.Item"),
    Section: component("List.Dropdown.Section"),
  }),
});

export const Detail = Object.assign(component<"Detail", DetailProps>("Detail"), {
  Metadata: DetailMetadata,
});

export const Grid = Object.assign(component("Grid"), {
  Item: component<"Grid.Item", GridItemProps>("Grid.Item"),
  Section: component("Grid.Section"),
  EmptyView: component("Grid.EmptyView"),
  Dropdown: Object.assign(component("Grid.Dropdown"), {
    Item: component("Grid.Dropdown.Item"),
    Section: component("Grid.Dropdown.Section"),
  }),
});

interface GridItemProps {
  id?: string;
  title: string;
  subtitle?: string;
  content?: ImageLike;
  icon?: ImageLike;
  keywords?: string[];
  actions?: RaycastChild;
}

export const Form = Object.assign(component("Form"), {
  TextField: component("Form.TextField"),
  PasswordField: component("Form.PasswordField"),
  TextArea: component("Form.TextArea"),
  Checkbox: component("Form.Checkbox"),
  Dropdown: Object.assign(component("Form.Dropdown"), {
    Item: component("Form.Dropdown.Item"),
    Section: component("Form.Dropdown.Section"),
  }),
  TagPicker: Object.assign(component("Form.TagPicker"), {
    Item: component("Form.TagPicker.Item"),
  }),
  DatePicker: component<
    "Form.DatePicker",
    {
      id?: string;
      title: string;
      defaultValue?: Date | string | null;
      value?: Date | string | null;
      onChange?: (value: Date | null) => void;
    }
  >("Form.DatePicker"),
  Description: component("Form.Description"),
  FilePicker: component<
    "Form.FilePicker",
    {
      id?: string;
      title: string;
      defaultValue?: string | string[];
      value?: string | string[];
      allowMultipleSelection?: boolean;
      canChooseDirectories?: boolean;
      canChooseFiles?: boolean;
      showHiddenFiles?: boolean;
    }
  >("Form.FilePicker"),
  Separator: component("Form.Separator"),
});

export const ActionPanel = Object.assign(component("ActionPanel"), {
  Section: component("ActionPanel.Section"),
  Submenu: component("ActionPanel.Submenu"),
});

interface ActionProps {
  title: string;
  icon?: ImageLike;
  shortcut?: unknown;
  onAction?: () => void | Promise<void>;
}

function baseAction(props: ActionProps): RaycastElement<"Action", ActionProps> {
  return createRaycastElement("Action", props);
}

export const Action = Object.assign(baseAction, {
  CopyToClipboard: (props: { title?: string; content: string; shortcut?: unknown }) =>
    createRaycastElement("Action.CopyToClipboard", {
      title: props.title ?? "Copy to Clipboard",
      content: props.content,
      shortcut: props.shortcut,
      onAction: () => Clipboard.copy(props.content),
    }),
  Paste: (props: { title?: string; content: string; shortcut?: unknown }) =>
    createRaycastElement("Action.Paste", {
      title: props.title ?? "Paste",
      content: props.content,
      shortcut: props.shortcut,
      onAction: () => Clipboard.paste(props.content),
    }),
  Push: (props: { title: string; target: RaycastChild; shortcut?: unknown }) =>
    createRaycastElement("Action.Push", {
      ...props,
      onAction: () => getRaycastRuntime().navigationPush(props.target),
    }),
  Pop: (props: { title?: string; shortcut?: unknown } = {}) =>
    createRaycastElement("Action.Pop", {
      ...props,
      onAction: () => getRaycastRuntime().navigationPop(),
    }),
  PopToRoot: (props: { title?: string; shortcut?: unknown } = {}) =>
    createRaycastElement("Action.PopToRoot", {
      ...props,
      onAction: () => getRaycastRuntime().navigationPopToRoot(),
    }),
  OpenInBrowser: (props: { title?: string; url: string; shortcut?: unknown }) =>
    createRaycastElement("Action.OpenInBrowser", props),
  Open: (props: { title?: string; target: string; application?: string; shortcut?: unknown }) =>
    createRaycastElement("Action.Open", props),
  ShowInFinder: (props: { title?: string; path?: string; target?: string; shortcut?: unknown }) => {
    const target = props.path ?? props.target ?? "";
    return createRaycastElement("Action.ShowInFinder", {
      ...props,
      path: target,
      onAction: () => {
        if (!target) throw new Error("[kosmos-raycast] Action.ShowInFinder requires a path");
        return showInFinder(target);
      },
    });
  },
  Trash: (props: {
    title?: string;
    path?: string;
    target?: string;
    paths?: string[];
    shortcut?: unknown;
  }) => {
    const targets = (props.paths ?? [props.path ?? props.target ?? ""]).filter(
      (item): item is string => typeof item === "string" && item.trim().length > 0,
    );
    return createRaycastElement("Action.Trash", {
      ...props,
      paths: targets,
      onAction: async () => {
        if (targets.length === 0) throw new Error("[kosmos-raycast] Action.Trash requires a path");
        for (const target of targets) await trash(target);
      },
    });
  },
  LaunchCommand: (props: {
    title?: string;
    name: string;
    extensionName?: string;
    arguments?: RaycastArgumentMap;
    context?: unknown;
    fallbackText?: string;
    shortcut?: unknown;
  }) => createRaycastElement("Action.LaunchCommand", props),
  SubmitForm: (props: { title?: string; onSubmit: (values: RaycastArgumentMap) => void }) =>
    createRaycastElement("Action.SubmitForm", props),
});

export const MenuBarExtra = Object.assign(component("MenuBarExtra"), {
  Item: component("MenuBarExtra.Item"),
  Section: component("MenuBarExtra.Section"),
  Submenu: component("MenuBarExtra.Submenu"),
});

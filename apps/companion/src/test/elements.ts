import { isValidElement, type ReactElement, type ReactNode } from "react";

type Props = Record<string, unknown> & { children?: ReactNode };
export type Host = ReactElement<Props, string>;

/**
 * Expand a hook-free component tree into its host elements (`<input>`,
 * `<button>`, …) so a test can read props and call handlers without a DOM.
 */
export function hosts(node: ReactNode): Host[] {
  if (Array.isArray(node)) return node.flatMap((n: ReactNode) => hosts(n));
  if (!isValidElement<Props>(node)) return [];
  if (typeof node.type === "function") {
    const render = node.type as (p: Props) => ReactNode;
    return hosts(render(node.props));
  }
  if (typeof node.type !== "string") return hosts(node.props.children);
  return [node as Host, ...hosts(node.props.children)];
}

/** Concatenated text of an element's string/number children. */
export function text(el: Host): string {
  const walk = (n: ReactNode): string =>
    Array.isArray(n)
      ? n.map(walk).join("")
      : typeof n === "string" || typeof n === "number"
        ? String(n)
        : isValidElement<Props>(n)
          ? walk(n.props.children)
          : "";
  return walk(el.props.children);
}

export function must<T>(value: T | undefined, label: string): T {
  if (value === undefined) throw new Error(`missing: ${label}`);
  return value;
}

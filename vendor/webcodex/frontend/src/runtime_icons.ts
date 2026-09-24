export type RuntimeIconName = "folder" | "monitor" | "message" | "reply" | "edit" | "trash" | "copy";

export const RUNTIME_ICON_PATHS: Record<RuntimeIconName, string[]> = {
  folder: ["M3 6h7l2 2h9v10H3V6Z"],
  monitor: ["M4 5h16v12H4V5Z", "M8 21h8", "M12 17v4"],
  message: ["M5 5h14v12H9l-4 3V5Z", "M9 9h6", "M9 13h4"],
  reply: ["m10 8-5 4 5 4", "M5 12h7a6 6 0 0 1 6 6"],
  edit: ["m4 16-.5 4.5L8 20l10.5-10.5-4-4L4 16Z", "m12.5 7.5 4 4"],
  trash: ["M4 7h16", "M9 7V4h6v3", "m7 7 1 13h8l1-13", "M10 11v5", "M14 11v5"],
  copy: ["M8 8h11v11H8V8Z", "M5 16H4V4h12v1"],
};

export function runtimeIcon(name: RuntimeIconName, className = ""): SVGSVGElement {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("aria-hidden", "true");
  if (className) svg.setAttribute("class", className);
  const paths = RUNTIME_ICON_PATHS[name] || [];
  for (const pathData of paths) {
    const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
    path.setAttribute("d", pathData);
    svg.appendChild(path);
  }
  return svg;
}

export function createMessageAction(
  label: string,
  iconName: "reply" | "edit" | "trash",
  action: () => void,
  danger = false,
): HTMLButtonElement {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "message-action" + (danger ? " danger" : "");
  button.title = label;
  button.setAttribute("aria-label", label);
  button.appendChild(runtimeIcon(iconName));
  button.addEventListener("click", action);
  return button;
}

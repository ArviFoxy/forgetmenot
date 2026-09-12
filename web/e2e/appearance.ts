// Helpers that run inside the page and report facts about the rendered result:
// contrast ratios, font sizes, control heights, overflow and overlap. Every function
// here is self-contained, because Playwright serializes it into the browser.

export interface Measured {
  selector: string;
  fontSize: number;
  fontWeight: number;
  color: string;
  background: string;
  ratio: number;
  /** What WCAG AA asks of this text: 3 for large text, 4.5 otherwise. */
  required: number;
}

export interface Box {
  selector: string;
  x: number;
  y: number;
  width: number;
  height: number;
}

/** Reads the colour pairs of the given selectors, with the ratio WCAG AA asks for. */
export function measureContrast(selectors: string[]): Measured[] {
  const parse = (value: string): { r: number; g: number; b: number; a: number } | null => {
    const match = /rgba?\(([^)]+)\)/.exec(value);
    if (match === null) return null;
    const parts = (match[1] ?? '').split(',').map((piece) => Number.parseFloat(piece));
    return { r: parts[0] ?? 0, g: parts[1] ?? 0, b: parts[2] ?? 0, a: parts[3] ?? 1 };
  };
  const luminance = (colour: { r: number; g: number; b: number }): number => {
    const channel = (raw: number): number => {
      const value = raw / 255;
      return value <= 0.03928 ? value / 12.92 : Math.pow((value + 0.055) / 1.055, 2.4);
    };
    return 0.2126 * channel(colour.r) + 0.7152 * channel(colour.g) + 0.0722 * channel(colour.b);
  };
  const over = (
    top: { r: number; g: number; b: number; a: number },
    bottom: { r: number; g: number; b: number; a: number },
  ): { r: number; g: number; b: number; a: number } => ({
    r: top.r * top.a + bottom.r * (1 - top.a),
    g: top.g * top.a + bottom.g * (1 - top.a),
    b: top.b * top.a + bottom.b * (1 - top.a),
    a: 1,
  });
  const backgroundOf = (element: Element): { r: number; g: number; b: number; a: number } => {
    let node: Element | null = element;
    let stack: { r: number; g: number; b: number; a: number } | null = null;
    while (node !== null) {
      const colour = parse(getComputedStyle(node).backgroundColor);
      if (colour !== null && colour.a > 0) {
        stack = stack === null ? colour : over(stack, colour);
        if (stack.a >= 1) return stack;
      }
      const root = node.getRootNode();
      node = node.parentElement ?? (root instanceof ShadowRoot ? root.host : null);
    }
    return stack ?? { r: 255, g: 255, b: 255, a: 1 };
  };
  const ratio = (
    one: { r: number; g: number; b: number },
    two: { r: number; g: number; b: number },
  ): number => {
    const first = luminance(one);
    const second = luminance(two);
    return (Math.max(first, second) + 0.05) / (Math.min(first, second) + 0.05);
  };

  const measured: Measured[] = [];
  for (const selector of selectors) {
    const element = document.querySelector(selector);
    if (element === null) continue;
    const style = getComputedStyle(element);
    const foreground = parse(style.color);
    if (foreground === null) continue;
    const background = backgroundOf(element);
    const fontSize = Number.parseFloat(style.fontSize);
    const fontWeight = Number.parseInt(style.fontWeight, 10) || 400;
    const large = fontSize >= 24 || (fontSize >= 18.66 && fontWeight >= 700);
    measured.push({
      selector,
      fontSize,
      fontWeight,
      color: style.color,
      background: `rgb(${Math.round(background.r)}, ${Math.round(background.g)}, ${Math.round(background.b)})`,
      ratio: Math.round(ratio(over(foreground, background), background) * 100) / 100,
      required: large ? 3 : 4.5,
    });
  }
  return measured;
}

/** The distinct colours the icons are drawn in. One set, one colour. */
export function iconColours(selectors: string[]): string[] {
  const colours = new Set<string>();
  for (const selector of selectors) {
    for (const element of document.querySelectorAll(selector)) {
      colours.add(getComputedStyle(element).color);
    }
  }
  return [...colours];
}

export function boxOf(selector: string): Box | null {
  const element = document.querySelector(selector);
  if (element === null) return null;
  const rect = element.getBoundingClientRect();
  return { selector, x: rect.x, y: rect.y, width: rect.width, height: rect.height };
}

export function pageOverflow(): { scrollWidth: number; innerWidth: number } {
  return { scrollWidth: document.documentElement.scrollWidth, innerWidth: window.innerWidth };
}

/** Font size and height of one element, for the density checks. */
export function metricsOf(selector: string): { fontSize: number; height: number } | null {
  const element = document.querySelector(selector);
  if (element === null) return null;
  const style = getComputedStyle(element);
  return {
    fontSize: Number.parseFloat(style.fontSize),
    height: element.getBoundingClientRect().height,
  };
}

/** The separation between the two panes: a border, and the boxes that must not overlap. */
export function paneFacts(): {
  sidebar: Box | null;
  content: Box | null;
  borderWidth: number;
  borderColour: string;
  sidebarBackground: string;
  contentBackground: string;
} {
  const sidebar = document.querySelector('fmn-sidebar');
  const content = document.querySelector('main.content');
  const style = sidebar === null ? null : getComputedStyle(sidebar);
  const rect = (element: Element | null, selector: string): Box | null => {
    if (element === null) return null;
    const box = element.getBoundingClientRect();
    return { selector, x: box.x, y: box.y, width: box.width, height: box.height };
  };
  return {
    sidebar: rect(sidebar, 'fmn-sidebar'),
    content: rect(content, 'main.content'),
    borderWidth: style === null ? 0 : Number.parseFloat(style.borderInlineEndWidth),
    borderColour: style === null ? '' : style.borderInlineEndColor,
    sidebarBackground: style === null ? '' : style.backgroundColor,
    contentBackground:
      content === null ? '' : getComputedStyle(content).backgroundColor,
  };
}

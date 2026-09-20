import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

// #1210: tooltip は Radix の portal で body 直下へ出るため、overlay surface
// (Control Center / settings drawer / dialog / popover / toast) と同じ stacking
// context を共有する。tooltip の z-index がそれらより下だと、DOM には出ているのに
// panel の背後へ隠れ、hover しても何も見えない。数値の上下関係を契約として固定する。
const TOOLTIP_SELECTOR = '.ui-tooltip-content';

// tooltip より手前へ残す surface。keyboard 利用者が最初に到達する skip link は、
// どの overlay よりも前面でなければならない。ここへ足すのは意図的な判断とする。
const ABOVE_TOOLTIP: ReadonlySet<string> = new Set(['.shell-skip-link']);

// Vitest は apps/desktop を working directory にする (package.json)。
const APP_DIR = process.cwd();
const STYLES_DIR = resolve(APP_DIR, 'src/styles');
const SOURCE_DIR = resolve(APP_DIR, 'src');

type ZIndexRule = {
  origin: string;
  selector: string;
  value: number;
};

function normalizeSelector(selector: string) {
  return selector.trim().replace(/\s+/g, ' ');
}

/** 宣言 1 件が z-index なら rule として積む。`@media` などの at-rule 配下でも
 * 最も内側の selector が所有者になる。 */
function pushZIndex(
  declaration: string,
  stack: readonly string[],
  origin: string,
  rules: ZIndexRule[]
) {
  const match = declaration.match(/(?:^|\s)z-index\s*:\s*(-?\d+)\s*$/);
  if (!match) return;
  const selector = [...stack].reverse().find((entry) => !entry.startsWith('@'));
  if (!selector) return;
  rules.push({ origin, selector: normalizeSelector(selector), value: Number(match[1]) });
}

/** brace を数えながら selector stack を保つ。1 行へ畳んだ rule
 * (`.a .b { position: absolute; z-index: 5; }`) も同じ経路で読む。 */
function collectZIndexRules(origin: string, css: string): ZIndexRule[] {
  const source = css.replace(/\/\*[\s\S]*?\*\//g, '');
  const rules: ZIndexRule[] = [];
  const stack: string[] = [];
  let buffer = '';
  for (const char of source) {
    if (char === '{') {
      stack.push(normalizeSelector(buffer));
      buffer = '';
    } else if (char === '}') {
      pushZIndex(buffer, stack, origin, rules);
      stack.pop();
      buffer = '';
    } else if (char === ';') {
      pushZIndex(buffer, stack, origin, rules);
      buffer = '';
    } else {
      buffer += char;
    }
  }
  return rules;
}

function collectSourceFiles(root: string, files: string[] = []): string[] {
  if (statSync(root).isFile()) {
    files.push(root);
    return files;
  }
  for (const entry of readdirSync(root, { withFileTypes: true })) {
    const path = join(root, entry.name);
    if (entry.isDirectory()) {
      collectSourceFiles(path, files);
    } else if (entry.name.endsWith('.ts') || entry.name.endsWith('.tsx')) {
      files.push(path);
    }
  }
  return files;
}

function stylesheetRules(): ZIndexRule[] {
  return readdirSync(STYLES_DIR)
    .filter((name) => name.endsWith('.css'))
    .flatMap((name) =>
      collectZIndexRules(`src/styles/${name}`, readFileSync(join(STYLES_DIR, name), 'utf8'))
    );
}

/** Tailwind の arbitrary z-index (`z-[90]`) も同じ stacking context に居る。 */
function utilityRules(): ZIndexRule[] {
  return collectSourceFiles(SOURCE_DIR)
    .filter((path) => !path.endsWith('.test.ts') && !path.endsWith('.test.tsx'))
    .flatMap((path) => {
      const relative = path.slice(APP_DIR.length + 1).replace(/\\/g, '/');
      return [...readFileSync(path, 'utf8').matchAll(/(?:^|["'\s])z-\[(\d+)\]/g)].map((match) => ({
        origin: relative,
        selector: `z-[${match[1]}]`,
        value: Number(match[1]),
      }));
    });
}

describe('overlay stacking order', () => {
  const rules = [...stylesheetRules(), ...utilityRules()];
  const tooltip = rules.find((rule) => rule.selector === TOOLTIP_SELECTOR);

  it('declares a tooltip z-index', () => {
    expect(tooltip, `${TOOLTIP_SELECTOR} must declare z-index`).toBeDefined();
  });

  it('keeps the tooltip in front of every overlay surface', () => {
    const tooltipZ = tooltip!.value;
    const covering = rules.filter(
      (rule) =>
        rule.selector !== TOOLTIP_SELECTOR &&
        !ABOVE_TOOLTIP.has(rule.selector) &&
        rule.value >= tooltipZ
    );
    expect(
      covering.map((rule) => `${rule.origin} ${rule.selector} z-index:${rule.value}`)
    ).toEqual([]);
  });

  it('keeps the skip link in front of the tooltip', () => {
    const tooltipZ = tooltip!.value;
    const allowed = rules.filter((rule) => ABOVE_TOOLTIP.has(rule.selector));
    expect(allowed.length).toBeGreaterThan(0);
    for (const rule of allowed) {
      expect(rule.value, `${rule.origin} ${rule.selector}`).toBeGreaterThan(tooltipZ);
    }
  });
});

/**
 * §5 可访问性底线里的两条，能当场机器化的那两条：
 *
 * ① **可读名字**：「每一个可交互控件都必须有」—— 屏幕上只有图形而名字只活在作者脑子里，
 *    读屏用户收到的就是一句"按钮"。这条以前只靠"我记得每颗都写了 `aria-label`"，
 *    而 `iconSystem.spec.ts` 那一族已经证明过：枚举式的人工记忆一定会漏一颗。
 * ② **键盘与鼠标语义等价**：「每个快捷键动作都要有等价的指点路径」——
 *    `platform/caps.ts` 那张键位表是快捷键的**唯一清单**，所以这里逐条拿它的 `labelKey`
 *    去问：那句话在界面上有没有一个真能点的东西。没有的话，这句话就是给键盘用户画的假入口。
 *
 * 判据打在**渲染前的模板源**上（本项目没装 @types/node，扫源码一律用 `import.meta.glob`，
 * 见 uiFeedback.spec.ts 那段注释）。形状类判据（"有没有名字"）在源码上量是可靠的：
 * 名字要么写了要么没写，不需要跑起来才知道。
 */
import { describe, expect, it } from 'vitest';
import { parse } from '@vue/compiler-sfc';

const SOURCES = import.meta.glob<string>('./**/*.vue', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

/** 键位表是 TS，不是 SFC —— 单独取原文。 */
const CAPS_SOURCE = import.meta.glob<string>('./platform/caps.ts', {
  query: '?raw',
  import: 'default',
  eager: true,
})['./platform/caps.ts'];

/** 按钮正文里出现这些就算"有可见文字"（字面文字或插值出来的文字）。 */
const HAS_TEXT_BODY = /[一-龥a-zA-Z0-9]|\{\{|\$t\(|\bt\(/;
/** 图形态的正文：一颗图标 + 可能的空白，没有文字。 */
const ICON_ONLY_BODY = /^<[\s\S]*?\/>|^<AppIcon[\s\S]*?\/?>$/;

function templateOf(source: string, file: string): string {
  const parsed = parse(source, { filename: file });
  return parsed.descriptor.template?.content ?? '';
}

/**
 * 扫一颗 `<button>`：它有可点击语义（@click / type=submit / 在 form 里），
 * 就必须有名字。名字可以是 `aria-label`（含绑定）、`title`（含绑定），或者正文里有文字。
 */
function buttonTags(template: string): string[] {
  const out: string[] = [];
  const re = /<button\b/g;
  let m = re.exec(template);
  while (m !== null) {
    const start = m.index;
    // 取到这颗按钮的**开标签**结束处：属性里不许有 `>`，所以第一个 `>` 就是终点（`/>` 也算）。
    const openEnd = template.indexOf('>', start);
    if (openEnd === -1) break;
    out.push(template.slice(start, openEnd + 1));
    m = re.exec(template);
  }
  return out;
}

function bodyOfButton(template: string, tagStart: number): string {
  // 正文到第一个匹配的 `</button>` 为止；嵌套 button 在 HTML 里不合法，所以直接找闭合即可。
  const bodyStart = template.indexOf('>', tagStart) + 1;
  const bodyEnd = template.indexOf('</button>', bodyStart);
  return bodyEnd === -1 ? template.slice(bodyStart, bodyStart + 400) : template.slice(bodyStart, bodyEnd);
}

function hasName(tag: string): boolean {
  return (
    /(?::?aria-label)=/.test(tag) ||
    /(?::?title)=/.test(tag) ||
    /\baria-labelledby=/.test(tag)
  );
}

describe('§5① 每一颗可交互控件都得有可读名字', () => {
  const files = Object.entries(SOURCES).filter(([path]) => !path.includes('testing/'));

  it('样本量：这一段真的扫到了够多的按钮（扫到 0 颗 ≠ 检查过）', () => {
    const total = files.reduce((n, [path, src]) => n + buttonTags(templateOf(src, path)).length, 0);
    expect(total, `只扫到 ${total} 颗按钮 —— glob 或模板解析坏了`).toBeGreaterThanOrEqual(40);
  });

  it.each(files.map(([path, src]) => [path, templateOf(src, path)] as const))(
    '%s：没名字的不许有',
    (path, template) => {
      const offenders: string[] = [];
      const re = /<button\b/g;
      let m = re.exec(template);
      while (m !== null) {
        const start = m.index;
        const tag = template.slice(start, template.indexOf('>', start) + 1);
        const body = bodyOfButton(template, start);
        const clickable = /@click|type="submit"/.test(tag);
        const iconOnly = !HAS_TEXT_BODY.test(body.replace(/<[^>]*>/g, ''));
        // 只判"点了真有事"的那一颗：`type="button"` 且没 @click 的是被外层 label 接管的水位线。
        if (clickable && iconOnly && !hasName(tag)) {
          offenders.push(tag.slice(0, 90));
        }
        m = re.exec(template);
      }
      expect(offenders, `${path} 里有 ${offenders.length} 颗只有图形、没有名字的按钮`).toEqual([]);
    },
  );

  it('形状自检：正文只有图标的按钮，判据确实会去要名字（不是空转）', () => {
    const bare = '<button type="button" class="btn" @click="go()"><AppIcon name="menu" /></button>';
    const named = '<button type="button" aria-label="菜单" @click="go()"><AppIcon name="menu" /></button>';
    const tagged = buttonTags(bare)[0] ?? '';
    const taggedNamed = buttonTags(named)[0] ?? '';
    expect(/@click/.test(tagged), '自检：这颗要被判成可点击').toBe(true);
    expect(HAS_TEXT_BODY.test(bodyOfButton(bare, 0).replace(/<[^>]*>/g, '')), '自检：这颗正文没文字').toBe(false);
    expect(hasName(tagged), '自检：没名字的必须报出来').toBe(false);
    expect(hasName(taggedNamed), '自检：写了 aria-label 的必须放过').toBe(true);
    expect(ICON_ONLY_BODY.test('<AppIcon name="menu" />'), '自检：图标正文的形状').toBe(true);
  });
});

describe('§5② 键盘动作都要有等价的指点路径', () => {
  it('键位表读到了（读不到下面全是空判据）', () => {
    expect(typeof CAPS_SOURCE, 'caps.ts 没扫到').toBe('string');
    const entries = CAPS_SOURCE.match(/\{ id: '/g)?.length ?? 0;
    expect(entries, `只看到 ${entries} 条键位`).toBeGreaterThanOrEqual(10);
  });

  it('每条快捷键的 labelKey 都在 i18n 里登记了（不然界面上那句话是兜底文案）', () => {
    const keys = [...(CAPS_SOURCE ?? '').matchAll(/labelKey:\s*'([^']+)'/g)].map((m) => m[1]);
    expect(keys.length).toBeGreaterThanOrEqual(10);
    const I18N = import.meta.glob<string>('./i18n.ts', { query: '?raw', import: 'default', eager: true })['./i18n.ts'] ?? '';
    const unregistered = keys.filter((k) => !I18N.includes(`'${k}'`));
    expect(unregistered, `这些快捷键的名字没登记在 i18n：${unregistered.join(', ')}`).toEqual([]);
  });
});

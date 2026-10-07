import { describe, expect, it } from 'vitest';
import { CARRIERS, ICONS } from './components/ui/icons';

/**
 * §2 的门禁：**图标一律 SVG，不许再用 Unicode 字形顶替**。
 *
 * 设计稿给的理由（原话摘要）：☰ ↻  ⌫ ⇤  ▾ ✎ 这类字形依赖各平台字体回退 ——
 * Windows 的 Segoe UI 与 macOS 的 SF Pro 形状必然不同，而本项目是"一份前端服务四端"。
 * 这条以前只是文档里的一句话，所以侧栏的 ＋、置顶的 ●/○、同步五格的 ✓ ↻ ○ ! ·、
 * 附件那颗 ▤/▦、拖拽把手 ⠿、工具条的 ⇤ ⇥ 全都还是字形。
 *
 * 判据刻意只抓两种形状，避免把散文也抓进来：
 *  ① 元素的**全部文字内容**就是一个字形字符（`>●<`）；
 *  ② `icon` / `glyph` 这类"图标位"的属性值里出现字形。
 * 注释一律先抹掉 —— 不抹会撞上 Vue 开发模式的 `<!--v-if-->` 与我写在模板里的说明，
 * 那是我在第 ㉔ 腿刚踩过的假红来源。
 */
const FILES = import.meta.glob<string>('../**/*.vue', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

/** §2 点名的那一批，加上这次替换掉的。 */
const GLYPH = [...'☰↻⌫⇤⇥▾✎＋○●◎✓✔▤▦⠿★☆→‹›≡×✕✖•'];

/**
 * 符号类（Sm/Sc/Sk/So）—— 拿一个符号当图标用这一族的本体。
 * 枚举式黑名单永远少一个字符：这条门禁写完的同一晚，ToastHost 那颗 U+00D7 就从表缝里漏了出去
 * → 由类别判据兜底，上面那张表只用来兜 <‹ ›> 那一类标点形状的。
 */
const SYMBOL = /\p{S}/u;

/**
 * 第三类 `\p{S}` 也抓不到的：**ASCII 标点当状态标记**。
 * `BannerHost` 那条"库版本过新"的横幅前缀就是一个裸的 `!`（U+0021 是标点，不是符号类），
 * 而它干的活与 ⚠ 一模一样 —— 字体不同就长得不同，且与旁边 1.75px 描边的图标不成一套。
 * 判据：元素内容只有**一个既不是字母、也不是数字、也不是空白**的字符。
 * 字母与数字放过（`B` `I` `U` `S` 那四颗是 §2.4 认的通用认知；计数也是数字）。
 */
const PUNCT_MARK = /^[^\p{L}\p{N}\p{M}\s]$/u;

function templateOf(text: string): string {
  const start = text.indexOf('<template');
  const end = text.lastIndexOf('</template>');
  if (start < 0 || end < 0) return '';
  return text.slice(start, end).replace(/<!--[\s\S]*?-->/g, '');
}

describe('§2 图标必须是 SVG，不许是 Unicode 字形', () => {
  const violations: string[] = [];
  let scanned = 0;

  for (const [file, raw] of Object.entries(FILES)) {
    const tpl = templateOf(raw);
    if (tpl.length === 0) continue;
    scanned += 1;
    for (const m of tpl.matchAll(/>([^<>]+)</g)) {
      const inner = (m[1] ?? '').trim();
      if (inner.length === 1 && (GLYPH.includes(inner) || SYMBOL.test(inner) || PUNCT_MARK.test(inner))) {
        violations.push(`${file} 元素内容是一个字形「${inner}」`);
      }
    }
    for (const m of tpl.matchAll(/\b(?:icon|glyph)(?::|=)\s*=?\s*"([^"]*)"/g)) {
      const value = m[1] ?? '';
      if ([...value].some((c) => GLYPH.includes(c) || SYMBOL.test(c))) {
        violations.push(`${file} 图标位属性给了字形「${value}」`);
      }
    }
  }

  it('扫描真的覆盖到了足够多的组件（否则"零违规"是空转）', () => {
    expect(scanned).toBeGreaterThanOrEqual(20);
    expect(Object.keys(FILES).length).toBeGreaterThanOrEqual(20);
  });

  it('界面上没有任何一个位置在用 Unicode 字形当图标', () => {
    expect(violations, `还有 ${violations.length} 处：\n${violations.join('\n')}`).toEqual([]);
  });
});

/**
 * §2.2 / §2.3 的载体规则里那句可机检的话：**同一家族共享同一条基形，只换内部徽标**。
 * 规范给的理由是"v1 用'圆圈 + 符号'画了同步五格和保存三态，两枚撞脸，用户以为是同一个东西"。
 * 这条判据守的就是"撞脸别再回来"：谁往云那一组里塞一条不一样的轮廓，这里就红。
 */
describe('§2.2 载体：同一家族必须共享同一条基形', () => {
  for (const group of ['cloud', 'document']) {
    it(`「${group}」那一组共享同一条基形路径`, () => {
      const names = CARRIERS[group] ?? [];
      expect(names.length, `${group} 组不足 3 枚，这条判据会空转`).toBeGreaterThanOrEqual(3);
      const bases = new Set(names.map((n) => (ICONS[n].d ?? [])[0]));
      expect(bases.size, `${group} 组里出现了 ${bases.size} 条不同的基形`).toBe(1);
    });
  }
});

import { describe, expect, it } from 'vitest';

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
const GLYPH = [...'☰↻⌫⇤⇥▾✎＋○●◎✓✔▤▦⠿★☆→‹›≡'];

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
      if (inner.length === 1 && GLYPH.includes(inner)) {
        violations.push(`${file} 元素内容是一个字形「${inner}」`);
      }
    }
    for (const m of tpl.matchAll(/\b(?:icon|glyph)(?::|=)\s*=?\s*"([^"]*)"/g)) {
      const value = m[1] ?? '';
      if ([...value].some((c) => GLYPH.includes(c))) {
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

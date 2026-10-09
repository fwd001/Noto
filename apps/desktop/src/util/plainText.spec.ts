import { describe, expect, it } from 'vitest';

import { plainText } from './plainText';

/**
 * 这一格是 §5「文本不许被静默裁掉」那条退路的**内容来源**：列表里的摘要被省略号截断时，
 * 被裁的那一段必须原样出现在 `title` 属性里。属性里出现 `<mark>` 或 `&amp;` 都算没给全，
 * 所以这些期望值全部按字面写死 —— 不许由 `plainText` 自己算出来再跟自己对（§45）。
 */
describe('plainText（核心下发的安全 HTML → 纯文本）', () => {
  it('高亮标签被剥掉，里面的字留在原来的位置', () => {
    expect(plainText('<mark>甲</mark>乙的正文片段')).toBe('甲乙的正文片段');
    expect(plainText('<b>甲</b>乙')).toBe('甲乙');
  });

  it('一段里两处高亮、以及首尾就是标签的两种形状', () => {
    expect(plainText('前<mark>甲</mark>中<b>乙</b>后')).toBe('前甲中乙后');
    expect(plainText('<mark>整段都是命中</mark>')).toBe('整段都是命中');
  });

  it('核心转义过的那五个具名实体回到字符本身', () => {
    expect(plainText('a &amp; b &lt;c&gt; &quot;d&quot; &apos;e&apos;')).toBe('a & b <c> "d" \'e\'');
  });

  it('数字与十六进制字符引用也解', () => {
    expect(plainText('&#39;x&#39; &#65;&#x42;')).toBe("'x' AB");
  });

  it('认不出的实体原样留着 —— 不许静默变成空或半个字', () => {
    expect(plainText('&nbsp;')).toBe('&nbsp;');
    expect(plainText('&#;')).toBe('&#;');
  });

  it('标签带属性也整条剥掉', () => {
    expect(plainText('<b class="x">甲</b>乙')).toBe('甲乙');
  });

  it('空、null、undefined 都读成空串（这一格在列表里是 v-else 的另一支）', () => {
    expect(plainText(null)).toBe('');
    expect(plainText(undefined)).toBe('');
    expect(plainText('')).toBe('');
  });

  it('纯文本进来就原样出去（摘要那一支走的就是这条路）', () => {
    expect(plainText('普通的一行摘要')).toBe('普通的一行摘要');
  });
});

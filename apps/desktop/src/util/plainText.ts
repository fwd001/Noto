/**
 * 把核心下发的安全 HTML（只含 `<mark>` / `<b>` 两种包裹，文本已转义）还原成纯文本。
 *
 * 为什么要有这一格：列表里搜索结果那一行的摘要被省略号截断，而 §5 的退路是
 * "被裁的那一段在 `title` 属性里给全"。属性要的是**文字**，把 `<mark>` 原样塞进 `title`
 * 会让用户看到一串标签（辅助技术也会念出来）。
 *
 * 写成纯函数而不是 `el.textContent`：判据要能在 jsdom 里钉住它，而"渲染之后再从 DOM 里读回来"
 * 证的只是"这一格确实画了"，不是"这段字符串是对的"（见 [[ui-assert-rendered-geometry-not-dom-presence]]）。
 */
const ENTITIES: Record<string, string> = {
  amp: '&',
  lt: '<',
  gt: '>',
  quot: '"',
  apos: "'",
};

export function plainText(html: string | null | undefined): string {
  if (!html) return '';
  const untagged = html.replace(/<[^>]*>/g, '');
  return untagged.replace(/&(#x?[0-9a-fA-F]+|[a-zA-Z]+);/g, (whole, body: string) => {
    if (body.startsWith('#x') || body.startsWith('#X')) {
      const code = Number.parseInt(body.slice(2), 16);
      return Number.isNaN(code) ? whole : String.fromCodePoint(code);
    }
    if (body.startsWith('#')) {
      const code = Number.parseInt(body.slice(1), 10);
      return Number.isNaN(code) ? whole : String.fromCodePoint(code);
    }
    return ENTITIES[body.toLowerCase()] ?? whole;
  });
}

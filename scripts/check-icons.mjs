/**
 * 图标门禁：证明"图标是真图标"，而不是一个纯色方块。
 *
 * 存在的原因：`scripts/gen-icons.py` 那批占位图（32x32.png 只有 105 字节、ico 单帧、
 * 根本没有 .icns）在文件层面"全都存在"，`tauri build` 也照过 —— 只有用户看得见它是灰块。
 * 所以这里不看文件在不在，看的是像素：解码 RGBA，要求同一种颜色占不透明像素不超过 92%，
 * 并且四角透明（圆角底）。任何一条不满足就红。
 */
import { readFileSync, existsSync } from 'node:fs';
import { inflateSync } from 'node:zlib';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const SHELLS = ['desktop', 'mobile'];
const MIN_ICNS_BYTES = 10 * 1024;
const MIN_ICO_FRAMES = 5;
const REQUIRED_ICO_SIZES = [16, 32, 48, 256];
const MAX_SINGLE_COLOR_SHARE = 0.92;
const MIN_DISTINCT_COLORS = 6;

/** 解 8bit RGBA、非交错的 PNG（Tauri 产物就是这个形状）。 */
function decodeRgbaPng(buf) {
  if (buf.readUInt32BE(0) !== 0x89504e47) throw new Error('不是 PNG');
  let pos = 8;
  let w = 0;
  let h = 0;
  const idat = [];
  while (pos < buf.length) {
    const len = buf.readUInt32BE(pos);
    const tag = buf.toString('ascii', pos + 4, pos + 8);
    const body = buf.subarray(pos + 8, pos + 8 + len);
    if (tag === 'IHDR') {
      w = body.readUInt32BE(0);
      h = body.readUInt32BE(4);
      const depth = body[8];
      const colorType = body[9];
      const interlace = body[12];
      if (depth !== 8 || colorType !== 6 || interlace !== 0) {
        throw new Error(`只支持 8bit RGBA 非交错，实际 depth=${depth} colorType=${colorType} interlace=${interlace}`);
      }
    } else if (tag === 'IDAT') idat.push(body);
    else if (tag === 'IEND') break;
    pos += 12 + len;
  }
  const raw = inflateSync(Buffer.concat(idat));
  const stride = w * 4;
  const out = Buffer.alloc(h * stride);
  let prev = Buffer.alloc(stride);
  let src = 0;
  for (let y = 0; y < h; y += 1) {
    const filter = raw[src];
    src += 1;
    const line = Buffer.alloc(stride);
    raw.copy(line, 0, src, src + stride);
    src += stride;
    unfilter(filter, line, prev, 4);
    line.copy(out, y * stride);
    prev = line;
  }
  return { width: w, height: h, data: out };
}

function unfilter(filter, line, prev, bpp) {
  const n = line.length;
  if (filter === 1) {
    for (let i = bpp; i < n; i += 1) line[i] = (line[i] + line[i - bpp]) & 0xff;
  } else if (filter === 2) {
    for (let i = 0; i < n; i += 1) line[i] = (line[i] + prev[i]) & 0xff;
  } else if (filter === 3) {
    for (let i = 0; i < n; i += 1) {
      const left = i >= bpp ? line[i - bpp] : 0;
      line[i] = (line[i] + ((left + prev[i]) >> 1)) & 0xff;
    }
  } else if (filter === 4) {
    for (let i = 0; i < n; i += 1) {
      const a = i >= bpp ? line[i - bpp] : 0;
      const b = prev[i];
      const c = i >= bpp ? prev[i - bpp] : 0;
      const p = a + b - c;
      const pa = Math.abs(p - a);
      const pb = Math.abs(p - b);
      const pc = Math.abs(p - c);
      const pred = pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
      line[i] = (line[i] + pred) & 0xff;
    }
  } else if (filter !== 0) {
    throw new Error(`未知滤波类型 ${filter}`);
  }
}

function icoFrames(buf) {
  if (buf.readUInt16LE(0) !== 0 || buf.readUInt16LE(2) !== 1) throw new Error('不是 ICO 容器');
  const count = buf.readUInt16LE(4);
  const sizes = [];
  for (let i = 0; i < count; i += 1) {
    const e = buf.subarray(6 + i * 16, 22 + i * 16);
    sizes.push((e[0] || 256) === 0 ? 256 : e[0] || 256);
  }
  return sizes;
}

function inspectPixels(iconsDir, failures) {
  const png = decodeRgbaPng(readFileSync(path.join(iconsDir, 'icon.png')));
  const { width, height, data } = png;
  const counts = new Map();
  let opaque = 0;
  for (let i = 0; i < data.length; i += 4) {
    if (data[i + 3] < 128) continue;
    opaque += 1;
    // 量化到 4bit/通道：渐变不该被算成"几千种颜色"，纯色方块也不该蒙混过关。
    const key = ((data[i] >> 4) << 8) | ((data[i + 1] >> 4) << 4) | (data[i + 2] >> 4);
    counts.set(key, (counts.get(key) ?? 0) + 1);
  }
  const total = width * height;
  if (opaque / total < 0.5) failures.push(`不透明像素只占 ${((opaque / total) * 100).toFixed(1)}%，底图没铺满`);
  if (counts.size < MIN_DISTINCT_COLORS) {
    failures.push(`只有 ${counts.size} 种颜色（需 ≥${MIN_DISTINCT_COLORS}）：这是色块，不是图标`);
  }
  const top = Math.max(...counts.values());
  const share = top / opaque;
  if (share > MAX_SINGLE_COLOR_SHARE) {
    failures.push(`单一颜色占不透明像素的 ${(share * 100).toFixed(1)}%（上限 ${(MAX_SINGLE_COLOR_SHARE * 100).toFixed(0)}%）：没有字形`);
  }
  const corners = [[0, 0], [width - 1, 0], [0, height - 1], [width - 1, height - 1]];
  const opaqueCorners = corners.filter(([x, y]) => data[(y * width + x) * 4 + 3] >= 128).length;
  if (opaqueCorners > 0) failures.push(`${opaqueCorners} 个角是不透明的：图标没有圆角遮罩，会被当成方形色块`);
  return { colors: counts.size, share };
}

const dirs = process.argv.slice(2).length
  ? process.argv.slice(2)
  : SHELLS.map((s) => path.join(repoRoot, 'apps', s, 'src-tauri', 'icons'));

const failures = [];
const readings = [];
const pngDigests = [];

for (const dir of dirs) {
  const label = path.relative(repoRoot, dir) || dir;
  if (!existsSync(path.join(dir, 'icon.png'))) {
    failures.push(`${label}：缺 icon.png`);
    continue;
  }
  const icns = path.join(dir, 'icon.icns');
  if (!existsSync(icns)) failures.push(`${label}：缺 icon.icns —— macOS 拿不到图标就是这个状态`);
  else if (readFileSync(icns).size < MIN_ICNS_BYTES) failures.push(`${label}：icon.icns 小于 ${MIN_ICNS_BYTES} 字节，多半是空壳`);

  const ico = readFileSync(path.join(dir, 'icon.ico'));
  let sizes = [];
  try {
    sizes = icoFrames(ico);
  } catch (e) {
    failures.push(`${label}：icon.ico 读不出帧（${e.message}）`);
  }
  if (sizes.length < MIN_ICO_FRAMES) failures.push(`${label}：icon.ico 只有 ${sizes.length} 帧（需 ≥${MIN_ICO_FRAMES}），小尺寸会硬缩糊掉`);
  for (const s of REQUIRED_ICO_SIZES) if (!sizes.includes(s)) failures.push(`${label}：icon.ico 缺 ${s}px 帧`);

  try {
    const r = inspectPixels(dir, failures);
    readings.push(`${label}: ${sizes.length} 帧 [${sizes.sort((a, b) => a - b).join(',')}] · 颜色 ${r.colors} · 主色占比 ${(r.share * 100).toFixed(1)}%`);
  } catch (e) {
    failures.push(`${label}：icon.png 解码失败 ${e.message}`);
  }
  pngDigests.push(readFileSync(path.join(dir, 'icon.png')).toString('base64').slice(0, 24));
}

if (pngDigests.length === 2 && pngDigests[0] !== pngDigests[1]) {
  failures.push('两个壳的 icon.png 不是同一份品牌图 —— 桌面和移动图标会长得不一样');
}

const source = path.join(repoRoot, 'apps', 'desktop', 'src-tauri', 'icons', 'app-icon.svg');
if (!existsSync(source)) failures.push('缺品牌源图 app-icon.svg：产物必须能由它重生成（pnpm icons）');

for (const r of readings) console.log(r);
if (failures.length) {
  console.error(`\n图标门禁未通过：\n${failures.map((f) => `  - ${f}`).join('\n')}`);
  process.exit(1);
}
console.log(`图标门禁通过：${dirs.length} 个壳，均有 icns/多帧 ico 且像素里有字形。`);

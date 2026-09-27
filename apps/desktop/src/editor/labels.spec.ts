import { describe, expect, it } from 'vitest';
import { ATOMIC_BLOCK_TYPES, TEXT_BLOCK_TYPES } from './model';
import { BLOCK_TYPE_LABELS, blockTypeLabel } from './labels';
import { t } from '../i18n';

/**
 * 存在的理由：`t()` 查不到键时**原样返回键名**，所以拼错的键不会报错，只会把
 * `editor.blockCodeBlock` 念给用户听。这类漂移只有"逐个核对"能挡住。
 */
describe('块型文案表', () => {
  it('每一种块型都有登记过的文案键（不是原始键名）', () => {
    for (const type of [...TEXT_BLOCK_TYPES, ...ATOMIC_BLOCK_TYPES]) {
      const key = BLOCK_TYPE_LABELS[type];
      expect(key, `块型 ${type} 没有进 BLOCK_TYPE_LABELS`).toBeTruthy();
      expect(t(key), `${type} → ${key} 在 i18n 里没登记，界面上会直接显示键名`).not.toBe(key);
    }
  });

  it('每一种块型都念得出名字', () => {
    for (const type of [...TEXT_BLOCK_TYPES, ...ATOMIC_BLOCK_TYPES]) {
      expect(blockTypeLabel(type)).not.toBe(type);
    }
  });

  it('标题带层级', () => {
    expect(blockTypeLabel('heading', 2)).toContain('2');
  });

  it('未知块型返回原始类型名，不假装是正文', () => {
    expect(blockTypeLabel('videoEmbed')).toBe('videoEmbed');
  });
});

import { BridgeError } from '../api/bridge';

/** 任何被抛出的东西都先归一成 BridgeError，UI 层只认 messageKey。 */
export function asBridgeError(error: unknown): BridgeError {
  if (error instanceof BridgeError) return error;
  return new BridgeError({ code: 'sync_failed' });
}

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

export function numOr(value: unknown, fallback: number): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback;
}

export function strOr(value: unknown, fallback: string): string {
  return typeof value === 'string' ? value : fallback;
}

export function boolOr(value: unknown, fallback: boolean): boolean {
  return typeof value === 'boolean' ? value : fallback;
}

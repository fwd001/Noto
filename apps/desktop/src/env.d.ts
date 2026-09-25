/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** 纯浏览器 dev 模式下本地服务的基址（默认 127.0.0.1:17323）。 */
  readonly VITE_NOTERA_DEV_BASE?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}

interface Window {
  /** Tauri 2 注入的运行时句柄；存在即代表运行在原生壳内。 */
  __TAURI_INTERNALS__?: {
    invoke?: (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;
    metadata?: Record<string, unknown>;
  };
  __TAURI_EVENT_PLUGIN_INTERNALS__?: Record<string, unknown>;
}

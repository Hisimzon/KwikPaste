/** 可读格式导出契约，对应 Rust readable_export 模块。 */
export interface ReadableExportOptions {
  format: "xlsx" | "markdown";
  favoritesOnly: boolean;
  groupIds: string[] | null;
  includeUngrouped: boolean;
  splitByGroup: boolean;
  includeSensitive: boolean;
}

export interface ReadableExportPreview {
  fingerprint: string;
  itemCount: number;
  fileCount: number;
  excludedSensitive: number;
  referenceCount: number;
  groups: Array<{ id: string; name: string; count: number }>;
}

export interface ReadableExportResult {
  path: string;
  itemCount: number;
  fileCount: number;
}

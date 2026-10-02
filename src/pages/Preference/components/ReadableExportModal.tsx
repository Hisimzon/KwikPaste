import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { open as pickDirectory, save } from "@tauri-apps/plugin-dialog";
import { useMount, useUnmount } from "ahooks";
import {
  Alert,
  Button,
  Checkbox,
  Flex,
  Modal,
  Segmented,
  Select,
  Spin,
  Typography,
} from "antd";
import type { FC } from "react";
import { useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  acquireWindowKeepalive,
  exportReadableData,
  listClipboardGroups,
  previewReadableExport,
  releaseWindowKeepalive,
} from "@/commands";
import type { ClipboardGroupRecord } from "@/types/clipboard";
import type {
  ReadableExportOptions,
  ReadableExportPreview,
  ReadableExportResult,
} from "@/types/readableExport";
import { log } from "@/utils/log";

interface ReadableExportModalProps {
  onCancel: () => void;
  onExported: (result: ReadableExportResult) => void;
}

/**
 * 可读格式导出：选择范围、预览数量，再选择输出位置；每次打开重新读取现有分组。
 */
const ReadableExportModal: FC<ReadableExportModalProps> = (props) => {
  const { onCancel, onExported } = props;
  const { t } = useTranslation(["preferences", "common"]);
  const [groups, setGroups] = useState<ClipboardGroupRecord[]>([]);
  const [groupsReady, setGroupsReady] = useState(false);
  const [options, setOptions] = useState<ReadableExportOptions>({
    favoritesOnly: false,
    format: "xlsx",
    groupIds: null,
    includeSensitive: false,
    includeUngrouped: true,
    splitByGroup: false,
  });
  const [preview, setPreview] = useState<ReadableExportPreview | null>(null);
  const [sensitiveConfirmed, setSensitiveConfirmed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [previewFailed, setPreviewFailed] = useState(false);
  const mounted = useRef(true);
  const windowLabel = getCurrentWebviewWindow().label;
  const groupOptions = groups.map((group) => {
    return { label: group.name, value: group.id };
  });
  const noGroups =
    options.groupIds !== null &&
    options.groupIds.length === 0 &&
    !options.includeUngrouped;
  const confirmDisabled =
    busy ||
    !preview ||
    preview.itemCount === 0 ||
    (options.includeSensitive && !sensitiveConfirmed);

  /**
   * 分组只由 Rust 提供；失败时不把空列表当成已加载，允许重新打开重试。
   */
  const loadGroups = async () => {
    try {
      const next = await listClipboardGroups();
      if (!mounted.current) return;
      setGroups(next);
      setGroupsReady(true);
    } catch (error) {
      log.warn("load readable export groups failed", error);
    }
  };

  useMount(() => {
    void loadGroups();
  });
  useUnmount(() => {
    mounted.current = false;
  });

  /**
   * 选项变化立即作废预览，避免用旧范围的数量确认导出。
   */
  const changeOptions = (patch: Partial<ReadableExportOptions>) => {
    setOptions((current) => {
      return { ...current, ...patch };
    });
    setPreview(null);
    setPreviewFailed(false);
    setSensitiveConfirmed(false);
  };

  const changeFormat = (value: string | number) => {
    changeOptions({ format: value as ReadableExportOptions["format"] });
  };
  const changeRange = (value: string | number) => {
    changeOptions({ favoritesOnly: value === "favorites" });
  };
  const changeGroupMode = (value: string | number) => {
    changeOptions({
      groupIds: value === "all" ? null : [],
      includeUngrouped: true,
    });
  };
  const changeGroups = (value: string[]) => {
    changeOptions({ groupIds: value });
  };
  const changeOutput = (value: string | number) => {
    changeOptions({ splitByGroup: value === "split" });
  };
  const changeUngrouped = (event: { target: { checked: boolean } }) => {
    changeOptions({ includeUngrouped: event.target.checked });
  };
  const changeSensitive = (event: { target: { checked: boolean } }) => {
    changeOptions({ includeSensitive: event.target.checked });
  };
  const confirmSensitive = (event: { target: { checked: boolean } }) => {
    setSensitiveConfirmed(event.target.checked);
  };

  /**
   * 数量预览不包含记录正文；后台指纹还会在真正导出前重新核对。
   */
  const preparePreview = async () => {
    setBusy(true);
    setPreview(null);
    setPreviewFailed(false);
    try {
      setPreview(await previewReadableExport(options));
    } catch (error) {
      setPreviewFailed(true);
      log.warn("preview readable export failed", error);
    } finally {
      setBusy(false);
    }
  };

  /**
   * 原生文件对话框期间保活窗口；拆分导出选择目录，后台在其中新建唯一子目录。
   */
  const exportFiles = async () => {
    if (confirmDisabled || !preview) return;
    const owner = "readable-export";
    setBusy(true);
    try {
      await acquireWindowKeepalive(
        windowLabel,
        owner,
        "readable-export",
        120_000,
      );
      const extension = options.format === "xlsx" ? "xlsx" : "md";
      const stamp = new Date().toISOString().replace(/[:.]/g, "-");
      const target = options.splitByGroup
        ? await pickDirectory({
            directory: true,
            multiple: false,
            title: t("preferences:readableExport.pickDirectory"),
          })
        : await save({
            defaultPath: `KwikPaste-Export-${stamp}.${extension}`,
            filters: [
              {
                extensions: [extension],
                name: options.format === "xlsx" ? "Excel" : "Markdown",
              },
            ],
          });
      if (!target || Array.isArray(target)) return;

      const result = await exportReadableData(
        target,
        options,
        preview.fingerprint,
      );
      onExported(result);
    } catch (error) {
      setPreview(null);
      setPreviewFailed(true);
      log.warn("readable export failed", error);
    } finally {
      try {
        await releaseWindowKeepalive(windowLabel, owner);
      } catch (error) {
        log.warn("release readable export keepalive failed", error);
      }
      if (mounted.current) setBusy(false);
    }
  };

  const cancel = () => {
    if (!busy) onCancel();
  };

  return (
    <Modal
      cancelText={t("common:actions.cancel")}
      centered
      closable={!busy}
      confirmLoading={busy}
      keyboard={!busy}
      maskClosable={!busy}
      okButtonProps={{ disabled: confirmDisabled }}
      okText={t("preferences:readableExport.export")}
      onCancel={cancel}
      onOk={exportFiles}
      open
      styles={{
        body: { maxHeight: "65vh", overflowX: "hidden", overflowY: "auto" },
      }}
      title={t("preferences:readableExport.title")}
    >
      <Spin spinning={busy}>
        <Flex gap="middle" vertical>
          <Alert
            description={t("preferences:readableExport.notBackup")}
            showIcon
            title={t("preferences:readableExport.plainWarning")}
            type="warning"
          />
          <fieldset className="m-0 min-w-0 border-0 p-0" disabled={busy}>
            <Flex gap="middle" vertical>
              <Flex gap="small" vertical>
                <span>{t("preferences:readableExport.format")}</span>
                <Segmented
                  block
                  disabled={busy}
                  onChange={changeFormat}
                  options={[
                    { label: "Excel (.xlsx)", value: "xlsx" },
                    { label: "Markdown (.md)", value: "markdown" },
                  ]}
                  value={options.format}
                />
              </Flex>
              <Flex gap="small" vertical>
                <span>{t("preferences:readableExport.range")}</span>
                <Segmented
                  block
                  disabled={busy}
                  onChange={changeRange}
                  options={[
                    {
                      label: t("preferences:readableExport.allRecords"),
                      value: "all",
                    },
                    {
                      label: t("preferences:readableExport.favorites"),
                      value: "favorites",
                    },
                  ]}
                  value={options.favoritesOnly ? "favorites" : "all"}
                />
              </Flex>
              <Flex gap="small" vertical>
                <span>{t("preferences:readableExport.groups")}</span>
                <Segmented
                  block
                  disabled={busy}
                  onChange={changeGroupMode}
                  options={[
                    {
                      label: t("preferences:readableExport.allGroups"),
                      value: "all",
                    },
                    {
                      label: t("preferences:readableExport.selectedGroups"),
                      value: "selected",
                    },
                  ]}
                  value={options.groupIds === null ? "all" : "selected"}
                />
                {options.groupIds !== null ? (
                  <>
                    <Select
                      disabled={!groupsReady || busy}
                      loading={!groupsReady}
                      mode="multiple"
                      onChange={changeGroups}
                      options={groupOptions}
                      placeholder={t("preferences:readableExport.selectGroups")}
                      value={options.groupIds}
                    />
                    <Checkbox
                      checked={options.includeUngrouped}
                      onChange={changeUngrouped}
                    >
                      {t("preferences:readableExport.ungrouped")}
                    </Checkbox>
                  </>
                ) : null}
              </Flex>
              <Flex gap="small" vertical>
                <span>{t("preferences:readableExport.output")}</span>
                <Segmented
                  block
                  disabled={busy}
                  onChange={changeOutput}
                  options={[
                    {
                      label: t("preferences:readableExport.merged"),
                      value: "merged",
                    },
                    {
                      label: t("preferences:readableExport.split"),
                      value: "split",
                    },
                  ]}
                  value={options.splitByGroup ? "split" : "merged"}
                />
              </Flex>
              <Checkbox
                checked={options.includeSensitive}
                onChange={changeSensitive}
              >
                {t("preferences:readableExport.includeSensitive")}
              </Checkbox>
            </Flex>
          </fieldset>
          {options.includeSensitive ? (
            <Checkbox
              checked={sensitiveConfirmed}
              disabled={busy}
              onChange={confirmSensitive}
            >
              {t("preferences:readableExport.confirmSensitive")}
            </Checkbox>
          ) : null}
          <Button
            disabled={
              busy || noGroups || (options.groupIds !== null && !groupsReady)
            }
            onClick={preparePreview}
          >
            {t("preferences:readableExport.preview")}
          </Button>
          {noGroups ? (
            <Typography.Text type="warning">
              {t("preferences:readableExport.noGroups")}
            </Typography.Text>
          ) : null}
          {previewFailed ? (
            <Typography.Text type="warning">
              {t("preferences:readableExport.retryPreview")}
            </Typography.Text>
          ) : null}
          {preview ? (
            <Flex gap="small" vertical>
              <Typography.Text strong>
                {t("preferences:readableExport.summary", {
                  files: preview.fileCount,
                  groups: preview.groups.length,
                  items: preview.itemCount,
                })}
              </Typography.Text>
              <span className="text-ant-secondary text-sm">
                {t("preferences:readableExport.excluded", {
                  count: preview.excludedSensitive,
                })}
              </span>
              <div className="max-h-36 overflow-y-auto text-ant-secondary text-sm">
                {preview.groups.map((group) => {
                  return (
                    <div key={group.id}>
                      {group.name} · {group.count}
                    </div>
                  );
                })}
              </div>
              {preview.itemCount === 0 ? (
                <Alert
                  title={t("preferences:readableExport.empty")}
                  type="info"
                />
              ) : null}
              {preview.referenceCount > 0 ? (
                <Alert
                  title={t("preferences:readableExport.references", {
                    count: preview.referenceCount,
                  })}
                  type="warning"
                />
              ) : null}
              {options.splitByGroup ? (
                <span className="text-ant-secondary text-sm">
                  {t("preferences:readableExport.newFolder")}
                </span>
              ) : null}
            </Flex>
          ) : null}
        </Flex>
      </Spin>
    </Modal>
  );
};

export default ReadableExportModal;

import { getName, getVersion } from "@tauri-apps/api/app";
import { useMount, useUnmount } from "ahooks";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import type { ChangeEvent, FC, UIEvent } from "react";
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useSnapshot } from "valtio";
import {
  type BackupReceivedPayload,
  type ChangeStorageLocationResult,
  type CleanCacheResult,
  type ExportHistoryBackupResult,
  getStorageLocation,
  getStorageUsage,
  type ImportHistoryBackupResult,
  type StorageLocation,
  type StorageUsage,
  takePendingBackup,
  takePendingPreferenceHighlight,
} from "@/commands";
import ScrollArea from "@/components/ScrollArea";
import WindowMaterialSurface from "@/components/WindowMaterialSurface";
import { TAURI_EVENT } from "@/constants/events";
import { useTauriListen } from "@/hooks/useTauriListen";
import { settingsState } from "@/stores/settings";
import { preloadSourceApps, reloadSourceApps } from "@/stores/sourceApps";
import type { Settings } from "@/types/settings";
import { log } from "@/utils/log";
import BackupImportModal from "./components/BackupImportModal";
import PreferenceHeader from "./components/PreferenceHeader";
import PreferenceSection from "./components/PreferenceSection";
import PreferenceSidebar from "./components/PreferenceSidebar";
import {
  isPreferenceSettingCollapsed,
  preferenceTabs,
} from "./config/preferenceSchema";
import { confirmHistoryCleanupChange } from "./services/historyCleanup";
import {
  commitSettingChange,
  settingValuesEqual,
} from "./services/preferenceSettings";
import type {
  PreferenceSetting,
  PreferenceStorageState,
  PreferenceTabId,
  SettingValue,
} from "./types/preferences";
import {
  resetContentScroll,
  resolveVisibleSectionId,
  scrollHighlightedSetting,
  scrollToSection,
} from "./utils/preferenceScroll";
import {
  type PreferenceSearchResult,
  searchPreferenceSettings,
} from "./utils/preferenceSearch";
import { buildNextHistory } from "./utils/retention";

type PreferenceHighlightTarget = {
  settingId: string;
  token: number;
};

interface ClipboardCleanupPayload {
  cleanup?: number;
}

const STORAGE_OVERVIEW_TAB_ID: PreferenceTabId = "data";
const STORAGE_OVERVIEW_SECTION_ID = "overview";
/** 程序滚动开始前的等待上限：定位高亮要等分类切换动画后才滚。 */
const SECTION_TRACKING_HOLD_MS = 400;
/** 滚动事件停止这么久即视为程序滚动结束，恢复目录跟随。 */
const SECTION_TRACKING_SETTLE_MS = 150;

interface PreferenceHighlightSettingPayload {
  settingId: string;
}

interface AppMetadata {
  name: string;
  version: string;
}

/**
 * KwikPaste 偏好设置：以用户心智组织设置，而非代码模块。
 */
const Preference: FC = () => {
  const { t } = useTranslation("preferences");
  const settings = useSnapshot(settingsState) as Settings;
  const shouldReduceMotion = useReducedMotion();
  const reduceMotion = shouldReduceMotion === true;
  const contentRef = useRef<HTMLDivElement | null>(null);
  const sectionTrackingHeldRef = useRef(false);
  const sectionTrackingTimerRef = useRef<number | undefined>(void 0);
  const [activeTabId, setActiveTabId] = useState<PreferenceTabId>(
    preferenceTabs[0].id,
  );
  const [activeSectionId, setActiveSectionId] = useState(
    preferenceTabs[0].sections[0]?.id ?? "",
  );
  const [searchQuery, setSearchQuery] = useState("");
  const [highlightTarget, setHighlightTarget] =
    useState<PreferenceHighlightTarget | null>(null);
  const [appMetadata, setAppMetadata] = useState<AppMetadata>({
    name: "",
    version: "",
  });
  const [storageState, setStorageState] =
    useState<PreferenceStorageState>("loading");
  const [storageUsage, setStorageUsage] = useState<StorageUsage | null>(null);
  const [storageLocation, setStorageLocation] =
    useState<StorageLocation | null>(null);
  const [backupImportTarget, setBackupImportTarget] =
    useState<BackupReceivedPayload | null>(null);

  const activeTab =
    preferenceTabs.find((tab) => {
      return tab.id === activeTabId;
    }) ?? preferenceTabs[0];
  const searchResults = useMemo(() => {
    return searchPreferenceSettings(searchQuery, t);
  }, [searchQuery, t]);
  const totalSettings = activeTab.sections.reduce((total, section) => {
    const visibleSettings = section.settings.filter((setting) => {
      return !isPreferenceSettingCollapsed(setting, settings);
    });

    return total + visibleSettings.length;
  }, 0);
  const hasSourceSection = activeTab.sections.some((section) => {
    return section.id === "source";
  });

  const handleSearchChange = (event: ChangeEvent<HTMLInputElement>) => {
    setSearchQuery(event.target.value);
  };

  const handleTabSelect = (nextTabId: PreferenceTabId) => {
    const nextTab =
      preferenceTabs.find((tab) => {
        return tab.id === nextTabId;
      }) ?? preferenceTabs[0];
    const nextSectionId = nextTab.sections[0]?.id ?? "";

    setActiveTabId(nextTabId);
    setActiveSectionId(nextSectionId);
    resetContentScroll(contentRef.current);
  };

  const handleSectionSelect = (sectionId: string) => {
    setActiveSectionId(sectionId);
    holdSectionTracking(SECTION_TRACKING_HOLD_MS);
    scrollToSection(contentRef.current, sectionId, reduceMotion);
  };

  /**
   * 程序触发的滚动期间暂停目录跟随，避免平滑滚动途经的分组抢走刚选中的高亮。
   */
  const holdSectionTracking = (releaseDelay: number) => {
    sectionTrackingHeldRef.current = true;
    window.clearTimeout(sectionTrackingTimerRef.current);
    sectionTrackingTimerRef.current = window.setTimeout(() => {
      sectionTrackingHeldRef.current = false;
    }, releaseDelay);
  };

  /**
   * 用户滚动内容区时让页内目录跟随当前阅读的分组；内部嵌套滚动区不参与。
   */
  const handleContentScroll = (event: UIEvent<HTMLDivElement>) => {
    const container = contentRef.current;
    if (!container || event.target !== container) return;

    if (sectionTrackingHeldRef.current) {
      holdSectionTracking(SECTION_TRACKING_SETTLE_MS);
      return;
    }

    const sectionIds = activeTab.sections.map((section) => {
      return section.id;
    });
    const sectionId = resolveVisibleSectionId(container, sectionIds);
    if (!sectionId) return;

    setActiveSectionId(sectionId);
  };

  const handlePickSearchResult = (result: PreferenceSearchResult) => {
    setActiveTabId(result.tab.id);
    setActiveSectionId(result.section.id);
    setSearchQuery("");
    highlightSetting(result.setting.id);
  };

  /**
   * 切换到指定设置项所属分类，并触发滚动高亮。
   * 子项在父开关关闭时是收起的，此时改为高亮父开关，提示先打开它。
   */
  const highlightSetting = (settingId: string) => {
    const target = findPreferenceSetting(settingId);
    if (!target) return;

    const { parentId } = target.setting;
    const collapsed = isPreferenceSettingCollapsed(
      target.setting,
      settingsState as Settings,
    );

    setActiveTabId(target.tab.id);
    setActiveSectionId(target.section.id);
    setSearchQuery("");
    holdSectionTracking(SECTION_TRACKING_HOLD_MS);
    setHighlightTarget((currentTarget) => {
      return {
        settingId: collapsed && parentId ? parentId : settingId,
        token: (currentTarget?.token ?? 0) + 1,
      };
    });
  };

  /**
   * 侧栏存储卡片直达数据概览。
   */
  const openStorageOverview = () => {
    setActiveTabId(STORAGE_OVERVIEW_TAB_ID);
    setActiveSectionId(STORAGE_OVERVIEW_SECTION_ID);
    setSearchQuery("");
    resetContentScroll(contentRef.current);
  };

  /**
   * 数据概览或清理操作拿到最新占用后同步侧栏，省一次单独统计。
   */
  const handleStorageUsageChange = (usage: StorageUsage) => {
    setStorageUsage(usage);
    setStorageState("ready");
  };

  /**
   * 保存单个设置；清理设置保存后若会立即删除记录，先预演并让用户确认。
   * 返回 `false` 表示没有保存，控件据此回退草稿值。
   */
  const handleSettingChange = async (
    setting: PreferenceSetting,
    value: SettingValue,
  ) => {
    if (!setting.path) return false;

    const currentValue = setting.value?.(settings);
    if (currentValue === void 0) return false;
    if (settingValuesEqual(currentValue, value)) return true;

    const nextHistory = buildNextHistory(
      settings.clipboard.history,
      setting.path,
      value,
    );
    if (nextHistory) {
      const confirmed = await confirmHistoryCleanupChange(nextHistory);
      if (!confirmed) return false;
    }

    try {
      await commitSettingChange(setting, value);

      return true;
    } catch {
      // 错误 toast 已由 commands 层统一处理；设置镜像等待 Rust 事件回灌。
      return false;
    }
  };

  const highlightBackupImport = () => {
    setActiveTabId("data");
    setActiveSectionId("backup");
    holdSectionTracking(SECTION_TRACKING_HOLD_MS);
    setHighlightTarget((currentTarget) => {
      return {
        settingId: "backup.importHistory",
        token: (currentTarget?.token ?? 0) + 1,
      };
    });
  };

  const handleBackupReceived = (payload: BackupReceivedPayload) => {
    highlightBackupImport();
    setBackupImportTarget(payload);
  };

  /**
   * 关闭备份导入弹窗并丢弃当前接收的文件路径。
   */
  const closeBackupImportModal = () => {
    setBackupImportTarget(null);
  };

  /**
   * 导入完成后刷新存储占用。
   */
  const handleBackupImported = (_result: ImportHistoryBackupResult) => {
    closeBackupImportModal();
    void initializeStorageUsage();
  };

  const handleActionComplete = (
    setting: PreferenceSetting,
    result?:
      | ChangeStorageLocationResult
      | CleanCacheResult
      | ExportHistoryBackupResult,
  ) => {
    if (result && "location" in result) {
      setStorageLocation(result.location);
      setStorageUsage(result.storageUsage);
      setStorageState("ready");
      return;
    }

    if (result && "storageUsage" in result) {
      setStorageUsage(result.storageUsage);
      setStorageState("ready");
      return;
    }

    if (
      setting.id.startsWith("localData.") ||
      setting.id.startsWith("backup.")
    ) {
      void initializeStorageUsage();
    }
  };

  /**
   * 加载当前环境数据目录的递归占用，用于侧栏低频状态展示。
   */
  const initializeStorageUsage = async () => {
    setStorageState("loading");

    try {
      const [usage, location] = await Promise.all([
        getStorageUsage(),
        getStorageLocation(),
      ]);

      setStorageUsage(usage);
      setStorageLocation(location);
      setStorageState("ready");
    } catch (error) {
      log.warn("load storage usage failed", error);
      setStorageState("error");
    }
  };

  /**
   * 从 Tauri 应用元信息读取展示名称和版本，避免前端手写包信息。
   */
  const initializeAppMetadata = async () => {
    try {
      const [name, version] = await Promise.all([getName(), getVersion()]);

      setAppMetadata({ name, version });
    } catch (error) {
      log.warn("load app metadata failed", error);
    }
  };

  useMount(async () => {
    void initializeStorageUsage();
    void initializeAppMetadata();
    void preloadSourceApps();

    // 窗口空闲销毁后重建时，备份接收 / 定位高亮事件已无法 push 给刚挂载的前端，改为主动拉取暂存值。
    const pendingBackup = await takePendingBackup();

    if (pendingBackup) {
      handleBackupReceived(pendingBackup);
    }

    const pendingHighlight = await takePendingPreferenceHighlight();

    if (pendingHighlight) {
      highlightSetting(pendingHighlight);
    }
  });

  useUnmount(() => {
    window.clearTimeout(sectionTrackingTimerRef.current);
  });

  useTauriListen<BackupReceivedPayload>(
    TAURI_EVENT.BACKUP_RECEIVED,
    (event) => {
      handleBackupReceived(event.payload);
    },
  );

  useTauriListen<PreferenceHighlightSettingPayload>(
    TAURI_EVENT.PREFERENCE_HIGHLIGHT_SETTING,
    (event) => {
      highlightSetting(event.payload.settingId);
    },
  );

  // 后台按存储上限清理后占用会变，侧栏需要跟着刷新。
  useTauriListen<ClipboardCleanupPayload>(
    TAURI_EVENT.CLIPBOARD_UPDATED,
    (event) => {
      if (event.payload.cleanup === void 0) return;

      void initializeStorageUsage();
    },
  );

  useEffect(() => {
    if (!highlightTarget) return;

    const scrollToTarget = () => {
      scrollHighlightedSetting(
        contentRef.current,
        highlightTarget.settingId,
        reduceMotion,
      );
    };
    const scrollTimer = window.setTimeout(
      scrollToTarget,
      reduceMotion ? 0 : 140,
    );
    // 同页上方的数据概览等分组异步加载后会变高，高亮期间内容尺寸一变就重新对准目标。
    const resizeObserver = new ResizeObserver(scrollToTarget);
    const content = contentRef.current?.firstElementChild;
    if (content) resizeObserver.observe(content);

    const clearTimer = window.setTimeout(
      () => {
        setHighlightTarget((currentTarget) => {
          if (!currentTarget) return currentTarget;
          if (currentTarget.token !== highlightTarget.token) {
            return currentTarget;
          }

          return null;
        });
      },
      reduceMotion ? 1200 : 2200,
    );

    return () => {
      window.clearTimeout(scrollTimer);
      window.clearTimeout(clearTimer);
      resizeObserver.disconnect();
    };
  }, [highlightTarget, reduceMotion]);

  useEffect(() => {
    if (!hasSourceSection) return;

    void reloadSourceApps();
  }, [hasSourceSection]);

  if (!activeTab) return null;

  return (
    <WindowMaterialSurface
      className="h-screen overflow-hidden text-ant-text"
      tone="layout"
    >
      <motion.div
        animate={{ opacity: 1, y: 0 }}
        className="flex h-full overflow-hidden"
        data-tauri-drag-region
        initial={{ opacity: 0, y: reduceMotion ? 0 : 6 }}
        transition={{ duration: reduceMotion ? 0 : 0.18, ease: "easeOut" }}
      >
        <PreferenceSidebar
          activeTabId={activeTabId}
          appName={appMetadata.name}
          appVersion={appMetadata.version}
          onStorageSelect={openStorageOverview}
          onTabSelect={handleTabSelect}
          storageLimitMb={settings.clipboard.history.storageLimitMb}
          storageState={storageState}
          storageUsage={storageUsage}
        />

        <main className="flex min-w-0 flex-1 flex-col overflow-hidden">
          <PreferenceHeader
            activeSectionId={activeSectionId}
            activeTab={activeTab}
            onPickSearchResult={handlePickSearchResult}
            onSearchChange={handleSearchChange}
            onSectionSelect={handleSectionSelect}
            searchQuery={searchQuery}
            searchResults={searchResults}
            shouldReduceMotion={reduceMotion}
            totalSettings={totalSettings}
          />

          <ScrollArea
            className="min-h-0 flex-1"
            contentClassName="p-6"
            data-tauri-drag-region
            onScrollCapture={handleContentScroll}
            ref={contentRef}
          >
            <AnimatePresence mode="wait">
              <motion.div
                animate={{ opacity: 1 }}
                className="flex max-w-228 flex-col gap-6"
                exit={{ opacity: 0 }}
                initial={{ opacity: 0 }}
                key={activeTabId}
                transition={{
                  duration: reduceMotion ? 0 : 0.12,
                  ease: "easeOut",
                }}
              >
                {activeTab.sections.map((section) => {
                  return (
                    <PreferenceSection
                      highlightedSettingId={highlightTarget?.settingId ?? null}
                      highlightToken={highlightTarget?.token ?? 0}
                      key={section.id}
                      onActionComplete={handleActionComplete}
                      onChange={handleSettingChange}
                      onNavigateSetting={highlightSetting}
                      onStorageUsageChange={handleStorageUsageChange}
                      section={section}
                      settings={settings}
                      shouldReduceMotion={reduceMotion}
                      storageLocation={storageLocation}
                    />
                  );
                })}
              </motion.div>
            </AnimatePresence>
          </ScrollArea>
        </main>
      </motion.div>

      <BackupImportModal
        onCancel={closeBackupImportModal}
        onImported={handleBackupImported}
        open={backupImportTarget !== null}
        target={backupImportTarget}
      />
    </WindowMaterialSurface>
  );
};

/**
 * 从完整偏好设置 schema 中找到指定设置项及其所属层级。
 */
function findPreferenceSetting(settingId: string) {
  for (const tab of preferenceTabs) {
    for (const section of tab.sections) {
      const setting = section.settings.find((item) => {
        return item.id === settingId;
      });
      if (!setting) continue;

      return { section, setting, tab };
    }
  }

  return null;
}

export default Preference;
